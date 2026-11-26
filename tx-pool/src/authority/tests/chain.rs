use super::*;
use crate::authority::{
    ingress,
    model::{Premises, RecoveryTriggers, Resolved, Status},
    tests::common::*,
};
use crate::callback::CallbackEvent;
use ckb_proposal_table::ProposalView;
use ckb_test_chain_utils::MockStore;
use ckb_types::{
    bytes::Bytes,
    core::{
        BlockBuilder, BlockView, Capacity, DepType,
        cell::{CellMeta, ResolvedTransaction},
    },
    packed::{Byte32, CellDep, CellInput, OutPoint, ProposalShortId},
};
use std::collections::{HashSet, VecDeque};

fn snapshot(
    base: &Snapshot,
    tip: &BlockView,
    gap: HashSet<ProposalShortId>,
    proposed: HashSet<ProposalShortId>,
) -> Arc<Snapshot> {
    Arc::new(Snapshot::new(
        tip.header(),
        base.total_difficulty().clone(),
        base.epoch_ext().clone(),
        MockStore::default().store().get_snapshot(),
        ProposalView::new(gap, proposed),
        base.cloned_consensus(),
    ))
}

fn command(store: &Store, transactions: Vec<ckb_types::core::TransactionView>) -> ChainReorgArgs {
    let base = store.snapshot().1;
    let block = BlockBuilder::default()
        .transaction(tx(0))
        .transactions(transactions)
        .build();
    let snapshot = snapshot(&base, &block, HashSet::new(), HashSet::new());
    ChainReorgArgs::for_test(VecDeque::new(), [block].into(), snapshot)
}

fn apply(store: &Store, command: &ChainReorgArgs) -> Option<Arc<Batch>> {
    commit_reconcile(store, command, &config()).unwrap()
}

fn accepted_hashes(store: &Store) -> BTreeSet<Byte32> {
    store
        .capture_accepted()
        .owners
        .into_iter()
        .map(|entry| entry.hash())
        .collect()
}

#[test]
fn clear_pipeline_preserves_accepted_membership_but_invalidates_old_view() {
    let store = store();
    let accepted = accept(&store, output_tx(5003), 1, 1, Status::Pending);
    let waiting = entry(&store, tx(5004), Source::Recovery);
    insert(&store, Arc::clone(&waiting));
    let old = store.snapshot().0;
    commit_clear(&store, None, ClearScope::Unaccepted).unwrap();
    assert_eq!(accepted_hashes(&store), [accepted].into());
    assert!(store.point(&waiting.hash()).1.is_none());
    assert!(store.snapshot().0 > old);
}

#[test]
fn retained_replacement_history_survives_age_and_unrelated_chain_until_wake_or_clear() {
    for wake_before_clear in [false, true] {
        let store = store();
        let configuration = TxPoolConfig {
            min_rbf_rate: ckb_types::core::FeeRate::from_u64(1000),
            ..config()
        };
        let parent = accept(&store, output_tx(6400), 1, 1, Status::Pending);
        let point = OutPoint::new(parent.clone(), 0);
        let old = accept(
            &store,
            spend(6401, std::slice::from_ref(&point), &[]),
            1000,
            1,
            Status::Pending,
        );
        let replacement = entry(&store, spend(6402, &[point], &[]), Source::Local);
        let (_, reject) = commit_admission(
            &store,
            &replacement,
            10_000,
            1,
            Status::Pending,
            &configuration,
        )
        .unwrap();
        assert!(reject.is_none());
        let history = store.point(&old).1.unwrap();
        assert_eq!(history.source, Source::Recovery);
        assert!(matches!(history.phase, Phase::Replaced(_)));
        let expired = store.expired(std::time::Instant::now(), u64::MAX, 32);
        assert!(expired.iter().any(|entry| entry.hash() == parent));
        assert!(!expired.iter().any(|entry| entry.hash() == old));
        apply(&store, &command(&store, Vec::new()));
        assert!(Arc::ptr_eq(&store.point(&old).1.unwrap(), &history));

        if wake_before_clear {
            let current = store.point(&replacement.hash()).1.unwrap();
            membership::commit_removal(&store, &current, &configuration, None).unwrap();
            let mut cursor = None;
            assert!(
                crate::authority::waiting::commit_wake(&store, &mut cursor)
                    .unwrap()
                    .is_some()
            );
            let recovered = store.point(&old).1.unwrap();
            assert_eq!(recovered.source, Source::Recovery);
            assert!(matches!(recovered.phase, Phase::Resolve));
            assert!(recovered.accepted().is_none());
        }
        commit_clear(&store, None, ClearScope::Unaccepted).unwrap();
        assert!(store.point(&old).1.is_none());
        assert!(store.point(&parent).1.unwrap().accepted().is_some());
        assert!(!store.is_faulted());
    }
}

#[test]
fn mismatched_attached_tip_is_rejected_before_any_mutation() {
    let store = store();
    let hash = accept(&store, output_tx(5005), 1, 1, Status::Pending);
    let command = ChainReorgArgs::for_test(
        VecDeque::new(),
        [BlockBuilder::default().build()].into(),
        store.snapshot().1,
    );
    assert!(matches!(
        commit_reconcile(&store, &command, &config()),
        Err(Error::Fault("chain snapshot tip"))
    ));
    assert_eq!(accepted_hashes(&store), [hash].into());
}

#[test]
fn committed_parent_leaves_child_backed_by_chain_and_updates_compact_lookup() {
    let store = store();
    let parent = output_tx(5006);
    let ph = accept(&store, parent.clone(), 1, 1, Status::Pending);
    let child = spend(5007, &[OutPoint::new(ph.clone(), 0)], &[]);
    let ch = accept(&store, child, 1, 1, Status::Pending);
    apply(&store, &command(&store, vec![parent.clone()]));
    assert_eq!(accepted_hashes(&store), [ch.clone()].into());
    assert!(
        store
            .point(&ch)
            .1
            .unwrap()
            .accepted()
            .unwrap()
            .parents
            .is_empty()
    );
    let (_, _, committed) = store.compact_lookup(&[parent.proposal_short_id()]);
    assert_eq!(committed, vec![(parent.proposal_short_id(), ph.unpack())]);
    assert_eq!(
        store.filter_fresh_proposals(vec![parent.proposal_short_id()]),
        vec![parent.proposal_short_id()],
        "the compact-block cache does not suppress fresh proposal requests"
    );
}

#[test]
fn committed_network_work_settles_its_origin_without_repeating_accepted_relay() {
    let store = store();
    let source = remote(81, 1);
    let queued = entry(&store, output_tx(5010), source);
    let proposal = entry(&store, output_tx(5011), source.promote_remote());
    let local = entry(&store, output_tx(5012), Source::Local);
    for owner in [&queued, &proposal, &local] {
        insert(&store, Arc::clone(owner));
    }
    let accepted = entry(&store, output_tx(5013), source);
    let (accepted_batch, reject) =
        commit_admission(&store, &accepted, 1, 1, Status::Pending, &config()).unwrap();
    assert!(reject.is_none());
    assert!(
        accepted_batch
            .unwrap()
            .effects()
            .iter()
            .any(|effect| matches!(
                effect.relay_result(),
                Some(TxVerificationResult::Ok { original_peer: Some(peer), tx_hash })
                    if *peer == 81.into() && *tx_hash == accepted.hash()
            ))
    );

    let attached = command(
        &store,
        [&queued, &proposal, &local, &accepted]
            .into_iter()
            .map(|owner| owner.transaction.as_ref().clone())
            .collect(),
    );
    let batch = commit_reconcile(&store, &attached, &config())
        .unwrap()
        .unwrap();
    let mut relayed = batch
        .effects()
        .iter()
        .filter_map(Effect::relay_result)
        .map(|result| match result {
            TxVerificationResult::Ok {
                original_peer,
                tx_hash,
            } => (tx_hash.clone(), *original_peer),
            unexpected => panic!("unexpected relay outcome: {unexpected:?}"),
        })
        .collect::<Vec<_>>();
    relayed.sort_unstable();
    let mut expected = vec![
        (queued.hash(), Some(81.into())),
        (proposal.hash(), Some(81.into())),
    ];
    expected.sort_unstable();
    assert_eq!(relayed, expected);
    for owner in [&queued, &proposal, &local, &accepted] {
        assert!(store.point(&owner.hash()).1.is_none());
    }
}

