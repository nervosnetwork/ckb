//! Reconstruct projections from fixture facts after complete owner transitions.
//! The matrix exercises Store directly; planner sequences exercise its producers.
use super::*;
use crate::authority::{
    budget::{OwnerUsage, owner_amount},
    model::{Accepted, Source, Status},
    notice::Class,
    tests::common::{
        config, delete, entry, insert, output_tx, spend, store_with_pipeline_limit, verified,
    },
};
use ckb_types::core::{
    Capacity, TransactionView,
    cell::{CellMeta, ResolvedTransaction},
};

#[derive(Clone, Copy, Debug)]
enum Shape {
    Resolve,
    Verify,
    Waiting,
    HistoryAll,
    HistoryAny,
    Pending,
    Gap,
    Proposed,
}

impl Shape {
    const ALL: [Self; 8] = [
        Self::Resolve,
        Self::Verify,
        Self::Waiting,
        Self::HistoryAll,
        Self::HistoryAny,
        Self::Pending,
        Self::Gap,
        Self::Proposed,
    ];

    fn accepted(self) -> bool {
        matches!(self, Self::Pending | Self::Gap | Self::Proposed)
    }

    fn history(self) -> bool {
        matches!(self, Self::HistoryAll | Self::HistoryAny)
    }
}

#[derive(Clone, Copy, Debug)]
struct Origin {
    source: Source,
    peer: Option<PeerIndex>,
    deadline: Option<Instant>,
}

impl Origin {
    fn trusted(source: Source) -> Self {
        Self {
            source,
            peer: None,
            deadline: None,
        }
    }

    fn all() -> [Self; 7] {
        let early = Instant::now() + std::time::Duration::from_secs(60);
        let late = early + std::time::Duration::from_secs(60);
        let remote = |peer: PeerIndex, deadline| Self {
            source: Source::Remote {
                peer,
                deadline,
                cycles: Some(1),
            },
            peer: Some(peer),
            deadline: Some(deadline),
        };
        [
            Self::trusted(Source::Local),
            Self::trusted(Source::Recovery),
            Self::trusted(Source::Proposal { remote: None }),
            remote(1.into(), early),
            remote(1.into(), late),
            remote(2.into(), late),
            Self {
                source: Source::Proposal {
                    remote: Some((1.into(), early)),
                },
                peer: Some(1.into()),
                deadline: Some(early),
            },
        ]
    }
}

struct Fixture {
    parent: TransactionView,
    transaction: TransactionView,
    input: OutPoint,
    dependency: OutPoint,
    header: Byte32,
    resolved: Arc<ResolvedTransaction>,
}

impl Fixture {
    fn new() -> Self {
        let parent = output_tx(30_000);
        let input = OutPoint::new(parent.hash(), 0);
        let dependency = OutPoint::new(Byte32::new([0x51; 32]), 0);
        let header = Byte32::new([0x52; 32]);
        let transaction = spend(
            30_001,
            std::slice::from_ref(&input),
            &[input.clone(), dependency.clone(), dependency.clone()],
        )
        .as_advanced_builder()
        .header_dep(header.clone())
        .build();
        let cell = |point| CellMeta {
            out_point: point,
            ..CellMeta::default()
        };
        let resolved = Arc::new(ResolvedTransaction {
            transaction: transaction.clone(),
            resolved_inputs: vec![cell(input.clone())],
            resolved_cell_deps: vec![
                cell(input.clone()),
                cell(dependency.clone()),
                cell(dependency.clone()),
            ],
            resolved_dep_groups: Vec::new(),
        });
        Self {
            parent,
            transaction,
            input,
            dependency,
            header,
            resolved,
        }
    }

    fn owner(
        &self,
        store: &Store,
        original: &Entry,
        shape: Shape,
        origin: Origin,
        timestamp: u64,
    ) -> Arc<Entry> {
        let phase = match shape {
            Shape::Resolve => Phase::Resolve,
            Shape::Verify => Phase::Verify(Arc::clone(
                verified(store, original, 11, 19, Status::Pending).resolved(),
            )),
            Shape::Waiting => Phase::Waiting(BTreeSet::from([
                DependencyKey::Cell(self.dependency.clone()),
                DependencyKey::Header(self.header.clone()),
            ])),
            Shape::HistoryAll | Shape::HistoryAny => Phase::Replaced {
                triggers: BTreeSet::from([
                    DependencyKey::Cell(self.input.clone()),
                    DependencyKey::Cell(self.dependency.clone()),
                ]),
                require_all: matches!(shape, Shape::HistoryAll),
            },
            Shape::Pending | Shape::Gap | Shape::Proposed => Phase::Accepted(Accepted {
                transaction: Arc::clone(&self.resolved),
                cycles: 19,
                fee: Capacity::shannons(11),
                size: self.transaction.data().serialized_size_in_block(),
                timestamp,
                parents: BTreeSet::from([self.parent.hash()]),
                context_sensitive: false,
                forced_status: Some(match shape {
                    Shape::Pending => Status::Pending,
                    Shape::Gap => Status::Gap,
                    _ => Status::Proposed,
                }),
            }),
        };
        Arc::new(Entry {
            source: origin.source,
            phase,
            ..original.clone()
        })
    }

