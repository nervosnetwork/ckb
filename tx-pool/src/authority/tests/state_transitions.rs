//! Reconstruct projections from fixture facts after complete owner transitions.
//! The matrix exercises Store directly; planner sequences exercise its producers.
use super::*;
use crate::authority::{
    budget::{Amount, OwnerUsage, owner_amount},
    chain::ClearScope,
    eviction::EvictionRank,
    model::{
        Accepted, Phase, Premises, RecoveryTriggers, RemoteOrigin, Source, Status, VerifiedScripts,
        hash_key,
    },
    tests::common::{
        commit_admission, commit_pairs, config, delete, entry, insert, output_tx, spend,
        store_with_pipeline_limit, verification_fixture,
    },
};
use ckb_network::PeerIndex;
use ckb_types::{
    core::{
        Capacity, TransactionView,
        cell::{CellMeta, ResolvedTransaction},
    },
    packed::{Byte32, OutPoint, ProposalShortId},
};
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    sync::{Arc, Weak},
    time::Instant,
};

#[path = "state_sequences.rs"]
mod sequences;

#[test]
fn invalid_entry_metric_sample_preserves_the_previous_export_and_service_liveness() {
    use crate::authority::tests::{
        common::{accept, store},
        entry_metrics::exported_entries,
    };

    ckb_metrics::METRICS_SERVICE_ENABLED.set(true).unwrap();
    let store = store();
    let hash = accept(&store, output_tx(31_300), 1, 1, Status::Pending);
    store.publish_entry_metrics();
    assert_eq!(exported_entries(), [1, 0, 0]);
    for (gap, proposed) in [(2, 0), (0, 2), (1, 1)] {
        {
            let mut state = store.state_write();
            state.gap_count = gap;
            state.proposed_count = proposed;
        }
        store.publish_entry_metrics();
        assert_eq!(exported_entries(), [1, 0, 0]);
        assert!(!store.is_faulted());
        assert!(!store.is_stopped());
    }
    {
        let mut state = store.state_write();
        state.gap_count = 0;
        state.proposed_count = 0;
    }
    delete(&store, store.point(&hash).1.unwrap()).unwrap();
    store.publish_entry_metrics();
    assert_eq!(exported_entries(), [0, 0, 0]);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

    fn preaccepted(self) -> bool {
        !self.accepted() && !self.history()
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
                origin: RemoteOrigin { peer, deadline },
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
                    remote: Some(RemoteOrigin {
                        peer: 1.into(),
                        deadline: early,
                    }),
                },
                peer: Some(1.into()),
                deadline: Some(early),
            },
        ]
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ExpectedRelation {
    Spender(OutPoint),
    CellDep(OutPoint),
    Child(Byte32),
    Waiter(OutPoint),
}

struct Fixture {
    parent: TransactionView,
    transaction: TransactionView,
    input: OutPoint,
    dependency: OutPoint,
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
        .header_dep(header)
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
                verification_fixture(store, original, 11, 19, Status::Pending).resolved(),
            )),
            Shape::Waiting => Phase::Waiting(BTreeSet::from([self.dependency.clone()])),
            Shape::HistoryAll | Shape::HistoryAny => {
                let keys = BTreeSet::from([self.input.clone(), self.dependency.clone()]);
                Phase::Replaced(if matches!(shape, Shape::HistoryAll) {
                    RecoveryTriggers::BlockedDependencies(keys)
                } else {
                    RecoveryTriggers::RetryInputs(keys)
                })
            }
            Shape::Pending | Shape::Gap | Shape::Proposed => Phase::Accepted(Accepted {
                transaction: Arc::clone(&self.resolved),
                scripts: VerifiedScripts::Fixture(19),
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
        original.with_source_and_phase(origin.source, phase)
    }

    fn relations(&self, shape: Shape) -> Vec<ExpectedRelation> {
        match shape {
            Shape::Resolve | Shape::Verify => Vec::new(),
            Shape::Waiting => vec![ExpectedRelation::Waiter(self.dependency.clone())],
            Shape::HistoryAll | Shape::HistoryAny => vec![
                ExpectedRelation::Waiter(self.input.clone()),
                ExpectedRelation::Waiter(self.dependency.clone()),
            ],
            Shape::Pending | Shape::Gap | Shape::Proposed => vec![
                ExpectedRelation::Spender(self.input.clone()),
                ExpectedRelation::CellDep(self.input.clone()),
                ExpectedRelation::CellDep(self.dependency.clone()),
                ExpectedRelation::Child(self.parent.hash()),
            ],
        }
    }
}