#[test]
fn detached_committed_producer_invalidates_its_pool_resolved_readers_before_recovery() {
    for dependency in [false, true] {
        let store = store();
        let original_snapshot = store.snapshot().1;
        let parent = output_tx(6530);
        let parent_hash = accept(&store, parent.clone(), 1, 1, Status::Pending);
        let point = OutPoint::new(parent_hash.clone(), 0);
        let reader = if dependency {
            spend(6531, &[], &[point])
        } else {
            spend(6531, &[point], &[])
        };
        let reader_hash = accept(&store, reader, 1, 1, Status::Pending);
        let child_hash = accept(
            &store,
            spend(6532, &[OutPoint::new(reader_hash.clone(), 0)], &[]),
            1,
            1,
            Status::Pending,
        );
        let unrelated = accept(&store, output_tx(6533), 1, 1, Status::Pending);
        let attached = command(&store, vec![parent]);
        apply(&store, &attached);
        let reader = store.point(&reader_hash).1.unwrap();
        let accepted = reader.accepted().unwrap();
        assert!(accepted.parents.is_empty());
        assert!(!accepted.context_sensitive);
        assert!(
            accepted
                .transaction
                .resolved_inputs
                .iter()
                .chain(&accepted.transaction.resolved_cell_deps)
                .all(|cell| cell.transaction_info.is_none())
        );
        let attached_blocks = attached.into_fork().unwrap().attached_blocks;
        apply(
            &store,
            &ChainReorgArgs::for_test(attached_blocks, VecDeque::new(), original_snapshot),
        );
        // Observe the committed reorg before any recovery worker can admit the
        // producer again and repair its readers' parent links as a side effect.
        for hash in [parent_hash, reader_hash, child_hash] {
            let owner = store.point(&hash).1.unwrap();
            assert!(
                matches!(owner.phase, Phase::Resolve),
                "stale backing: {hash}"
            );
            assert_eq!(owner.source, Source::Recovery);
        }
        assert_eq!(accepted_hashes(&store), [unrelated].into());
    }
}

#[test]
fn producer_reattached_on_the_new_chain_keeps_its_pool_resolved_reader_accepted() {
    let store = store();
    let parent = output_tx(6534);
    let parent_hash = accept(&store, parent.clone(), 1, 1, Status::Pending);
    let child_hash = accept(
        &store,
        spend(6535, &[OutPoint::new(parent_hash.clone(), 0)], &[]),
        1,
        1,
        Status::Pending,
    );
    let attached = command(&store, vec![parent.clone()]);
    apply(&store, &attached);
    let child = store.point(&child_hash).1.unwrap();
    let detached_blocks = attached.into_fork().unwrap().attached_blocks;
    let replacement = BlockBuilder::default()
        .timestamp(1)
        .transaction(tx(0))
        .transaction(parent)
        .build();
    let snapshot = snapshot(
        &store.snapshot().1,
        &replacement,
        HashSet::new(),
        HashSet::new(),
    );
    apply(
        &store,
        &ChainReorgArgs::for_test(detached_blocks, [replacement].into(), snapshot),
    );
    assert!(store.point(&parent_hash).1.is_none());
    assert!(Arc::ptr_eq(&store.point(&child_hash).1.unwrap(), &child));
    assert_eq!(accepted_hashes(&store), [child_hash].into());
}

#[test]
fn chain_commit_preserves_supplied_order_for_colliding_compact_ids() {
    let store = store();
    let first = Byte32::new([9; 32]);
    let mut bytes = [9; 32];
    bytes[31] = 7;
    let id = ProposalShortId::from_tx_hash(&first);
    let command = command(
        &store,
        vec![
            output_tx(1).fake_hash(first),
            output_tx(2).fake_hash(Byte32::new(bytes)),
        ],
    );
    apply(&store, &command);
    let (_, live, committed) = store.compact_lookup(&[id.clone(), tx(0).proposal_short_id()]);
    assert!(live.is_empty());
    assert_eq!(committed, vec![(id, bytes)]);
}

#[test]
fn compact_query_rejects_an_indexed_transaction_with_only_a_matching_prefix() {
    use ckb_store::ChainStore;

    let store = store();
    let base = store.snapshot().1;
    let transaction = output_tx(5601);
    let hash = transaction.hash();
    let id = transaction.proposal_short_id();
    let mut collision: [u8; 32] = hash.unpack();
    collision[31] ^= 1;
    let attached = BlockBuilder::default()
        .transaction(tx(0))
        .transaction(transaction)
        .build();
    // Index the attached hash at a position that contains a different transaction.
    let body = BlockBuilder::default()
        .transaction(tx(0))
        .transaction(output_tx(5602).fake_hash(Byte32::new(collision)))
        .build()
        .fake_hash(attached.hash());
    let database = MockStore::default();
    let write = database.store().begin_transaction();
    write.insert_block(&body).unwrap();
    write.attach_block(&attached).unwrap();
    write.commit().unwrap();
    let next = Arc::new(Snapshot::new(
        attached.header(),
        base.total_difficulty().clone(),
        base.epoch_ext().clone(),
        database.store().get_snapshot(),
        Default::default(),
        base.cloned_consensus(),
    ));
    let indexed = next.get_transaction(&hash).unwrap().0;
    assert_eq!(indexed.proposal_short_id(), id);
    assert_ne!(indexed.hash(), hash);
    apply(
        &store,
        &ChainReorgArgs::for_test(VecDeque::new(), [attached].into(), next),
    );
    assert!(crate::authority::query::compact_transactions(&store, &[id]).is_empty());
}

#[test]
fn bounded_recovery_committed_lookup_survives_partial_clear_only() {
    let store = store();
    let base = store.snapshot().1;
    let committed_tx = output_tx(5600);
    let attached = BlockBuilder::default()
        .transactions([tx(0), committed_tx.clone()])
        .build();
    let snapshot = snapshot(&base, &attached, HashSet::new(), HashSet::new());
    let command = ChainReorgArgs::for_test(VecDeque::new(), [attached].into(), snapshot);
    commit_recover_bounded(&store, &command).unwrap();

    let id = committed_tx.proposal_short_id();
    let committed = vec![(id.clone(), committed_tx.hash_ref().unpack())];
    assert_eq!(store.compact_lookup(std::slice::from_ref(&id)).2, committed);
    commit_clear(&store, None, ClearScope::Unaccepted).unwrap();
    assert_eq!(store.compact_lookup(std::slice::from_ref(&id)).2, committed);
    commit_clear(&store, None, ClearScope::All).unwrap();
    assert!(store.compact_lookup(std::slice::from_ref(&id)).2.is_empty());
}

#[test]
fn committed_spend_closes_input_conflict_its_dep_reader_and_descendants_atomically() {
    let store = store();
    let input = OutPoint::new(tx(5999).hash(), 0);
    let provider = spend(5008, std::slice::from_ref(&input), &[]);
    let ph = accept(&store, provider, 1, 1, Status::Pending);
    let reader = spend(5009, &[], &[OutPoint::new(ph, 0)]);
    let rh = accept(&store, reader, 1, 1, Status::Pending);
    accept(
        &store,
        spend(5010, &[OutPoint::new(rh, 0)], &[]),
        1,
        1,
        Status::Pending,
    );
    let survivor = accept(&store, output_tx(5011), 1, 1, Status::Pending);
    apply(&store, &command(&store, vec![spend(5012, &[input], &[])]));
    assert_eq!(accepted_hashes(&store), [survivor].into());
    assert!(!store.is_faulted());
}

#[test]
fn committed_spend_closes_waiter_even_when_its_wait_keys_name_another_missing_parent() {
    let store = store();
    let input = OutPoint::new(tx(5998).hash(), 0);
    let candidate = entry(
        &store,
        spend(5013, std::slice::from_ref(&input), &[]),
        Source::Recovery,
    );
    insert(&store, Arc::clone(&candidate));
    let waiting = replace(
        &store,
        candidate,
        Phase::Waiting([OutPoint::new(tx(5997).hash(), 0)].into()),
    );
    apply(&store, &command(&store, vec![spend(5014, &[input], &[])]));
    assert!(store.point(&waiting.hash()).1.is_none());
}

#[test]
fn chain_conflicts_preserve_declared_order_and_extra_preaccepted_dependencies() {
    let mut points = [5980, 5981, 5982, 5983].map(|nonce| OutPoint::new(tx(nonce).hash(), 0));
    points.sort();
    let [expanded, input, group, unrelated] = &points;
    // Even though the expanded member sorts first, a declared conflict wins.
    for (spent, expected) in [
        (
            vec![group.clone(), expanded.clone(), input.clone()],
            [Some(input), Some(input), Some(input), None],
        ),
        (
            vec![expanded.clone()],
            [None, Some(expanded), Some(expanded), None],
        ),
        (vec![unrelated.clone()], [None, None, None, None]),
    ] {
        let store = store();
        let owner = |nonce| {
            let transaction = spend(nonce, std::slice::from_ref(input), &[])
                .as_advanced_builder()
                .cell_dep(
                    CellDep::new_builder()
                        .out_point(group.clone())
                        .dep_type(DepType::DepGroup)
                        .build(),
                )
                .build();
            entry(&store, transaction, Source::Recovery)
        };
        let resolve = owner(5017);
        let verify = owner(5018);
        let cell = |point: &OutPoint| CellMeta {
            out_point: point.clone(),
            ..CellMeta::default()
        };
        let verify = verify.with_phase(Phase::Verify(Arc::new(Resolved {
            transaction: Arc::new(ResolvedTransaction {
                transaction: verify.transaction.as_ref().clone(),
                resolved_inputs: vec![cell(input)],
                resolved_cell_deps: vec![cell(expanded)],
                resolved_dep_groups: vec![cell(group)],
            }),
            fee: Capacity::shannons(1),
            view: store.snapshot().0,
            premises: Premises::default(),
            pool_cells: BTreeSet::new(),
        })));
        let keys = BTreeSet::from([expanded.clone()]);
        let waiting = owner(5019).with_phase(Phase::Waiting(keys.clone()));
        let history = owner(5025).with_phase(Phase::Replaced(RecoveryTriggers::RetryInputs(keys)));
        let owners = [resolve, verify, waiting, history];
        for owner in &owners {
            insert(&store, Arc::clone(owner));
        }
        let command = command(&store, vec![spend(5026, &spent, &[])]);
        let fork = command.fork().unwrap();
        let facts = ChainFacts::from_blocks(&fork.detached_blocks, &fork.attached_blocks).unwrap();
        for (owner, expected_point) in owners.iter().zip(expected) {
            let detected = if owner.preaccepted() {
                owner.find_spent_preaccepted(&facts.spent)
            } else {
                None
            };
            assert_eq!(detected.as_ref(), expected_point);
        }
        apply(&store, &command);
        for (owner, conflict) in owners.iter().zip(expected) {
            let current = store.point(&owner.hash()).1;
            if conflict.is_some() {
                assert!(current.is_none());
            } else if matches!(owner.phase, Phase::Verify(_)) {
                let current = current.unwrap();
                assert!(matches!(current.phase, Phase::Resolve));
                assert!(Arc::ptr_eq(&current.transaction, &owner.transaction));
                assert_eq!(current.arrival, owner.arrival);
                assert_eq!(current.source, owner.source);
            } else {
                assert!(Arc::ptr_eq(&current.unwrap(), owner));
            }
        }
        assert!(!store.is_faulted());
    }
}