    fn roles(&self, shape: Shape) -> Vec<(RelationKey, u8)> {
        let cell = |point| RelationKey::Dependency(DependencyKey::Cell(point));
        match shape {
            Shape::Resolve | Shape::Verify => Vec::new(),
            Shape::Waiting => vec![
                (cell(self.dependency.clone()), WAIT),
                (
                    RelationKey::Dependency(DependencyKey::Header(self.header.clone())),
                    WAIT,
                ),
            ],
            Shape::HistoryAll | Shape::HistoryAny => vec![
                (cell(self.input.clone()), WAIT),
                (cell(self.dependency.clone()), WAIT),
            ],
            Shape::Pending | Shape::Gap | Shape::Proposed => vec![
                (cell(self.input.clone()), INPUT | DEP),
                (cell(self.dependency.clone()), DEP),
                (RelationKey::Children(self.parent.hash()), CHILD),
            ],
        }
    }
}

struct Expected {
    owner: Arc<Entry>,
    shape: Shape,
    origin: Origin,
    timestamp: u64,
    roles: Vec<(RelationKey, u8)>,
}

fn add(total: &mut Amount, amount: Amount) {
    // These small fixture totals do not use the production delta arithmetic.
    total.items += amount.items;
    total.bytes += amount.bytes;
    total.edges += amount.edges;
    total.serialized += amount.serialized;
    total.cycles += amount.cycles;
}