struct Expected {
    owner: Arc<Entry>,
    shape: Shape,
    origin: Origin,
    timestamp: u64,
    relations: Vec<ExpectedRelation>,
}

fn add(total: &mut Amount, amount: Amount) {
    // These small fixture totals do not use the production delta arithmetic.
    total.items += amount.items;
    total.bytes += amount.bytes;
    total.edges += amount.edges;
    total.serialized += amount.serialized;
    total.cycles += amount.cycles;
}

fn assert_state(store: &Store, expected: &[Expected], wakes: &[OutPoint]) {
    let mut usage = OwnerUsage::default();
    let mut owners: BTreeMap<[u8; 32], Arc<Entry>> = BTreeMap::new();
    let mut proposals: BTreeMap<[u8; ProposalShortId::TOTAL_SIZE], [u8; 32]> = BTreeMap::new();
    let mut deadlines: BTreeSet<(Instant, [u8; 32])> = BTreeSet::new();
    let mut accepted_times: BTreeSet<(u64, [u8; 32])> = BTreeSet::new();
    let mut waiting_owners: BTreeSet<[u8; 32]> = BTreeSet::new();
    let mut preaccepted_owners: HashSet<[u8; 32]> = HashSet::new();
    let mut spenders: BTreeMap<OutPoint, [u8; 32]> = BTreeMap::new();
    let mut cell_deps: BTreeMap<OutPoint, BTreeSet<[u8; 32]>> = BTreeMap::new();
    let mut children: BTreeMap<[u8; 32], BTreeSet<[u8; 32]>> = BTreeMap::new();
    let mut waiters: BTreeMap<OutPoint, BTreeSet<[u8; 32]>> = BTreeMap::new();
    let mut peers: BTreeMap<PeerIndex, BTreeSet<[u8; 32]>> = BTreeMap::new();
    let mut eviction_bounds: BTreeSet<EvictionRank> = BTreeSet::new();
    let mut queued = [Vec::new(), Vec::new()];
    let mut gap = 0;
    let mut proposed = 0;
    for expected in expected {
        assert_eq!(
            expected.owner.source, expected.origin.source,
            "state oracle: source identity"
        );
        assert!(
            match (&expected.owner.phase, expected.shape) {
                (Phase::Resolve, Shape::Resolve)
                | (Phase::Verify(_), Shape::Verify)
                | (Phase::Waiting(_), Shape::Waiting)
                | (Phase::Replaced(RecoveryTriggers::BlockedDependencies(_)), Shape::HistoryAll)
                | (Phase::Replaced(RecoveryTriggers::RetryInputs(_)), Shape::HistoryAny) => true,
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
        let hash = expected.owner.hash_bytes();
        owners.insert(hash, Arc::clone(&expected.owner));
        let proposal: [u8; ProposalShortId::TOTAL_SIZE] =
            expected.owner.proposal().as_slice().try_into().unwrap();
        proposals.insert(proposal, hash);
        let amount = owner_amount(&expected.owner);
        if expected.shape.accepted() {
            add(&mut usage.accepted, amount);
            accepted_times.insert((expected.timestamp, hash));
            eviction_bounds.insert(EvictionRank::lower_bound(&expected.owner).unwrap());
        } else {
            add(&mut usage.pipeline, amount);
            if expected.shape.history() {
                add(&mut usage.history, amount);
            } else {
                if let Some(peer) = expected.origin.peer {
                    add(&mut usage.remote, amount);
                    add(usage.peers.entry(peer).or_default(), amount);
                    peers.entry(peer).or_default().insert(hash);
                }
                if let Some(deadline) = expected.origin.deadline {
                    deadlines.insert((deadline, hash));
                }
            }
        }
        if expected.shape.preaccepted() {
            preaccepted_owners.insert(hash);
        }
        match expected.shape {
            Shape::Resolve => queued[0].push(&expected.owner),
            Shape::Verify => queued[1].push(&expected.owner),
            Shape::Waiting => {
                waiting_owners.insert(hash);
            }
            Shape::Gap => gap += 1,
            Shape::Proposed => proposed += 1,
            _ => {}
        }
        for relation in &expected.relations {
            match relation {
                ExpectedRelation::Spender(point) => {
                    assert!(spenders.insert(point.clone(), hash).is_none());
                }
                ExpectedRelation::CellDep(point) => {
                    assert!(cell_deps.entry(point.clone()).or_default().insert(hash));
                }
                ExpectedRelation::Child(parent) => {
                    assert!(children.entry(hash_key(parent)).or_default().insert(hash));
                }
                ExpectedRelation::Waiter(point) => {
                    assert!(waiters.entry(point.clone()).or_default().insert(hash));
                }
            }
        }
    }

    let state = store.state.read();
    assert_eq!(
        state.owners.len(),
        owners.len(),
        "state oracle: owner population"
    );
    for (hash, owner) in &owners {
        assert!(Arc::ptr_eq(state.owners.get(hash).unwrap(), owner));
    }
    let actual_proposals: BTreeMap<[u8; ProposalShortId::TOTAL_SIZE], [u8; 32]> =
        state.proposals.iter().map(|(k, v)| (*k, *v)).collect();
    assert_eq!(actual_proposals, proposals, "state oracle: proposal index");
    assert_eq!(
        state.deadlines, deadlines,
        "state oracle: deadline projection"
    );
    assert_eq!(
        state.accepted_times, accepted_times,
        "state oracle: accepted timestamps"
    );
    assert_eq!(
        state.waiting_owners, waiting_owners,
        "state oracle: waiting owners"
    );
    let actual_preaccepted: HashSet<[u8; 32]> = state.preaccepted_owners.iter().copied().collect();
    assert_eq!(
        actual_preaccepted, preaccepted_owners,
        "state oracle: preaccepted owners"
    );
    assert_eq!(
        (state.gap_count, state.proposed_count),
        (gap, proposed),
        "state oracle: phase counts"
    );
    let actual_spenders: BTreeMap<OutPoint, [u8; 32]> = state
        .spenders
        .iter()
        .map(|(k, v)| (k.to_out_point(), *v))
        .collect();
    assert_eq!(actual_spenders, spenders, "state oracle: spenders");

    let actual_cell_deps: BTreeMap<OutPoint, BTreeSet<[u8; 32]>> = state
        .cell_deps
        .iter()
        .map(|(k, readers)| {
            assert!(!readers.is_empty(), "empty cell_deps row for {k:?}");
            (k.to_out_point(), readers.iter().copied().collect())
        })
        .collect();
    assert_eq!(actual_cell_deps, cell_deps, "state oracle: cell_deps");

    let actual_children: BTreeMap<[u8; 32], BTreeSet<[u8; 32]>> = state
        .children
        .iter()
        .map(|(k, kids)| {
            assert!(!kids.is_empty(), "empty children row for {k:?}");
            (*k, kids.iter().copied().collect())
        })
        .collect();
    assert_eq!(actual_children, children, "state oracle: children");

    let mut actual_waiters: BTreeMap<OutPoint, BTreeSet<[u8; 32]>> = BTreeMap::new();
    let mut actual_wakes: BTreeSet<OutPoint> = BTreeSet::new();
    for (key, row) in &state.waiters {
        assert!(
            !row.is_empty(),
            "retired wait row {key:?} remains allocated"
        );
        let point = key.to_out_point();
        if let Some(wake) = &row.wake {
            assert_eq!(wake.pass, row.next_pass, "state oracle: wake pass");
            assert!(wake.after.is_none());
            assert!(actual_wakes.insert(point.clone()));
        }
        assert!(
            actual_waiters
                .insert(point, row.waiters.keys().copied().collect())
                .is_none()
        );
    }
    assert_eq!(actual_waiters, waiters, "state oracle: waiters");
    assert_eq!(
        actual_wakes,
        wakes.iter().cloned().collect(),
        "state oracle: wake population"
    );
    let actual_dirty: BTreeSet<OutPoint> = state
        .dirty_wakes
        .iter()
        .map(|key| key.to_out_point())
        .collect();
    assert_eq!(actual_dirty, actual_wakes, "state oracle: dirty wakes");

    let actual_peers: BTreeMap<PeerIndex, BTreeSet<[u8; 32]>> = state
        .peer_members
        .iter()
        .map(|(peer, members)| {
            assert!(!members.is_empty(), "empty peer_members row for {peer:?}");
            (*peer, members.iter().copied().collect())
        })
        .collect();
    assert_eq!(actual_peers, peers, "state oracle: peer population");
    assert_eq!(
        state.eviction_bounds, eviction_bounds,
        "state oracle: eviction bounds"
    );
    let empty_excluded = HashSet::new();
    let mut expected_descendants = BTreeMap::new();
    for (&hash, owner) in &owners {
        if owner.accepted().is_some() && children.contains_key(&hash) {
            let closure = state.descendants_unbounded(&hash, &empty_excluded).unwrap();
            let mut total = crate::authority::membership::Aggregate::default();
            for member in closure {
                let acc = owners.get(&member).unwrap().accepted().unwrap();
                total = total
                    .add(crate::authority::membership::Aggregate::one(acc))
                    .unwrap();
            }
            expected_descendants.insert(hash, total);
        }
    }
    let actual_descendants: BTreeMap<[u8; 32], crate::authority::membership::Aggregate> = state
        .descendants
        .iter()
        .map(|(&hash, &agg)| (hash, agg))
        .collect();
    assert_eq!(
        actual_descendants, expected_descendants,
        "state oracle: incremental descendant aggregates"
    );
    drop(state);

    for (hash, owner) in &owners {
        assert!(Arc::ptr_eq(
            &store.point(&Byte32::new(*hash)).1.unwrap(),
            owner
        ));
    }
    let proposal_ids: Vec<_> = proposals
        .keys()
        .map(|id| ProposalShortId::new(*id))
        .collect();
    let (_, live, _) = store.compact_lookup(&proposal_ids);
    let actual_compact: BTreeMap<[u8; ProposalShortId::TOTAL_SIZE], [u8; 32]> = live
        .into_iter()
        .map(|(id, entry)| (id.unpack(), entry.hash_bytes()))
        .collect();
    assert_eq!(actual_compact, proposals, "state oracle: proposal lookup");

    let actual_queues = store.queues.queued_owners();
    assert_eq!(
        store.queues.queued_len(),
        queued.iter().map(Vec::len).sum::<usize>(),
        "state oracle: queue population"
    );
    for (actual, expected) in actual_queues.iter().zip(queued) {
        let mut actual: Vec<_> = actual.iter().map(Weak::as_ptr).collect();
        let mut expected: Vec<_> = expected.into_iter().map(Arc::as_ptr).collect();
        actual.sort_unstable();
        expected.sort_unstable();
        assert_eq!(actual, expected, "state oracle: queue owner identities");
    }
    assert_eq!(store.owner_usage(), usage, "state oracle: owner accounting");
    assert!(!store.is_faulted());
}

impl Store {
    /// Reuse the complete projection/account oracle at a quiescent service cut.
    pub(in crate::authority) fn assert_empty_for_test(&self) {
        assert_state(self, &[], &[]);
    }
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
        scripts: VerifiedScripts::Fixture(1),
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
        relations: Vec::new(),
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
                                relations: fixture.relations(before_shape),
                            },
                        ],
                        &[],
                    );
                    let view = store.snapshot().0;
                    let mut premises = Premises::default();
                    store.get(&before.hash(), &mut premises).unwrap();
                    let accepted_rev_before = store.state.read().accepted_revision;
                    let after = fixture.owner(&store, &before, after_shape, after_origin, 29);
                    commit_pairs(
                        &store,
                        vec![(Some(Arc::clone(&before)), Some(Arc::clone(&after)))],
                    )
                    .unwrap();
                    let current = || Expected {
                        owner: Arc::clone(&after),
                        shape: after_shape,
                        origin: after_origin,
                        timestamp: 29,
                        relations: fixture.relations(after_shape),
                    };
                    let wakes =
                        if before_shape.accepted() && matches!(after_shape, Shape::HistoryAll) {
                            vec![fixture.input.clone()]
                        } else {
                            Vec::new()
                        };
                    assert_state(&store, &[base(), current()], &wakes);
                    assert!(
                        matches!(
                            commit_pairs(
                                &store,
                                vec![(Some(Arc::clone(&before)), Some(Arc::clone(&after)))],
                            ),
                            Err(Error::Stale)
                        ),
                        "{before_shape:?}/{before_origin:?} -> {after_shape:?}/{after_origin:?}"
                    );
                    assert!(matches!(
                        store.check_premises(view, &premises),
                        Err(Error::Stale)
                    ));
                    let accepted_rev_after = store.state.read().accepted_revision;
                    if before_shape.accepted() || after_shape.accepted() {
                        assert!(accepted_rev_after > accepted_rev_before);
                    } else {
                        assert_eq!(accepted_rev_after, accepted_rev_before);
                    }
                    assert_state(&store, &[base(), current()], &wakes);
                    delete(&store, after).unwrap();
                    assert_state(&store, &[base()], &[]);
                }
            }
        }
    }
    delete(&store, parent).unwrap();
    assert_state(&store, &[], &[]);
}