#[test]
fn chain_conflict_reports_first_input_before_resolved_dependencies() {
    let points: Vec<_> = (5991..5995)
        .map(|nonce| OutPoint::new(tx(nonce).hash(), 0))
        .collect();
    for extra_spent in [0, 8] {
        for first in 0..points.len() {
            let store = store();
            let hash = accept(
                &store,
                spend(5020, &points[..2], &points[2..]),
                1,
                1,
                Status::Pending,
            );
            let child = accept(
                &store,
                spend(5021, &[OutPoint::new(hash.clone(), 0)], &[]),
                1,
                1,
                Status::Pending,
            );
            let survivors: BTreeSet<_> = (5022..5024)
                .map(|nonce| accept(&store, output_tx(nonce), 1, 1, Status::Pending))
                .collect();
            let mut spent: Vec<_> = points[first..].iter().rev().cloned().collect();
            spent.extend((0..extra_spent).map(|nonce| OutPoint::new(tx(5900 + nonce).hash(), 0)));
            let batch = commit_reconcile(
                &store,
                &command(&store, vec![spend(5024, &spent, &[])]),
                &config(),
            )
            .unwrap()
            .unwrap();
            let rejected: BTreeSet<_> = batch
                .effects()
                .iter()
                .filter_map(|effect| match effect.callback() {
                    Some(CallbackEvent::Reject(entry, reason)) => {
                        assert!(matches!(
                            reason,
                            Reject::Resolve(OutPointError::Dead(point)) if *point == points[first]
                        ));
                        Some(entry.transaction.hash())
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(rejected, [hash, child].into());
            assert_eq!(accepted_hashes(&store), survivors);
        }
    }
}

#[test]
fn reorg_keeps_survivors_when_detached_hashes_straddle_old_owners() {
    let store = store();
    let mut transactions: Vec<_> = (6900..6907).map(output_tx).collect();
    transactions.sort_by_key(|transaction| transaction.hash());
    for index in [1, 3, 5] {
        accept(&store, transactions[index].clone(), 1, 1, Status::Pending);
    }
    let survivors = [1, 5].map(|index| store.point(&transactions[index].hash()).1.unwrap());
    let restored = [0, 2, 4, 6].map(|index| transactions[index].clone());
    let detached = BlockBuilder::default()
        .transaction(tx(0))
        .transactions(restored.clone())
        .build();
    let attached = BlockBuilder::default()
        .transaction(tx(0))
        .transaction(transactions[3].clone())
        .build();
    let next = snapshot(
        &store.snapshot().1,
        &attached,
        HashSet::new(),
        HashSet::new(),
    );
    apply(
        &store,
        &ChainReorgArgs::for_test([detached].into(), [attached].into(), next),
    );
    assert_eq!(store.capture_all().owners.len(), 6);
    assert!(store.point(&transactions[3].hash()).1.is_none());
    for entry in survivors {
        assert!(Arc::ptr_eq(&store.point(&entry.hash()).1.unwrap(), &entry));
    }
    for transaction in restored {
        let entry = store.point(&transaction.hash()).1.unwrap();
        assert_eq!(entry.source, Source::Recovery);
        assert!(matches!(entry.phase, Phase::Resolve));
    }
}

#[test]
fn detached_witness_wins_raw_hash_and_recovers_parent_before_child() {
    let store = store();
    let parent = output_tx(5015);
    let old = entry(
        &store,
        parent.clone(),
        ingress::remote_source(51.into(), 123).unwrap(),
    );
    insert(&store, old);
    let changed = parent
        .as_advanced_builder()
        .witness(Bytes::from_static(b"detached").pack())
        .build();
    let child = spend(5016, &[OutPoint::new(parent.hash(), 0)], &[]);
    let detached = BlockBuilder::default()
        .transaction(tx(0))
        .transaction(child.clone())
        .transaction(changed.clone())
        .build();
    let command = ChainReorgArgs::for_test([detached].into(), VecDeque::new(), store.snapshot().1);
    apply(&store, &command);
    let restored = store.point(&parent.hash()).1.unwrap();
    assert_eq!(restored.transaction.witness_hash(), changed.witness_hash());
    assert!(matches!(restored.source, Source::Recovery));
    assert!(matches!(restored.phase, Phase::Resolve));
    assert!(restored.arrival < store.point(&child.hash()).1.unwrap().arrival);
}

#[test]
fn reorg_publishes_proposals_only_for_owners_that_stay_accepted() {
    let store = store();
    let detached_tx = output_tx(5020);
    let survivor = output_tx(5021);
    for transaction in [&detached_tx, &survivor] {
        let hash = accept(&store, transaction.clone(), 20, 1, Status::Pending);
        let old = store.point(&hash).1.unwrap();
        let mut value = old.accepted().unwrap().clone();
        value.forced_status = None;
        replace(&store, old, Phase::Accepted(value));
    }
    let detached = BlockBuilder::default()
        .transaction(tx(0))
        .transaction(detached_tx.clone())
        .build();
    let attached = BlockBuilder::default().transaction(tx(0)).build();
    let next = snapshot(
        &store.snapshot().1,
        &attached,
        HashSet::new(),
        HashSet::from([
            detached_tx.proposal_short_id(),
            survivor.proposal_short_id(),
        ]),
    );
    let command = ChainReorgArgs::for_test([detached].into(), [attached].into(), next);
    let batch = commit_reconcile(&store, &command, &config())
        .unwrap()
        .unwrap();
    let callbacks: Vec<_> = batch
        .effects()
        .iter()
        .filter_map(Effect::callback)
        .collect();
    assert!(
        matches!(callbacks.as_slice(), [CallbackEvent::Proposed(entry)] if entry.transaction.hash() == survivor.hash())
    );
    let recovered = store.point(&detached_tx.hash()).1.unwrap();
    assert!(matches!(recovered.phase, Phase::Resolve));
    assert_eq!(store.capture_summary().proposed, 1);
}

#[test]
fn reorg_moves_context_sensitive_accepted_closure_to_recovery() {
    let store = store();
    let time_locked = output_tx(5017)
        .as_advanced_builder()
        .input(CellInput::new(OutPoint::new(tx(7017).hash(), 0), 1))
        .build();
    let root = accept(&store, time_locked, 1, 1, Status::Pending);
    assert!(
        store
            .point(&root)
            .1
            .unwrap()
            .accepted()
            .unwrap()
            .context_sensitive
    );
    let child = accept(
        &store,
        spend(5018, &[OutPoint::new(root.clone(), 0)], &[]),
        1,
        1,
        Status::Pending,
    );
    let unrelated = accept(&store, output_tx(5019), 1, 1, Status::Pending);
    for hash in [&child, &unrelated] {
        assert!(
            !store
                .point(hash)
                .1
                .unwrap()
                .accepted()
                .unwrap()
                .context_sensitive
        );
    }
    apply(&store, &command(&store, Vec::new()));
    assert_eq!(
        accepted_hashes(&store),
        [root.clone(), child.clone(), unrelated.clone()].into(),
        "ordinary attachment retains a satisfied time condition"
    );
    apply(
        &store,
        &ChainReorgArgs::for_test(
            [BlockBuilder::default().transaction(tx(0)).build()].into(),
            VecDeque::new(),
            store.snapshot().1,
        ),
    );
    for hash in [root, child] {
        let owner = store.point(&hash).1.unwrap();
        assert!(matches!(owner.source, Source::Recovery));
        assert!(matches!(owner.phase, Phase::Resolve));
    }
    assert_eq!(accepted_hashes(&store), [unrelated].into());
}

#[test]
fn optional_detached_candidates_do_not_prevent_either_chain_recovery_plan() {
    use ckb_types::core::tx_pool::TRANSACTION_SIZE_LIMIT;

    for bounded in [false, true] {
        let store = store();
        let (view, base) = store.snapshot();
        let parent = output_tx(5500);
        let child = spend(5501, &[OutPoint::new(parent.hash(), 0)], &[]);
        let excluded = tx(5502)
            .as_advanced_builder()
            .witness(Bytes::from(vec![0; TRANSACTION_SIZE_LIMIT as usize]).pack())
            .build();
        assert!(BoundedTransaction::try_new(excluded.clone()).is_err());
        let detached = BlockBuilder::default()
            .transactions([tx(0), child.clone(), excluded.clone(), parent.clone()])
            .build();
        let attached = BlockBuilder::default().transaction(tx(0)).build();
        let next = snapshot(&base, &attached, HashSet::new(), HashSet::new());
        let command =
            ChainReorgArgs::for_test([detached].into(), [attached].into(), Arc::clone(&next));
        if bounded {
            commit_recover_bounded(&store, &command).unwrap();
        } else {
            commit_reconcile(&store, &command, &config()).unwrap();
        }
        let (after_view, after_snapshot) = store.snapshot();
        assert!(after_view > view);
        assert!(Arc::ptr_eq(&after_snapshot, &next));
        assert!(store.point(&excluded.hash()).1.is_none());
        let parent = store.point(&parent.hash()).1.unwrap();
        let child = store.point(&child.hash()).1.unwrap();
        assert!(parent.arrival < child.arrival);
        for entry in [parent, child] {
            assert!(matches!(entry.source, Source::Recovery));
            assert!(matches!(entry.phase, Phase::Resolve));
        }
        assert_eq!(store.capture_all().owners.len(), 2);
        assert!(!store.is_faulted());
    }
}

#[test]
fn proposal_view_promotes_remote_and_expiry_preserves_its_origin_and_deadline() {
    for bounded in [false, true] {
        let store = store();
        let source = ingress::remote_source(52.into(), 123).unwrap();
        let owner = entry(&store, tx(5019), source);
        insert(&store, Arc::clone(&owner));
        let base = store.snapshot().1;
        let tip = base.consensus().genesis_block();
        let proposed = snapshot(&base, tip, HashSet::new(), [owner.proposal()].into());
        let run = |store: &Store, command: &ChainReorgArgs| {
            if bounded {
                commit_recover_bounded(store, command).unwrap();
            } else {
                commit_reconcile(store, command, &config()).unwrap();
            }
        };
        run(
            &store,
            &ChainReorgArgs::for_test(VecDeque::new(), VecDeque::new(), proposed),
        );
        let promoted = store.point(&owner.hash()).1.unwrap();
        assert!(matches!(
            promoted.source,
            Source::Proposal { remote: Some(_) }
        ));
        assert_eq!(promoted.source.residency_peer(), source.residency_peer());
        assert_eq!(promoted.source.deadline(), source.deadline());
        run(
            &store,
            &ChainReorgArgs::for_test(VecDeque::new(), VecDeque::new(), base),
        );
        let demoted = store.point(&owner.hash()).1.unwrap();
        assert!(matches!(
            demoted.source,
            Source::Remote { cycles: None, .. }
        ));
        assert_eq!(demoted.source.deadline(), source.deadline());
    }
}

#[test]
fn chain_conflict_removes_more_readers_than_ordinary_replacement_policy_limit() {
    let store = store();
    let shared = OutPoint::new(tx(5996).hash(), 0);
    for nonce in 5100..5250 {
        accept(
            &store,
            spend(nonce, &[], std::slice::from_ref(&shared)),
            1,
            1,
            Status::Pending,
        );
    }
    apply(&store, &command(&store, vec![spend(5251, &[shared], &[])]));
    assert!(accepted_hashes(&store).is_empty());
    assert!(!store.is_faulted());
}

#[test]
fn over_capacity_reorg_retains_a_bounded_parent_first_recovery_population() {
    let configuration = config();
    let store = store_with_pipeline_limit(
        crate::test_support::genesis_snapshot(),
        &configuration,
        24_000,
    );
    let first = output_tx(5300);
    let mut parent = accept(&store, first, 1, 1, Status::Pending);
    let mut hashes = vec![parent.clone()];
    for nonce in 5301..5320 {
        parent = accept(
            &store,
            spend(nonce, &[OutPoint::new(parent, 0)], &[]),
            1,
            1,
            Status::Pending,
        );
        hashes.push(parent.clone());
    }
    let owner = store.point(&hashes[0]).1.unwrap();
    let mut value = owner.accepted().unwrap().clone();
    value.context_sensitive = true;
    replace(&store, owner, Phase::Accepted(value));
    let command = ChainReorgArgs::for_test(
        [BlockBuilder::default().transaction(tx(0)).build()].into(),
        VecDeque::new(),
        store.snapshot().1,
    );
    assert!(matches!(
        commit_reconcile(&store, &command, &configuration),
        Err(Error::Full(_))
    ));
    assert_eq!(accepted_hashes(&store).len(), 20);
    commit_recover_bounded(&store, &command).unwrap();
    let restored = store.capture_all().owners;
    assert!(!restored.is_empty());
    assert!(restored.len() < hashes.len());
    let actual: BTreeSet<_> = restored.iter().map(|entry| entry.hash()).collect();
    assert_eq!(actual, hashes.into_iter().take(restored.len()).collect());
    assert!(
        restored
            .iter()
            .all(|entry| matches!(entry.source, Source::Recovery)
                && matches!(entry.phase, Phase::Resolve))
    );
    assert!(accepted_hashes(&store).is_empty());
    assert!(!store.is_faulted());
}

#[test]
fn trusted_pending_proposal_survives_unchanged_window_and_expires_only_after_proposal() {
    for bounded in [false, true] {
        let store = Store::new(chain_snapshot(), &config()).unwrap();
        let transaction = funded_tx(OutPoint::new(tx(5024).hash(), 0), 20_000_000_000);
        ingress::commit_ingress(
            &store,
            Arc::new(transaction.clone()),
            Source::Proposal { remote: None },
        )
        .unwrap();
        let hash = transaction.hash();
        assert!(matches!(
            store.point(&hash).1.unwrap().phase,
            Phase::Resolve
        ));
        let base = store.snapshot().1;
        let update = |next| {
            let args = ChainReorgArgs::for_test(VecDeque::new(), VecDeque::new(), next);
            if bounded {
                commit_recover_bounded(&store, &args).unwrap();
            } else {
                commit_reconcile(&store, &args, &config()).unwrap();
            }
        };
        update(Arc::clone(&base));
        assert!(matches!(
            store.point(&hash).1.unwrap().source,
            Source::Proposal { remote: None }
        ));
        let proposed = snapshot(
            &base,
            base.consensus().genesis_block(),
            [transaction.proposal_short_id()].into(),
            HashSet::new(),
        );
        update(proposed);
        assert!(store.point(&hash).1.is_some());
        update(base);
        assert!(store.point(&hash).1.is_none());
    }
}

#[test]
fn detached_header_requeues_its_dependent_owner_and_causal_children_only() {
    let store = store();
    let detached = BlockBuilder::default()
        .number(7)
        .epoch(ckb_types::core::EpochNumberWithFraction::new(0, 7, 1000))
        .build();
    let reader = output_tx(8600)
        .as_advanced_builder()
        .header_dep(detached.hash())
        .build();
    let parent = accept(&store, reader, 1, 1, Status::Pending);
    let child = accept(
        &store,
        spend(8601, &[OutPoint::new(parent.clone(), 0)], &[]),
        1,
        1,
        Status::Pending,
    );
    let unrelated = accept(&store, output_tx(8602), 1, 1, Status::Pending);
    let stable = store.point(&unrelated).1.unwrap();
    let next = BlockBuilder::default()
        .number(8)
        .timestamp(1)
        .epoch(ckb_types::core::EpochNumberWithFraction::new(0, 8, 1000))
        .build();
    let base = store.snapshot().1;
    let command = ChainReorgArgs::for_test(
        [detached].into(),
        [next.clone()].into(),
        snapshot(&base, &next, HashSet::new(), HashSet::new()),
    );
    apply(&store, &command);
    for hash in [parent, child] {
        let owner = store.point(&hash).1.unwrap();
        assert!(matches!(owner.phase, Phase::Resolve));
        assert_eq!(owner.source, Source::Recovery);
    }
    assert!(Arc::ptr_eq(&stable, &store.point(&unrelated).1.unwrap()));
}

#[test]
fn script_rule_transition_rechecks_each_phase_only_when_the_next_block_changes_vm() {
    use ckb_chain_spec::consensus::ConsensusBuilder;
    use ckb_types::{
        U256,
        core::{EpochNumberWithFraction, hardfork::HardForks},
    };
    let hardfork = HardForks::new_mirana();
    let boundaries = [
        hardfork.ckb2021.vm_version_1_and_syscalls_2(),
        hardfork.ckb2023.vm_version_2_and_syscalls_3(),
    ];
    let consensus = Arc::new(
        ConsensusBuilder::default()
            .hardfork_switch(hardfork)
            .build(),
    );
    let at = |epoch, index| {
        let block = BlockBuilder::default()
            .number(1)
            .epoch(EpochNumberWithFraction::new(epoch, index, 1000))
            .build();
        let snapshot = Arc::new(Snapshot::new(
            block.header(),
            U256::zero(),
            consensus.genesis_epoch_ext().clone(),
            MockStore::default().store().get_snapshot(),
            Default::default(),
            Arc::clone(&consensus),
        ));
        (block, snapshot)
    };
    for boundary in boundaries {
        for phase in [Status::Pending, Status::Gap, Status::Proposed] {
            for (before_epoch, before_index, epoch, index, changed) in [
                (boundary - 1, 998, boundary - 1, 999, true),
                (boundary - 1, 999, boundary, 0, false),
                (boundary, 0, boundary - 1, 998, true),
                (boundary, 0, boundary, 1, false),
            ] {
                let (_, old_snapshot) = at(before_epoch, before_index);
                let store = Store::new(old_snapshot, &config()).unwrap();
                let hash = accept(&store, output_tx(8603), 1, 1, phase);
                let old = store.point(&hash).1.unwrap();
                let (block, new_snapshot) = at(epoch, index);
                apply(
                    &store,
                    &ChainReorgArgs::for_test(Default::default(), [block].into(), new_snapshot),
                );
                let current = store.point(&hash).1.unwrap();
                assert!(Arc::ptr_eq(&old.transaction, &current.transaction));
                if changed {
                    assert!(matches!(current.phase, Phase::Resolve));
                    assert_eq!(current.source, Source::Recovery);
                    assert!(!Arc::ptr_eq(&old, &current));
                } else {
                    assert!(Arc::ptr_eq(&old, &current));
                }
            }
        }
    }
}

#[test]
fn chain_without_accepted_callbacks_skips_both_aggregate_passes() {
    let store = store();
    let parent = accept(&store, output_tx(6500), 1, 1, Status::Pending);
    let child = accept(
        &store,
        spend(6501, &[OutPoint::new(parent.clone(), 0)], &[]),
        1,
        1,
        Status::Pending,
    );
    // The existing two-member ancestry deliberately exceeds this traversal
    // bound, so success proves neither unused aggregate pass was evaluated.
    let configuration = TxPoolConfig {
        max_ancestors_count: 1,
        ..config()
    };
    let batch = commit_reconcile(&store, &command(&store, Vec::new()), &configuration).unwrap();
    if let Some(batch) = batch {
        assert!(
            batch
                .effects()
                .iter()
                .all(|effect| effect.callback().is_none())
        );
    }
    // A real accepted conflict still requires the old graph snapshot, including
    // its ancestor bound; the lazy gate must not suppress required computation.
    assert!(matches!(
        commit_reconcile(
            &store,
            &command(
                &store,
                vec![spend(6502, &[OutPoint::new(parent.clone(), 0)], &[])]
            ),
            &configuration,
        ),
        Err(Error::Rejected(Reject::ExceededMaximumAncestorsCount))
    ));
    assert_eq!(accepted_hashes(&store), [parent, child].into());
}

#[test]
fn chain_conflict_callbacks_keep_old_ancestor_and_descendant_totals() {
    let store = store();
    let external = OutPoint::new(tx(6510).hash(), 0);
    let parent = spend(6511, std::slice::from_ref(&external), &[]);
    let parent_hash = accept(&store, parent, 10, 1, Status::Pending);
    let child = accept(
        &store,
        spend(6512, &[OutPoint::new(parent_hash.clone(), 0)], &[]),
        20,
        1,
        Status::Pending,
    );
    let batch = commit_reconcile(
        &store,
        &command(&store, vec![spend(6513, &[external], &[])]),
        &config(),
    )
    .unwrap()
    .unwrap();
    let callbacks: BTreeMap<_, _> = batch
        .effects()
        .iter()
        .filter_map(|effect| match effect.callback() {
            Some(CallbackEvent::Reject(entry, _)) => Some((entry.transaction.hash(), entry)),
            _ => None,
        })
        .collect();
    assert_eq!(callbacks.len(), 2);
    assert_eq!(callbacks[&parent_hash].descendants_count, 2);
    assert_eq!(callbacks[&parent_hash].descendants_fee.as_u64(), 30);
    assert_eq!(callbacks[&child].ancestors_count, 2);
    assert_eq!(callbacks[&child].ancestors_fee.as_u64(), 30);
}

#[test]
fn chain_callbacks_follow_public_pending_and_proposed_transitions() {
    for unrelated in [0, 4_096] {
        let store = store();
        let transaction = output_tx(6522);
        let hash = accept(&store, transaction.clone(), 20, 1, Status::Pending);
        let old = store.point(&hash).1.unwrap();
        let mut accepted = old.accepted().unwrap().clone();
        accepted.forced_status = None;
        replace(&store, old, Phase::Accepted(accepted));

        for (number, (status, callback)) in [
            (Status::Gap, None),
            (Status::Pending, None),
            (Status::Gap, None),
            (Status::Proposed, Some(Status::Proposed)),
            (Status::Proposed, None),
            (Status::Gap, Some(Status::Pending)),
            (Status::Pending, None),
            (Status::Proposed, Some(Status::Proposed)),
        ]
        .into_iter()
        .enumerate()
        {
            let base = store.snapshot().1;
            let height = number as u64 + 1;
            let block = BlockBuilder::default()
                .number(height)
                .epoch(base.epoch_ext().number_with_fraction(height))
                .transaction(tx(0))
                .build();
            let proposals = |phase| {
                let mut ids: HashSet<_> = (0..unrelated)
                    .map(|value: u32| {
                        let mut bytes = [0; 10];
                        bytes[..4].copy_from_slice(&value.to_be_bytes());
                        ProposalShortId::new(bytes)
                    })
                    .filter(|id| *id != transaction.proposal_short_id())
                    .collect();
                if status == phase {
                    ids.insert(transaction.proposal_short_id());
                }
                ids
            };
            let next = snapshot(
                &base,
                &block,
                proposals(Status::Gap),
                proposals(Status::Proposed),
            );
            let command = ChainReorgArgs::for_test(VecDeque::new(), [block].into(), next);
            let batch = commit_reconcile(&store, &command, &config())
                .unwrap()
                .unwrap();
            let callbacks: Vec<_> = batch
                .effects()
                .iter()
                .filter_map(|effect| match effect.callback() {
                    Some(CallbackEvent::Pending(entry)) => {
                        Some((Status::Pending, entry.transaction.hash()))
                    }
                    Some(CallbackEvent::Proposed(entry)) => {
                        Some((Status::Proposed, entry.transaction.hash()))
                    }
                    Some(CallbackEvent::Reject(..)) => panic!("a proposal change cannot reject"),
                    None => None,
                })
                .collect();
            assert_eq!(
                callbacks,
                callback
                    .map(|status| (status, hash.clone()))
                    .into_iter()
                    .collect::<Vec<_>>(),
                "transition {number}: Gap does not become a second new-transaction event"
            );
            let current = store.point(&hash).1.unwrap();
            assert_eq!(
                current.accepted().unwrap().status(&store.snapshot().1),
                status
            );
            assert_eq!(
                store.capture_summary().proposed,
                usize::from(status == Status::Proposed)
            );
            assert_eq!(
                store.capture_summary().gap,
                usize::from(status == Status::Gap)
            );
        }
    }
}

#[test]
fn chain_phase_counts_follow_the_final_owner_during_a_proposal_handoff() {
    for before in [Status::Pending, Status::Gap, Status::Proposed] {
        for after in [Status::Pending, Status::Gap, Status::Proposed] {
            for accepted_after in [false, true] {
                let store = store();
                let identity = |suffix| {
                    let mut bytes = [7; 32];
                    bytes[31] = suffix;
                    Byte32::new(bytes)
                };
                let transaction = output_tx(6800).fake_hash(identity(1));
                let hash = accept(&store, transaction.clone(), 1, 1, Status::Pending);
                let old = store.point(&hash).1.unwrap();
                let mut value = old.accepted().unwrap().clone();
                value.forced_status = None;
                replace(&store, old, Phase::Accepted(value));
                let proposals = |status, band| {
                    if status == band {
                        [transaction.proposal_short_id()].into()
                    } else {
                        HashSet::new()
                    }
                };
                let block = BlockBuilder::default().transaction(tx(0)).build();
                let previous = snapshot(
                    &store.snapshot().1,
                    &block,
                    proposals(before, Status::Gap),
                    proposals(before, Status::Proposed),
                );
                apply(
                    &store,
                    &ChainReorgArgs::for_test(VecDeque::new(), [block.clone()].into(), previous),
                );
                assert_eq!(
                    store.capture_summary().proposed,
                    usize::from(before == Status::Proposed)
                );
                assert_eq!(
                    store.capture_summary().gap,
                    usize::from(before == Status::Gap)
                );

                let old = store.point(&hash).1.unwrap();
                let new = entry(
                    &store,
                    output_tx(6801).fake_hash(identity(2)),
                    Source::Local,
                );
                let new = if accepted_after {
                    let mut value = old.accepted().unwrap().clone();
                    Arc::make_mut(&mut value.transaction).transaction =
                        new.transaction.as_ref().clone();
                    new.with_phase(Phase::Accepted(value))
                } else {
                    new
                };
                assert_eq!(old.proposal(), new.proposal());
                let next = snapshot(
                    &store.snapshot().1,
                    &block,
                    proposals(after, Status::Gap),
                    proposals(after, Status::Proposed),
                );
                store
                    .commit_with(|tx| {
                        let changes: Option<Vec<_>> = tx
                            .state
                            .snapshot
                            .proposals()
                            .status_changes(next.proposals(), tx.state.usage.accepted.items)
                            .map(Iterator::collect);
                        let edits = vec![
                            OwnerEdit {
                                hash: old.hash_bytes(),
                                before: Some(Arc::clone(&old)),
                                after: None,
                                ancestors: None,
                            },
                            OwnerEdit {
                                hash: new.hash_bytes(),
                                before: None,
                                after: Some(Arc::clone(&new)),
                                ancestors: None,
                            },
                        ];
                        tx.commit_edits(edits, Vec::new(), Vec::new(), Class::Trusted)?;
                        tx.apply_lifecycle(Arc::clone(&next), changes.as_deref(), &[]);
                        Ok(())
                    })
                    .unwrap();
                assert!(store.point(&hash).1.is_none());
                assert!(Arc::ptr_eq(&store.point(&new.hash()).1.unwrap(), &new));
                assert_eq!(
                    store.capture_summary().proposed,
                    usize::from(after == Status::Proposed && accepted_after)
                );
                assert_eq!(
                    store.capture_summary().gap,
                    usize::from(after == Status::Gap && accepted_after)
                );
                assert!(!store.is_faulted());
            }
        }
    }
}

#[test]
fn chain_phase_changes_preserve_fixture_overrides_and_full_hash_callback_order() {
    for extra_owners in [0, 2] {
        let store = store();
        let mut canonical = Vec::new();
        let mut ids = HashSet::new();
        for (prefix, forced) in [
            (30, Some(Status::Proposed)),
            (10, Some(Status::Pending)),
            (40, None),
            (20, None),
        ] {
            let transaction =
                output_tx(6600 + u32::from(prefix)).fake_hash(Byte32::new([prefix; 32]));
            ids.insert(transaction.proposal_short_id());
            let hash = accept(
                &store,
                transaction,
                20,
                1,
                forced.unwrap_or(Status::Pending),
            );
            let old = store.point(&hash).1.unwrap();
            let mut accepted = old.accepted().unwrap().clone();
            accepted.forced_status = forced;
            replace(&store, old, Phase::Accepted(accepted));
            if forced.is_none() {
                canonical.push(hash);
            }
        }
        for nonce in 0..extra_owners {
            accept(&store, output_tx(6700 + nonce), 1, 1, Status::Pending);
        }
        canonical.sort();
        ids.extend([0, 1, 2, 3, u8::MAX].map(|byte| ProposalShortId::new([byte; 10])));
        for (number, phase) in [
            Status::Proposed,
            Status::Gap,
            Status::Proposed,
            Status::Pending,
        ]
        .into_iter()
        .enumerate()
        {
            let base = store.snapshot().1;
            let height = number as u64 + 1;
            let block = BlockBuilder::default()
                .number(height)
                .epoch(base.epoch_ext().number_with_fraction(height))
                .transaction(tx(0))
                .build();
            let band = |status| {
                if phase == status {
                    ids.clone()
                } else {
                    HashSet::new()
                }
            };
            let next = snapshot(&base, &block, band(Status::Gap), band(Status::Proposed));
            let command = ChainReorgArgs::for_test(VecDeque::new(), [block].into(), next);
            let batch = commit_reconcile(&store, &command, &config())
                .unwrap()
                .unwrap();
            let callbacks: Vec<_> = batch
                .effects()
                .iter()
                .filter_map(|effect| match effect.callback() {
                    Some(CallbackEvent::Pending(entry)) => Some((false, entry.transaction.hash())),
                    Some(CallbackEvent::Proposed(entry)) => Some((true, entry.transaction.hash())),
                    Some(CallbackEvent::Reject(..)) => panic!("a phase change cannot reject"),
                    None => None,
                })
                .collect();
            let proposed = phase == Status::Proposed;
            assert_eq!(
                callbacks,
                canonical
                    .iter()
                    .cloned()
                    .map(|hash| (proposed, hash))
                    .collect::<Vec<_>>()
            );
            assert_eq!(
                store.capture_summary().proposed,
                1 + if proposed { 2 } else { 0 }
            );
            assert_eq!(
                store.capture_summary().gap,
                if phase == Status::Gap { 2 } else { 0 }
            );
        }
    }
}

#[test]
fn chain_status_callback_uses_final_graph_after_parent_commit() {
    let store = store();
    let parent = output_tx(6520);
    let parent_hash = accept(&store, parent.clone(), 10, 1, Status::Pending);
    let child = spend(6521, &[OutPoint::new(parent_hash, 0)], &[]);
    let child_hash = accept(&store, child.clone(), 20, 1, Status::Pending);
    let old = store.point(&child_hash).1.unwrap();
    let mut accepted = old.accepted().unwrap().clone();
    accepted.forced_status = None;
    replace(&store, old, Phase::Accepted(accepted));
    let block = BlockBuilder::default()
        .transaction(tx(0))
        .transaction(parent)
        .build();
    let snapshot = snapshot(
        &store.snapshot().1,
        &block,
        HashSet::new(),
        [child.proposal_short_id()].into(),
    );
    let command = ChainReorgArgs::for_test(VecDeque::new(), [block].into(), snapshot);
    let batch = commit_reconcile(&store, &command, &config())
        .unwrap()
        .unwrap();
    let callbacks: Vec<_> = batch
        .effects()
        .iter()
        .filter_map(|effect| match effect.callback() {
            Some(CallbackEvent::Proposed(entry)) => Some(entry),
            _ => None,
        })
        .collect();
    assert_eq!(callbacks.len(), 1);
    assert_eq!(callbacks[0].transaction.hash(), child_hash);
    assert_eq!(callbacks[0].ancestors_count, 1);
    assert_eq!(callbacks[0].ancestors_fee.as_u64(), 20);
    assert_eq!(callbacks[0].descendants_count, 1);
}

#[test]
fn proposal_callback_includes_unchanged_descendants() {
    let store = store();
    let root = output_tx(6530);
    let root_hash = accept(&store, root.clone(), 10, 1, Status::Pending);
    let child = accept(
        &store,
        spend(6531, &[OutPoint::new(root_hash.clone(), 0)], &[]),
        20,
        1,
        Status::Pending,
    );
    let leaf = accept(
        &store,
        spend(6532, &[OutPoint::new(child.clone(), 0)], &[]),
        30,
        1,
        Status::Pending,
    );
    let old_root = store.point(&root_hash).1.unwrap();
    let mut accepted = old_root.accepted().unwrap().clone();
    accepted.forced_status = None;
    replace(&store, old_root, Phase::Accepted(accepted));
    let old_child = store.point(&child).1.unwrap();
    let old_leaf = store.point(&leaf).1.unwrap();
    let block = BlockBuilder::default().transaction(tx(0)).build();
    let next = snapshot(
        &store.snapshot().1,
        &block,
        HashSet::new(),
        [root.proposal_short_id()].into(),
    );
    let command = ChainReorgArgs::for_test(VecDeque::new(), [block].into(), next);
    let batch = commit_reconcile(&store, &command, &config())
        .unwrap()
        .unwrap();
    let callbacks: Vec<_> = batch
        .effects()
        .iter()
        .filter_map(|effect| match effect.callback() {
            Some(CallbackEvent::Proposed(entry)) => Some(entry),
            _ => None,
        })
        .collect();
    assert_eq!(callbacks.len(), 1);
    assert_eq!(callbacks[0].transaction.hash(), root_hash);
    assert_eq!(callbacks[0].ancestors_count, 1);
    assert_eq!(callbacks[0].descendants_count, 3);
    assert_eq!(callbacks[0].descendants_fee.as_u64(), 60);
    assert!(Arc::ptr_eq(&old_child, &store.point(&child).1.unwrap()));
    assert!(Arc::ptr_eq(&old_leaf, &store.point(&leaf).1.unwrap()));
}

#[test]
fn reorg_conflict_wins_where_recovery_and_conflict_descendants_intersect() {
    let store = store();
    let time_locked = output_tx(8700)
        .as_advanced_builder()
        .input(CellInput::new(OutPoint::new(tx(8790).hash(), 0), 1))
        .build();
    let recovering = accept(&store, time_locked, 1, 1, Status::Pending);
    let spent = OutPoint::new(tx(8791).hash(), 0);
    let conflicting = accept(
        &store,
        spend(8701, std::slice::from_ref(&spent), &[]),
        1,
        1,
        Status::Pending,
    );
    let shared = accept(
        &store,
        spend(
            8702,
            &[OutPoint::new(recovering.clone(), 0)],
            &[OutPoint::new(conflicting.clone(), 0)],
        ),
        1,
        1,
        Status::Pending,
    );
    let descendant = accept(
        &store,
        spend(8703, &[OutPoint::new(shared.clone(), 0)], &[]),
        1,
        1,
        Status::Pending,
    );
    let unrelated = accept(&store, output_tx(8704), 1, 1, Status::Pending);
    let stable = store.point(&unrelated).1.unwrap();
    let detached = BlockBuilder::default().transaction(tx(0)).build();
    let attached = BlockBuilder::default()
        .transaction(tx(0))
        .transaction(spend(8705, &[spent], &[]))
        .build();
    let next = snapshot(
        &store.snapshot().1,
        &attached,
        HashSet::new(),
        HashSet::new(),
    );
    let command = ChainReorgArgs::for_test([detached].into(), [attached].into(), next);
    let batch = commit_reconcile(&store, &command, &config())
        .unwrap()
        .unwrap();
    let rejected: BTreeSet<_> = batch
        .effects()
        .iter()
        .filter_map(|effect| match effect.callback() {
            Some(CallbackEvent::Reject(entry, _)) => Some(entry.transaction.hash()),
            _ => None,
        })
        .collect();
    let removed = [conflicting, shared, descendant];
    assert_eq!(rejected, removed.iter().cloned().collect());
    for hash in removed {
        assert!(store.point(&hash).1.is_none());
    }
    let recovered = store.point(&recovering).1.unwrap();
    assert_eq!(recovered.source, Source::Recovery);
    assert!(matches!(recovered.phase, Phase::Resolve));
    assert!(Arc::ptr_eq(&stable, &store.point(&unrelated).1.unwrap()));
    assert_eq!(accepted_hashes(&store), [unrelated].into());
}

#[test]
fn committed_spend_retires_live_waiter_but_preserves_replacement_history() {
    let store = store();
    let spent = OutPoint::new(tx(8792).hash(), 0);
    let missing = OutPoint::new(tx(8793).hash(), 0);
    let waiting = entry(
        &store,
        spend(8710, std::slice::from_ref(&spent), &[]),
        Source::Recovery,
    );
    insert(&store, Arc::clone(&waiting));
    let waiting = replace(&store, waiting, Phase::Waiting([missing.clone()].into()));
    let history = entry(
        &store,
        spend(8711, std::slice::from_ref(&spent), &[]),
        Source::Recovery,
    );
    insert(&store, Arc::clone(&history));
    let history = replace(
        &store,
        history,
        Phase::Replaced(RecoveryTriggers::RetryInputs([missing].into())),
    );
    commit_reconcile(
        &store,
        &command(&store, vec![spend(8712, &[spent], &[])]),
        &config(),
    )
    .unwrap();
    assert!(store.point(&waiting.hash()).1.is_none());
    assert!(Arc::ptr_eq(
        &history,
        &store.point(&history.hash()).1.unwrap()
    ));
    assert!(accepted_hashes(&store).is_empty());
}

#[test]
fn chain_wakes_cover_owner_removals_and_canonical_only_transactions() {
    for detach in [false, true] {
        let store = store();
        let unrelated = [8748, 8749].map(|nonce| {
            let hash = accept(&store, output_tx(nonce), 1, 1, Status::Pending);
            store.point(&hash).1.unwrap()
        });
        let bodies = [8720, 8721, 8722, 8723]
            .map(|nonce| spend(nonce, &[OutPoint::new(tx(nonce + 100).hash(), 0)], &[]));
        accept(&store, bodies[0].clone(), 1, 1, Status::Pending);
        insert(&store, entry(&store, bodies[1].clone(), Source::Recovery));
        let mut expected = BTreeMap::new();
        for (index, point) in bodies[..if detach { 4 } else { 3 }]
            .iter()
            .flat_map(|tx| tx.input_pts_iter().chain(tx.output_pts_iter()))
            .enumerate()
        {
            let history = entry(
                &store,
                output_tx(8730 + u32::try_from(index).unwrap()),
                Source::Recovery,
            )
            .with_phase(Phase::Replaced(RecoveryTriggers::RetryInputs(
                [point.clone()].into(),
            )));
            assert!(expected.insert(point, history.hash()).is_none());
            insert(&store, history);
        }
        assert!(store.wake_page(&mut None).is_none());
        let attached = BlockBuilder::default()
            .transaction(tx(0))
            .transactions(bodies[..3].to_vec())
            .build();
        let detached = BlockBuilder::default()
            .transaction(tx(1))
            .transaction(bodies[3].clone())
            .build();
        let next = snapshot(
            &store.snapshot().1,
            &attached,
            HashSet::new(),
            HashSet::new(),
        );
        apply(
            &store,
            &ChainReorgArgs::for_test(
                if detach {
                    [detached].into()
                } else {
                    VecDeque::new()
                },
                [attached].into(),
                next,
            ),
        );
        for body in &bodies[..3] {
            assert!(store.point(&body.hash()).1.is_none());
        }
        let recovered = store.point(&bodies[3].hash()).1;
        if detach {
            assert!(matches!(recovered.unwrap().phase, Phase::Resolve));
        } else {
            assert!(recovered.is_none());
        }
        for owner in unrelated {
            assert!(Arc::ptr_eq(&owner, &store.point(&owner.hash()).1.unwrap()));
        }
        let mut observed = BTreeMap::new();
        let mut cursor = None;
        for _ in 0..expected.len() {
            let page = store.wake_page(&mut cursor).unwrap();
            assert_eq!(page.hashes().len(), 1);
            assert!(
                observed
                    .insert(page.key().to_out_point(), Byte32::new(page.hashes()[0]))
                    .is_none()
            );
            store
                .commit_with(|tx| {
                    tx.advance_wake(&page.key(), page.pass(), page.hashes().last().copied());
                    Ok(())
                })
                .unwrap();
        }
        assert_eq!(observed, expected);
        assert!(store.wake_page(&mut cursor).is_none());
        assert!(!store.is_faulted());
    }
}

#[test]
fn reorg_priority_gate_preempts_ordinary_writers_and_heavy_captures() {
    use std::sync::{
        Barrier,
        atomic::{AtomicBool, Ordering},
    };

    let store = store();
    let existing = accept(&store, output_tx(8800), 10, 1, Status::Pending);
    let guard = store.enter_reorg_priority();
    assert!(store.reorg_pending());

    let started = Arc::new(Barrier::new(3));
    let writer_done = Arc::new(AtomicBool::new(false));
    let capture_done = Arc::new(AtomicBool::new(false));

    let writer_store = Arc::clone(&store);
    let writer_started = Arc::clone(&started);
    let writer_flag = Arc::clone(&writer_done);
    let writer = std::thread::spawn(move || {
        writer_started.wait();
        writer_store.commit_with(|_| Ok(())).unwrap();
        writer_flag.store(true, Ordering::SeqCst);
    });

    let capture_store = Arc::clone(&store);
    let capture_started = Arc::clone(&started);
    let capture_flag = Arc::clone(&capture_done);
    let capture = std::thread::spawn(move || {
        capture_started.wait();
        let captured = capture_store.capture_all();
        assert_eq!(captured.owners.len(), 1);
        capture_flag.store(true, Ordering::SeqCst);
    });

    started.wait();
    while store.reorg_parked() < 2 {
        std::thread::yield_now();
    }

    assert!(!writer_done.load(Ordering::SeqCst));
    assert!(!capture_done.load(Ordering::SeqCst));

    // Fast point reads and priority reorg commits must proceed immediately
    // while ordinary writers and full-pool captures stay parked on ReorgGate.
    assert!(store.point(&existing).1.is_some());
    store.commit_reorg(|_| Ok(())).unwrap();
    assert!(!writer_done.load(Ordering::SeqCst));
    assert!(!capture_done.load(Ordering::SeqCst));

    drop(guard);
    writer.join().unwrap();
    capture.join().unwrap();
    assert!(writer_done.load(Ordering::SeqCst));
    assert!(capture_done.load(Ordering::SeqCst));
    assert!(!store.reorg_pending());
    assert_eq!(store.reorg_parked(), 0);
}

#[test]
fn reorg_optimistic_plan_escalates_on_intervening_commit_and_emits_localized_component_callbacks() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    let store = store();
    let parent_tx = output_tx(8810);
    let parent = accept(&store, parent_tx.clone(), 10, 1, Status::Pending);
    let child = accept(
        &store,
        spend(8811, &[OutPoint::new(parent.clone(), 0)], &[]),
        25,
        1,
        Status::Pending,
    );
    let solo_a_tx = output_tx(8812);
    let solo_a = accept(&store, solo_a_tx.clone(), 15, 1, Status::Pending);
    let solo_b = accept(&store, output_tx(8813), 5, 1, Status::Pending);
    for hash in [&parent, &child, &solo_a, &solo_b] {
        let old = store.point(hash).1.unwrap();
        let mut accepted = old.accepted().unwrap().clone();
        accepted.forced_status = None;
        replace(&store, old, Phase::Accepted(accepted));
    }

    let concurrent_tx = spend(8814, &[OutPoint::new(solo_b.clone(), 0)], &[]);
    let concurrent_hash = concurrent_tx.hash();
    let attempts = Arc::new(AtomicUsize::new(0));
    let optimistic_first_attempt = Arc::new(AtomicBool::new(false));
    let priority_on_retry = Arc::new(AtomicBool::new(false));

    let hook_store = Arc::clone(&store);
    let hook_attempts = Arc::clone(&attempts);
    let hook_optimistic = Arc::clone(&optimistic_first_attempt);
    let hook_priority = Arc::clone(&priority_on_retry);
    let hook_tx = concurrent_tx;
    *store.before_commit.lock() = Some(Arc::new(move || {
        let attempt = hook_attempts.fetch_add(1, Ordering::SeqCst);
        if attempt == 0 {
            hook_optimistic.store(!hook_store.reorg_pending(), Ordering::SeqCst);
            let saved = hook_store.before_commit.lock().take();
            accept(&hook_store, hook_tx.clone(), 7, 1, Status::Pending);
            *hook_store.before_commit.lock() = saved;
        } else {
            hook_priority.store(hook_store.reorg_pending(), Ordering::SeqCst);
        }
    }));

    let block = BlockBuilder::default().transaction(tx(0)).build();
    let next = snapshot(
        &store.snapshot().1,
        &block,
        HashSet::new(),
        [parent_tx.proposal_short_id(), solo_a_tx.proposal_short_id()].into(),
    );
    let command = ChainReorgArgs::for_test(VecDeque::new(), [block].into(), next);
    let batch = commit_reconcile(&store, &command, &config())
        .unwrap()
        .unwrap();
    *store.before_commit.lock() = None;

    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert!(optimistic_first_attempt.load(Ordering::SeqCst));
    assert!(priority_on_retry.load(Ordering::SeqCst));
    assert!(!store.reorg_pending());
    assert!(store.point(&concurrent_hash).1.is_some());
    assert!(store.point(&solo_b).1.is_some());

    let mut callbacks: Vec<_> = batch
        .effects()
        .iter()
        .filter_map(|effect| match effect.callback() {
            Some(CallbackEvent::Proposed(entry)) => Some(entry),
            _ => None,
        })
        .collect();
    assert_eq!(callbacks.len(), 2);
    let mut expected_order = [parent.clone(), solo_a.clone()];
    expected_order.sort_by_key(hash_key);
    assert_eq!(
        callbacks
            .iter()
            .map(|entry| entry.transaction.hash())
            .collect::<Vec<_>>(),
        expected_order
    );
    callbacks.sort_by_key(|entry| entry.transaction.hash() != parent);
    let parent_cb = callbacks[0];
    let solo_cb = callbacks[1];
    assert_eq!(parent_cb.transaction.hash(), parent);
    assert_eq!(parent_cb.ancestors_count, 1);
    assert_eq!(parent_cb.descendants_count, 2);
    assert_eq!(parent_cb.descendants_fee.as_u64(), 35);
    assert_eq!(solo_cb.transaction.hash(), solo_a);
    assert_eq!(solo_cb.ancestors_count, 1);
    assert_eq!(solo_cb.descendants_count, 1);
    assert_eq!(solo_cb.descendants_fee.as_u64(), 15);
}

#[test]
fn reorg_optimistic_plan_commits_first_pass_across_disjoint_leaf_admissions() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    let store = store();
    // Fill past 64 pure leaf inserts so the ring buffer wraps before planning,
    // proving ring-buffer wraparound does not bump structural_epoch.
    for seed in 0..80u32 {
        accept(&store, output_tx(9000 + seed), 5, 1, Status::Pending);
    }
    let attached_tx = output_tx(8820);
    let attached_hash = accept(&store, attached_tx.clone(), 10, 1, Status::Proposed);
    let solo_tx = output_tx(8821);
    let solo = accept(&store, solo_tx.clone(), 15, 1, Status::Pending);
    for hash in [&attached_hash, &solo] {
        let old = store.point(hash).1.unwrap();
        let mut accepted = old.accepted().unwrap().clone();
        accepted.forced_status = None;
        replace(&store, old, Phase::Accepted(accepted));
    }

    let disjoint_leaf = output_tx(8822);
    let disjoint_hash = disjoint_leaf.hash();
    let attempts = Arc::new(AtomicUsize::new(0));
    let saw_reorg_pending = Arc::new(AtomicBool::new(false));
    let hook_store = Arc::clone(&store);
    let hook_attempts = Arc::clone(&attempts);
    let hook_pending = Arc::clone(&saw_reorg_pending);
    let hook_tx = disjoint_leaf;
    *store.before_commit.lock() = Some(Arc::new(move || {
        let attempt = hook_attempts.fetch_add(1, Ordering::SeqCst);
        if hook_store.reorg_pending() {
            hook_pending.store(true, Ordering::SeqCst);
        }
        if attempt == 0 {
            let saved = hook_store.before_commit.lock().take();
            accept(&hook_store, hook_tx.clone(), 7, 1, Status::Pending);
            *hook_store.before_commit.lock() = saved;
        }
    }));

    let block = BlockBuilder::default()
        .transaction(tx(0))
        .transaction(attached_tx)
        .build();
    let next = snapshot(
        &store.snapshot().1,
        &block,
        HashSet::new(),
        [solo_tx.proposal_short_id()].into(),
    );
    let command = ChainReorgArgs::for_test(VecDeque::new(), [block].into(), next);
    commit_reconcile(&store, &command, &config()).unwrap();
    *store.before_commit.lock() = None;

    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert!(!saw_reorg_pending.load(Ordering::SeqCst));
    assert!(!store.reorg_pending());
    assert!(store.point(&attached_hash).1.is_none());
    assert!(store.point(&disjoint_hash).1.is_some());
}

#[test]
fn parallel_filter_map_propagates_worker_panic_instead_of_returning_partial_results() {
    let items: Vec<usize> = (0..512).collect();
    let result = parallel_filter_map_with_workers(items, 4, None, |item| {
        assert_ne!(item, 150, "simulated worker panic inside chunk 1");
        Some(item)
    });
    assert!(
        matches!(result, Err(Error::Fault(_))),
        "any panicked worker must fail the entire parallel scan instead of returning partial results"
    );
}

#[test]
fn reorg_worker_panic_propagates_and_never_installs_partial_pool_view() {
    // 1. Detached block transaction compaction during `commit_reconcile`.
    {
        let store = store();
        let incumbent = accept(&store, tx(40_000), 10, 1, Status::Pending);
        let (before_view, before_snapshot) = store.snapshot();
        let detached_txs: Vec<_> = (1..=512).map(|i| tx(40_000 + i)).collect();
        let detached_block = BlockBuilder::default()
            .transaction(tx(40_999))
            .transactions(detached_txs)
            .build();
        let attached_block = BlockBuilder::default().transaction(tx(41_000)).build();
        let next = snapshot(
            &before_snapshot,
            &attached_block,
            HashSet::new(),
            HashSet::new(),
        );
        let args = ChainReorgArgs::for_test([detached_block].into(), [attached_block].into(), next);

        let _inject = inject_parallel_worker_panic(1);
        let result = commit_reconcile(&store, &args, &config());
        assert!(matches!(result, Err(Error::Fault(_))));
        assert!(store.is_faulted());
        let (after_view, after_snapshot) = store.snapshot();
        assert_eq!(after_view, before_view);
        assert_eq!(after_snapshot.tip_hash(), before_snapshot.tip_hash());
        assert_eq!(accepted_hashes(&store), [incumbent].into());
    }

    // 2. Recovery-root owner scan during `commit_reconcile` (detached header + >=512 owners).
    {
        let store = store();
        let mut expected = BTreeSet::new();
        for i in 0..512u32 {
            expected.insert(accept(&store, tx(42_000 + i), 10, 1, Status::Pending));
        }
        let (before_view, before_snapshot) = store.snapshot();
        let detached_block = BlockBuilder::default().transaction(tx(43_000)).build();
        let attached_block = BlockBuilder::default().transaction(tx(43_001)).build();
        let next = snapshot(
            &before_snapshot,
            &attached_block,
            HashSet::new(),
            HashSet::new(),
        );
        let args = ChainReorgArgs::for_test([detached_block].into(), [attached_block].into(), next);

        let _inject = inject_parallel_worker_panic(1);
        let result = commit_reconcile(&store, &args, &config());
        assert!(matches!(result, Err(Error::Fault(_))));
        assert!(store.is_faulted());
        let (after_view, after_snapshot) = store.snapshot();
        assert_eq!(after_view, before_view);
        assert_eq!(after_snapshot.tip_hash(), before_snapshot.tip_hash());
        assert_eq!(accepted_hashes(&store), expected);
    }

    // 3. Detached block transaction compaction during `commit_recover_bounded`.
    {
        let store = store();
        let incumbent = accept(&store, tx(44_000), 10, 1, Status::Pending);
        let (before_view, before_snapshot) = store.snapshot();
        let detached_txs: Vec<_> = (1..=512).map(|i| tx(44_000 + i)).collect();
        let detached_block = BlockBuilder::default()
            .transaction(tx(44_999))
            .transactions(detached_txs)
            .build();
        let attached_block = BlockBuilder::default().transaction(tx(45_000)).build();
        let next = snapshot(
            &before_snapshot,
            &attached_block,
            HashSet::new(),
            HashSet::new(),
        );
        let args = ChainReorgArgs::for_test([detached_block].into(), [attached_block].into(), next);

        let _inject = inject_parallel_worker_panic(1);
        let result = commit_recover_bounded(&store, &args);
        assert!(matches!(result, Err(Error::Fault(_))));
        assert!(store.is_faulted());
        let (after_view, after_snapshot) = store.snapshot();
        assert_eq!(after_view, before_view);
        assert_eq!(after_snapshot.tip_hash(), before_snapshot.tip_hash());
        assert_eq!(accepted_hashes(&store), [incumbent].into());
    }
}