fn assert_state(store: &Store, expected: &[Expected], wakes: &[DependencyKey]) {
    let mut usage = OwnerUsage::default();
    let mut owners = BTreeMap::new();
    let mut proposals = BTreeMap::new();
    let mut deadlines = BTreeSet::new();
    let mut accepted_times = BTreeSet::new();
    let mut roles: BTreeMap<RelationKey, BTreeMap<Byte32, u8>> = BTreeMap::new();
    let mut peers: BTreeMap<PeerIndex, BTreeSet<Byte32>> = BTreeMap::new();
    let mut queued = [Vec::new(), Vec::new()];
    let mut orphan = 0;
    let mut proposed = 0;
    for expected in expected {
        assert_eq!(expected.owner.source, expected.origin.source);
        assert!(
            match (&expected.owner.phase, expected.shape) {
                (Phase::Resolve, Shape::Resolve)
                | (Phase::Verify(_), Shape::Verify)
                | (Phase::Waiting(_), Shape::Waiting)
                | (
                    Phase::Replaced {
                        require_all: true, ..
                    },
                    Shape::HistoryAll,
                )
                | (
                    Phase::Replaced {
                        require_all: false, ..
                    },
                    Shape::HistoryAny,
                ) => true,
                (Phase::Accepted(accepted), shape) => {
                    let status = accepted.status(&store.snapshot().1);
                    matches!(
                        (shape, status),
                        (Shape::Pending, Status::Pending)
                            | (Shape::Gap, Status::Gap)
                            | (Shape::Proposed, Status::Proposed)
                    )
                }
                _ => false,
            },
            "unexpected phase for {:?}",
            expected.owner.hash()
        );
        let hash = expected.owner.hash();
        owners.insert(hash.clone(), Arc::clone(&expected.owner));
        let proposal: [u8; ProposalShortId::TOTAL_SIZE] =
            expected.owner.proposal().as_slice().try_into().unwrap();
        proposals.insert(proposal, hash.clone());
        // Share individual sizing, but derive account routing and aggregation
        // independently of owner_accounts and OwnerDelta.
        let amount = owner_amount(&expected.owner).unwrap();
        if expected.shape.accepted() {
            add(&mut usage.accepted, amount);
            accepted_times.insert((expected.timestamp, hash.clone()));
        } else {
            add(&mut usage.pipeline, amount);
            if expected.shape.history() {
                add(&mut usage.history, amount);
            } else {
                if let Some(peer) = expected.origin.peer {
                    add(&mut usage.remote, amount);
                    add(usage.peers.entry(peer).or_default(), amount);
                    peers.entry(peer).or_default().insert(hash.clone());
                }
                if let Some(deadline) = expected.origin.deadline {
                    deadlines.insert((deadline, hash.clone()));
                }
            }
        }
        match expected.shape {
            Shape::Resolve => queued[0].push(&expected.owner),
            Shape::Verify => queued[1].push(&expected.owner),
            Shape::Waiting => orphan += 1,
            Shape::Proposed => proposed += 1,
            _ => {}
        }
        for (key, role) in &expected.roles {
            assert!(
                roles
                    .entry(key.clone())
                    .or_default()
                    .insert(hash.clone(), *role)
                    .is_none()
            );
        }
    }
    let mut actual_owners = BTreeMap::new();
    let mut actual_proposals = BTreeMap::new();
    let mut actual_deadlines = BTreeSet::new();
    let mut actual_times = BTreeSet::new();
    let mut actual_orphan = 0;
    let mut actual_proposed = 0;
    for shard in &store.shards {
        let shard = shard.read();
        for (hash, owner) in &shard.owners {
            assert!(
                actual_owners
                    .insert(hash.clone(), Arc::clone(owner))
                    .is_none()
            );
        }
        for (id, hash) in &shard.proposals {
            assert!(actual_proposals.insert(*id, hash.clone()).is_none());
        }
        for deadline in &shard.deadlines {
            assert!(actual_deadlines.insert(deadline.clone()));
        }
        for timestamp in &shard.accepted_times {
            assert!(actual_times.insert(timestamp.clone()));
        }
        actual_orphan += shard.orphan;
        actual_proposed += shard.proposed;
    }
    assert_eq!(actual_owners.len(), owners.len());
    for (hash, owner) in &owners {
        assert!(Arc::ptr_eq(actual_owners.get(hash).unwrap(), owner));
        assert!(Arc::ptr_eq(&store.point(hash).1.unwrap(), owner));
    }
    assert_eq!(actual_proposals, proposals);
    assert_eq!(actual_deadlines, deadlines);
    assert_eq!(actual_times, accepted_times);
    assert_eq!((actual_orphan, actual_proposed), (orphan, proposed));
    let mut actual_roles = BTreeMap::new();
    let mut actual_wakes = BTreeSet::new();
    for collection in &store.relations {
        for (key, relation) in collection.lock().iter() {
            let relation = relation.lock();
            let mut members: BTreeMap<_, _> = relation
                .members
                .iter()
                .map(|(hash, member)| (hash.clone(), member.roles))
                .collect();
            if let Some(hash) = &relation.spender {
                *members.entry(hash.clone()).or_default() |= INPUT;
            }
            if let Some(wake) = &relation.wake {
                let RelationKey::Dependency(key) = key else {
                    panic!("child relations cannot carry availability wakes");
                };
                assert_eq!(wake.pass, relation.next_pass);
                assert!(wake.after.is_none());
                assert!(actual_wakes.insert(key.clone()));
            }
            assert!(
                !members.is_empty(),
                "retired relation {key:?} remains allocated"
            );
            assert!(actual_roles.insert(key.clone(), members).is_none());
        }
    }
    assert_eq!(actual_roles, roles);
    assert_eq!(actual_wakes, wakes.iter().cloned().collect());
    assert_eq!(*store.dirty.lock(), actual_wakes);
    let actual_peers: BTreeMap<_, _> = store
        .peers
        .lock()
        .iter()
        .map(|(peer, row)| (*peer, row.lock().members.clone()))
        .collect();
    assert_eq!(actual_peers, peers);
    let actual_queues = store.queues.queued_owners();
    assert_eq!(
        store.queues.queued_len(),
        queued.iter().map(Vec::len).sum::<usize>()
    );
    for (actual, expected) in actual_queues.iter().zip(queued) {
        let mut actual: Vec<_> = actual.iter().map(Weak::as_ptr).collect();
        let mut expected: Vec<_> = expected.into_iter().map(Arc::as_ptr).collect();
        actual.sort_unstable();
        expected.sort_unstable();
        assert_eq!(actual, expected);
    }
    assert_eq!(store.budget.owner_usage(), usage);
    assert!(!store.is_faulted());
}