#[test]
fn admission_replacement_recovery_reorg_and_clear_preserve_the_whole_population() {
    use crate::authority::tests::common::tx;
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
    let child_relations = || {
        vec![
            ExpectedRelation::Spender(parent_output.clone()),
            ExpectedRelation::Child(parent.hash()),
        ]
    };
    let grandchild_relations = || {
        vec![
            ExpectedRelation::Spender(child_output.clone()),
            ExpectedRelation::Child(child.hash()),
        ]
    };
    let current = |transaction: &TransactionView| store.point(&transaction.hash()).1.unwrap();
    let expected = |transaction: &TransactionView, shape, origin, relations| Expected {
        owner: current(transaction),
        shape,
        origin,
        timestamp: 0,
        relations,
    };
    let accepted_parent = || expected(&parent, Shape::Pending, local, vec![]);
    let admit = |transaction: &TransactionView, origin: Origin, fee| {
        let owner = store
            .point(&transaction.hash())
            .1
            .unwrap_or_else(|| entry(&store, transaction.clone(), origin.source));
        assert_eq!(owner.source, origin.source);
        let (_, reject) =
            commit_admission(&store, &owner, fee, 19, Status::Pending, &configuration).unwrap();
        assert!(reject.is_none(), "fixture admission rejected: {reject:?}");
    };
    let wake = || {
        waiting::commit_wake(&store, &mut None)
            .unwrap()
            .expect("a recorded availability event has work");
    };

    insert(&store, entry(&store, parent.clone(), local.source));
    insert(&store, entry(&store, child.clone(), remote.source));
    let child_wait =
        Resolution::Waiting(BTreeSet::from([parent_output.clone()]), Premises::default());
    ingress::commit_resolution(&store, store.snapshot().0, &current(&child), &child_wait).unwrap();
    assert_state(
        &store,
        &[
            expected(&parent, Shape::Resolve, local, vec![]),
            expected(
                &child,
                Shape::Waiting,
                remote,
                vec![ExpectedRelation::Waiter(parent_output.clone())],
            ),
        ],
        &[],
    );

    let resolved = verification_fixture(&store, &current(&parent), 1_000, 19, Status::Pending);
    ingress::commit_resolution(
        &store,
        store.snapshot().0,
        &current(&parent),
        &Resolution::Ready(Arc::clone(resolved.resolved())),
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
                vec![ExpectedRelation::Waiter(parent_output.clone())],
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
                vec![ExpectedRelation::Waiter(parent_output.clone())],
            ),
        ],
        std::slice::from_ref(&parent_output),
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

    let stale_child = current(&child);
    let stale_verified = verification_fixture(&store, &stale_child, 1_000, 19, Status::Pending);
    let resolved = verification_fixture(&store, &stale_child, 1_000, 19, Status::Pending);
    ingress::commit_resolution(
        &store,
        store.snapshot().0,
        &stale_child,
        &Resolution::Ready(Arc::clone(resolved.resolved())),
    )
    .unwrap();
    assert!(matches!(
        membership::commit_admission(
            &store,
            &stale_child,
            Some(Arc::clone(&stale_child)),
            &stale_verified,
            &configuration,
            membership::AdmissionOptions::commit(),
        ),
        Err(Error::Stale)
    ));
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
            expected(&child, Shape::Pending, remote, child_relations()),
            expected(&grandchild, Shape::Pending, local, grandchild_relations()),
        ],
        &[],
    );

    let stale_child_accepted = current(&child);
    admit(&replacement, local, 10_000);
    assert!(matches!(
        membership::commit_removal(&store, &stale_child_accepted, &configuration, None),
        Err(Error::Stale)
    ));
    let replaced_population = || {
        vec![
            accepted_parent(),
            expected(&replacement, Shape::Pending, local, child_relations()),
            expected(
                &child,
                Shape::HistoryAll,
                recovery,
                vec![ExpectedRelation::Waiter(parent_output.clone())],
            ),
            expected(
                &grandchild,
                Shape::HistoryAll,
                recovery,
                vec![ExpectedRelation::Waiter(child_output.clone())],
            ),
        ]
    };
    assert_state(
        &store,
        &replaced_population(),
        std::slice::from_ref(&child_output),
    );
    wake();
    assert_state(&store, &replaced_population(), &[]);
    membership::commit_removal(&store, &current(&replacement), &configuration, None).unwrap();
    assert_state(
        &store,
        &[
            accepted_parent(),
            expected(
                &child,
                Shape::HistoryAll,
                recovery,
                vec![ExpectedRelation::Waiter(parent_output.clone())],
            ),
            expected(
                &grandchild,
                Shape::HistoryAll,
                recovery,
                vec![ExpectedRelation::Waiter(child_output.clone())],
            ),
        ],
        std::slice::from_ref(&parent_output),
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
                vec![ExpectedRelation::Waiter(child_output.clone())],
            ),
        ],
        &[],
    );
    admit(&child, recovery, 1_000);
    assert_state(
        &store,
        &[
            accepted_parent(),
            expected(&child, Shape::Pending, recovery, child_relations()),
            expected(
                &grandchild,
                Shape::HistoryAll,
                recovery,
                vec![ExpectedRelation::Waiter(child_output.clone())],
            ),
        ],
        std::slice::from_ref(&child_output),
    );
    wake();
    admit(&grandchild, recovery, 1_000);
    assert_state(
        &store,
        &[
            accepted_parent(),
            expected(&child, Shape::Pending, recovery, child_relations()),
            expected(
                &grandchild,
                Shape::Pending,
                recovery,
                grandchild_relations(),
            ),
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
    let command = ChainReorgArgs::for_test(Default::default(), [block.clone()].into(), snapshot);
    chain::commit_reconcile(&store, &command, &configuration).unwrap();
    assert_state(&store, &[accepted_parent()], &[]);
    let command = ChainReorgArgs::for_test([block].into(), Default::default(), base);
    chain::commit_reconcile(&store, &command, &configuration).unwrap();
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
    chain::commit_clear(&store, None, ClearScope::Unaccepted).unwrap();
    assert_state(
        &store,
        &[
            accepted_parent(),
            expected(&child, Shape::Pending, recovery, child_relations()),
        ],
        &[],
    );
    chain::commit_clear(&store, None, ClearScope::All).unwrap();
    assert_state(&store, &[], &[]);
}

#[test]
fn accepted_readers_deduplicates_spender_and_multiple_cell_dep_readers() {
    use crate::authority::model::OutPointKey;
    use crate::authority::tests::common::{accept, store};

    let store = store();
    let shared_parent = accept(&store, output_tx(50_100), 10, 1, Status::Pending);
    let shared_point = OutPoint::new(shared_parent, 0);
    let key = OutPointKey::from_out_point(&shared_point);

    // 1. Multiple cell-dep readers with no spender (`ReaderSet::One` -> `Many`).
    let mut dep_readers = Vec::new();
    for i in 0..6u32 {
        let reader = accept(
            &store,
            spend(50_110 + i, &[], std::slice::from_ref(&shared_point)),
            10,
            1,
            Status::Pending,
        );
        dep_readers.push(hash_key(&reader));
    }
    let mut out = Vec::new();
    store.state_read().accepted_readers(&key, &mut out);
    assert_eq!(out.len(), dep_readers.len());
    assert_eq!(
        out.iter().copied().collect::<BTreeSet<_>>(),
        dep_readers.iter().copied().collect::<BTreeSet<_>>()
    );

    // 2. Spender that is distinct from all cell-dep readers.
    let pure_spender = accept(
        &store,
        spend(50_120, std::slice::from_ref(&shared_point), &[]),
        10,
        1,
        Status::Pending,
    );
    let pure_spender_key = hash_key(&pure_spender);
    let prefix_sentinel = [0xAA; 32];
    let mut out = vec![prefix_sentinel];
    store.state_read().accepted_readers(&key, &mut out);
    assert_eq!(out[0], prefix_sentinel);
    assert_eq!(out.len(), 1 + dep_readers.len() + 1);
    let mut expected: BTreeSet<[u8; 32]> = dep_readers.iter().copied().collect();
    expected.insert(pure_spender_key);
    assert_eq!(out[1..].iter().copied().collect::<BTreeSet<_>>(), expected);

    // 3. Remove pure_spender and add a transaction that is BOTH spender AND cell-dep reader.
    delete(&store, store.point(&pure_spender).1.unwrap()).unwrap();
    let dual = accept(
        &store,
        spend(
            50_121,
            std::slice::from_ref(&shared_point),
            std::slice::from_ref(&shared_point),
        ),
        10,
        1,
        Status::Pending,
    );
    let dual_key = hash_key(&dual);
    let mut out = vec![prefix_sentinel];
    store.state_read().accepted_readers(&key, &mut out);
    assert_eq!(out[0], prefix_sentinel);
    assert_eq!(
        out.len(),
        1 + dep_readers.len() + 1,
        "spender that is also a cell-dep reader must appear exactly once"
    );
    let mut expected: BTreeSet<[u8; 32]> = dep_readers.iter().copied().collect();
    expected.insert(dual_key);
    assert_eq!(out[1..].iter().copied().collect::<BTreeSet<_>>(), expected);
}