#[test]
fn every_phase_and_source_pair_preserves_projections_and_rejects_stale_replay() {
    let store = store_with_pipeline_limit(
        crate::test_support::genesis_snapshot(),
        &config(),
        64_000_000,
    );
    let fixture = Fixture::new();
    let parent = entry(&store, fixture.parent.clone(), Source::Local);
    let parent = parent.with_phase(Phase::Accepted(Accepted {
        transaction: Arc::new(ResolvedTransaction::dummy_resolve(fixture.parent.clone())),
        cycles: 1,
        fee: Capacity::shannons(1),
        size: fixture.parent.data().serialized_size_in_block(),
        timestamp: 0,
        parents: BTreeSet::new(),
        context_sensitive: false,
        forced_status: Some(Status::Pending),
    }));
    insert(&store, Arc::clone(&parent));
    let base = || Expected {
        owner: Arc::clone(&parent),
        shape: Shape::Pending,
        origin: Origin::trusted(Source::Local),
        timestamp: 0,
        roles: Vec::new(),
    };
    let origins = Origin::all();
    for before_shape in Shape::ALL {
        for before_origin in origins {
            for after_shape in Shape::ALL {
                for after_origin in origins {
                    let original = entry(&store, fixture.transaction.clone(), before_origin.source);
                    let before = fixture.owner(&store, &original, before_shape, before_origin, 11);
                    insert(&store, Arc::clone(&before));
                    assert_state(
                        &store,
                        &[
                            base(),
                            Expected {
                                owner: Arc::clone(&before),
                                shape: before_shape,
                                origin: before_origin,
                                timestamp: 11,
                                roles: fixture.roles(before_shape),
                            },
                        ],
                        &[],
                    );
                    let (view, _, _, all_reads) = store.capture(false);
                    let accepted_reads = store.capture(true).3;
                    let after = fixture.owner(&store, &before, after_shape, after_origin, 29);
                    let mut transition = Plan::new(view, Class::Trusted, ReadSet::default());
                    transition
                        .edit(Some(Arc::clone(&before)), Some(Arc::clone(&after)), None)
                        .unwrap();
                    store.apply(transition.clone()).unwrap();
                    let current = || Expected {
                        owner: Arc::clone(&after),
                        shape: after_shape,
                        origin: after_origin,
                        timestamp: 29,
                        roles: fixture.roles(after_shape),
                    };
                    // Releasing an accepted input makes all-blocker history
                    // eligible. Any-trigger history waits for a later event.
                    let wakes =
                        if before_shape.accepted() && matches!(after_shape, Shape::HistoryAll) {
                            vec![DependencyKey::Cell(fixture.input.clone())]
                        } else {
                            Vec::new()
                        };
                    assert_state(&store, &[base(), current()], &wakes);
                    assert!(
                        matches!(store.apply(transition), Err(Error::Stale)),
                        "{before_shape:?}/{before_origin:?} -> {after_shape:?}/{after_origin:?}"
                    );
                    assert!(matches!(
                        store.apply(Plan::new(view, Class::Trusted, all_reads)),
                        Err(Error::Stale)
                    ));
                    let accepted_read =
                        store.apply(Plan::new(view, Class::Trusted, accepted_reads));
                    if before_shape.accepted() || after_shape.accepted() {
                        assert!(matches!(accepted_read, Err(Error::Stale)));
                    } else {
                        accepted_read.unwrap();
                    }
                    assert_state(&store, &[base(), current()], &wakes);
                    store.apply(delete(&store, after)).unwrap();
                    assert_state(&store, &[base()], &[]);
                }
            }
        }
    }
    store.apply(delete(&store, parent)).unwrap();
    assert_state(&store, &[], &[]);
}

#[test]
fn admission_replacement_recovery_reorg_and_clear_preserve_the_whole_population() {
    use crate::authority::tests::common::{admission, tx};
    use crate::authority::{chain, ingress, jobs::Resolution, membership, waiting};
    use crate::service::ChainReorgArgs;
    use ckb_types::core::{BlockBuilder, FeeRate};

    let configuration = ckb_app_config::TxPoolConfig {
        min_rbf_rate: FeeRate::from_u64(1_000),
        ..config()
    };
    let base = crate::test_support::genesis_snapshot();
    let store = store_with_pipeline_limit(Arc::clone(&base), &configuration, 64_000_000);
    let local = Origin::trusted(Source::Local);
    let recovery = Origin::trusted(Source::Recovery);
    let remote = Origin::all()[3];
    let parent = output_tx(31_000);
    let parent_output = OutPoint::new(parent.hash(), 0);
    let child = spend(31_001, std::slice::from_ref(&parent_output), &[]);
    let child_output = OutPoint::new(child.hash(), 0);
    let grandchild = spend(31_002, std::slice::from_ref(&child_output), &[]);
    let replacement = spend(31_003, std::slice::from_ref(&parent_output), &[]);
    let cell = |point: &OutPoint| RelationKey::Dependency(DependencyKey::Cell(point.clone()));
    let child_roles = || {
        vec![
            (cell(&parent_output), INPUT),
            (RelationKey::Children(parent.hash()), CHILD),
        ]
    };
    let grandchild_roles = || {
        vec![
            (cell(&child_output), INPUT),
            (RelationKey::Children(child.hash()), CHILD),
        ]
    };
    let current = |transaction: &TransactionView| store.point(&transaction.hash()).1.unwrap();
    let expected = |transaction: &TransactionView, shape, origin, roles| Expected {
        owner: current(transaction),
        shape,
        origin,
        timestamp: 0,
        roles,
    };
    let accepted_parent = || expected(&parent, Shape::Pending, local, vec![]);
    let admit = |transaction: &TransactionView, origin: Origin, fee| {
        let owner = store
            .point(&transaction.hash())
            .1
            .unwrap_or_else(|| entry(&store, transaction.clone(), origin.source));
        assert_eq!(owner.source, origin.source);
        let (plan, reject) =
            admission(&store, &owner, fee, 19, Status::Pending, &configuration).unwrap();
        assert!(reject.is_none(), "fixture admission rejected: {reject:?}");
        store.apply(plan).unwrap();
    };
    let wake = || {
        let plan = waiting::wake(&store, &mut None)
            .unwrap()
            .expect("a recorded availability event has work");
        store.apply(plan).unwrap();
    };

    // Verified fixtures isolate authority composition from canonical script execution.
    insert(&store, entry(&store, parent.clone(), local.source));
    insert(&store, entry(&store, child.clone(), remote.source));
    let child_wait = Resolution::Waiting(
        BTreeSet::from([DependencyKey::Cell(parent_output.clone())]),
        ReadSet::default(),
    );
    store
        .apply(
            ingress::resolution(&store, store.snapshot().0, &current(&child), &child_wait).unwrap(),
        )
        .unwrap();
    assert_state(
        &store,
        &[
            expected(&parent, Shape::Resolve, local, vec![]),
            expected(
                &child,
                Shape::Waiting,
                remote,
                vec![(cell(&parent_output), WAIT)],
            ),
        ],
        &[],
    );

    let resolved = verified(&store, &current(&parent), 1_000, 19, Status::Pending);
    store
        .apply(
            ingress::resolution(
                &store,
                store.snapshot().0,
                &current(&parent),
                &Resolution::Ready(Arc::clone(resolved.resolved())),
            )
            .unwrap(),
        )
        .unwrap();
    assert_state(
        &store,
        &[
            expected(&parent, Shape::Verify, local, vec![]),
            expected(
                &child,
                Shape::Waiting,
                remote,
                vec![(cell(&parent_output), WAIT)],
            ),
        ],
        &[],
    );
    admit(&parent, local, 1_000);
    assert_state(
        &store,
        &[
            accepted_parent(),
            expected(
                &child,
                Shape::Waiting,
                remote,
                vec![(cell(&parent_output), WAIT)],
            ),
        ],
        &[DependencyKey::Cell(parent_output.clone())],
    );
    wake();
    assert_state(
        &store,
        &[
            accepted_parent(),
            expected(&child, Shape::Resolve, remote, vec![]),
        ],
        &[],
    );

    let (stale_admission, reject) = admission(
        &store,
        &current(&child),
        1_000,
        19,
        Status::Pending,
        &configuration,
    )
    .unwrap();
    assert!(reject.is_none());
    let resolved = verified(&store, &current(&child), 1_000, 19, Status::Pending);
    store
        .apply(
            ingress::resolution(
                &store,
                store.snapshot().0,
                &current(&child),
                &Resolution::Ready(Arc::clone(resolved.resolved())),
            )
            .unwrap(),
        )
        .unwrap();
    assert!(matches!(store.apply(stale_admission), Err(Error::Stale)));
    assert_state(
        &store,
        &[
            accepted_parent(),
            expected(&child, Shape::Verify, remote, vec![]),
        ],
        &[],
    );
    admit(&child, remote, 1_000);
    admit(&grandchild, local, 1_000);
    assert_state(
        &store,
        &[
            accepted_parent(),
            expected(&child, Shape::Pending, remote, child_roles()),
            expected(&grandchild, Shape::Pending, local, grandchild_roles()),
        ],
        &[],
    );

    let stale_removal =
        membership::removal(&store, &current(&child), &configuration, None).unwrap();
    admit(&replacement, local, 10_000);
    assert!(matches!(store.apply(stale_removal), Err(Error::Stale)));
    let replaced_population = || {
        vec![
            accepted_parent(),
            expected(&replacement, Shape::Pending, local, child_roles()),
            expected(
                &child,
                Shape::HistoryAll,
                recovery,
                vec![(cell(&parent_output), WAIT)],
            ),
            expected(
                &grandchild,
                Shape::HistoryAll,
                recovery,
                vec![(cell(&child_output), WAIT)],
            ),
        ]
    };
    assert_state(
        &store,
        &replaced_population(),
        &[DependencyKey::Cell(child_output.clone())],
    );
    // The child's output is still unavailable; consuming that hint must not revive its descendant.
    wake();
    assert_state(&store, &replaced_population(), &[]);
    store
        .apply(membership::removal(&store, &current(&replacement), &configuration, None).unwrap())
        .unwrap();
    assert_state(
        &store,
        &[
            accepted_parent(),
            expected(
                &child,
                Shape::HistoryAll,
                recovery,
                vec![(cell(&parent_output), WAIT)],
            ),
            expected(
                &grandchild,
                Shape::HistoryAll,
                recovery,
                vec![(cell(&child_output), WAIT)],
            ),
        ],
        &[DependencyKey::Cell(parent_output.clone())],
    );
    wake();
    assert_state(
        &store,
        &[
            accepted_parent(),
            expected(&child, Shape::Resolve, recovery, vec![]),
            expected(
                &grandchild,
                Shape::HistoryAll,
                recovery,
                vec![(cell(&child_output), WAIT)],
            ),
        ],
        &[],
    );
    admit(&child, recovery, 1_000);
    assert_state(
        &store,
        &[
            accepted_parent(),
            expected(&child, Shape::Pending, recovery, child_roles()),
            expected(
                &grandchild,
                Shape::HistoryAll,
                recovery,
                vec![(cell(&child_output), WAIT)],
            ),
        ],
        &[DependencyKey::Cell(child_output.clone())],
    );
    wake();
    admit(&grandchild, recovery, 1_000);
    assert_state(
        &store,
        &[
            accepted_parent(),
            expected(&child, Shape::Pending, recovery, child_roles()),
            expected(&grandchild, Shape::Pending, recovery, grandchild_roles()),
        ],
        &[],
    );

    let block = BlockBuilder::default()
        .transaction(tx(0))
        .transactions([child.clone(), grandchild.clone()])
        .build();
    let snapshot = Arc::new(Snapshot::new(
        block.header(),
        base.total_difficulty().clone(),
        base.epoch_ext().clone(),
        ckb_test_chain_utils::MockStore::default()
            .store()
            .get_snapshot(),
        Default::default(),
        base.cloned_consensus(),
    ));
    {
        let _pause = store.begin_chain().unwrap();
        let command = ChainReorgArgs::Detailed {
            detached_blocks: Default::default(),
            attached_blocks: [block.clone()].into(),
            snapshot,
        };
        store
            .apply(chain::reconcile(&store, &command, &configuration).unwrap())
            .unwrap();
    }
    assert_state(&store, &[accepted_parent()], &[]);
    {
        let _pause = store.begin_chain().unwrap();
        let command = ChainReorgArgs::Detailed {
            detached_blocks: [block].into(),
            attached_blocks: Default::default(),
            snapshot: base,
        };
        store
            .apply(chain::reconcile(&store, &command, &configuration).unwrap())
            .unwrap();
    }
    assert_state(
        &store,
        &[
            accepted_parent(),
            expected(&child, Shape::Resolve, recovery, vec![]),
            expected(&grandchild, Shape::Resolve, recovery, vec![]),
        ],
        &[],
    );
    assert!(current(&child).arrival < current(&grandchild).arrival);
    admit(&child, recovery, 1_000);
    store
        .apply(chain::clear(&store, None, true).unwrap())
        .unwrap();
    assert_state(
        &store,
        &[
            accepted_parent(),
            expected(&child, Shape::Pending, recovery, child_roles()),
        ],
        &[],
    );
    store
        .apply(chain::clear(&store, None, false).unwrap())
        .unwrap();
    assert_state(&store, &[], &[]);
}
