use super::*;
use crate::authority::chain::ClearScope;
use crate::authority::store::Captured;
use crate::authority::{
    model::{Phase, RecoveryTriggers, Source},
    notice::Class,
    queue::{WorkSelection, WorkStage},
    store::Plan,
    tests::common::*,
};
use ckb_types::{bytes::Bytes, core::TransactionBuilder, packed::CellOutput, prelude::*};
use std::{collections::BTreeSet, sync::Arc};

fn rbf_config() -> TxPoolConfig {
    TxPoolConfig {
        min_rbf_rate: FeeRate::from_u64(1000),
        ..config()
    }
}

#[test]
fn input_snapshot_reconstructs_membership_without_notifications_or_owner_retention() {
    let store = store();
    let cache = InputSnapshotCache::default();
    let mut accepted = std::collections::HashSet::new();
    for (index, status) in [Status::Pending, Status::Gap, Status::Proposed]
        .into_iter()
        .enumerate()
    {
        let point = OutPoint::new(tx(6800 + index as u32).hash(), 0);
        let transaction = spend(6810 + index as u32, std::slice::from_ref(&point), &[]);
        accept(&store, transaction, 1000, 1, status);
        accepted.insert(point);
    }
    let before = cache.capture(&store).unwrap();
    assert_eq!(*before.inputs, accepted);
    assert_eq!(before.tip_hash, store.snapshot().1.tip_hash());

    let unresolved = entry(
        &store,
        spend(6820, &[OutPoint::default()], &[]),
        Source::Local,
    );
    insert(&store, Arc::clone(&unresolved));
    let waiting = replace(&store, unresolved, Phase::Waiting(BTreeSet::new()));
    replace(
        &store,
        waiting,
        Phase::Replaced(RecoveryTriggers::RetryInputs(BTreeSet::new())),
    );
    let unchanged = cache.capture(&store).unwrap();
    assert!(Arc::ptr_eq(&before.inputs, &unchanged.inputs));

    let owner = store.capture_accepted().owners.pop().unwrap();
    let retained = Arc::downgrade(&owner);
    let removed: Vec<_> = owner.transaction.input_pts_iter().collect();
    let mut removal = Plan::new(store.snapshot().0, Class::Trusted, Default::default());
    removal.edit(Some(owner), None, None).unwrap();
    store.apply(removal).unwrap();
    let after = cache.capture(&store).unwrap();
    for point in removed {
        accepted.remove(&point);
    }
    assert_eq!(*after.inputs, accepted);
    assert!(
        retained.upgrade().is_none(),
        "snapshots retain no owner or VM state"
    );
    assert_eq!(
        before.inputs.len(),
        3,
        "published snapshots remain immutable"
    );

    store
        .apply(super::super::chain::clear(&store, None, ClearScope::All).unwrap())
        .unwrap();
    assert!(cache.capture(&store).unwrap().inputs.is_empty());
}

#[test]
fn input_snapshot_tracks_chain_changes_without_membership_changes() {
    let store = store();
    let cache = InputSnapshotCache::default();
    let before = cache.capture(&store).unwrap();
    let Captured {
        view,
        snapshot: base,
        reads,
        ..
    } = store.capture_accepted();
    let backing = ckb_test_chain_utils::MockStore::default();
    let next = Arc::new(Snapshot::new(
        base.tip_header().as_advanced_builder().nonce(1u128).build(),
        base.total_difficulty().clone(),
        base.epoch_ext().clone(),
        backing.store().get_snapshot(),
        ckb_proposal_table::ProposalView::default(),
        base.cloned_consensus(),
    ));
    let tip_hash = next.tip_hash();
    let mut plan = Plan::new(view, Class::Critical, reads);
    plan.chain(next, []);
    store.apply(plan).unwrap();
    let after = cache.capture(&store).unwrap();
    assert_ne!(before.tip_hash, after.tip_hash);
    assert_eq!(after.tip_hash, tip_hash);
    assert_eq!(before.inputs, after.inputs);
}

#[test]
fn optional_replacement_fee_overflow_keeps_transaction_and_status_visible() {
    let store = store();
    let hash = accept(&store, tx(6000), u64::MAX, 7, Status::Pending);
    let found = transaction(&store, &hash, &rbf_config()).unwrap().unwrap();
    assert_eq!(
        transaction_status(&store, &hash),
        Some((TxStatus::Pending, Some(7)))
    );
    assert_eq!(found.fee, Some(Capacity::shannons(u64::MAX)));
    assert_eq!(found.min_replace_fee, None);
    assert_eq!(
        detail(&store, &hash, &rbf_config())
            .unwrap()
            .descendants_count,
        0
    );
}

#[test]
fn descendant_fee_saturation_remains_exact_after_removing_one_reader() {
    let store = store();
    let parent = output_tx(6001);
    let point = OutPoint::new(parent.hash(), 0);
    let readers = [
        spend(6002, &[], std::slice::from_ref(&point)),
        spend(6003, &[], &[point]),
    ];
    let parent_hash = accept(&store, parent.clone(), 1000, 1, Status::Pending);
    let reader_fee = u64::MAX / 2 + 1000;
    for reader in &readers {
        accept(&store, reader.clone(), reader_fee, 1, Status::Pending);
    }
    let config = rbf_config();
    assert_eq!(
        transaction(&store, &parent_hash, &config)
            .unwrap()
            .unwrap()
            .min_replace_fee,
        None
    );
    assert_eq!(
        detail(&store, &parent_hash, &config)
            .unwrap()
            .descendants_count,
        2
    );
    let before = store.point(&readers[1].hash()).1.unwrap();
    let mut plan = Plan::new(store.snapshot().0, Class::Trusted, Default::default());
    plan.edit(Some(before), None, None).unwrap();
    store.apply(plan).unwrap();
    let expected = Capacity::shannons(1000 + reader_fee)
        .safe_add(
            config
                .min_rbf_rate
                .fee(parent.data().serialized_size_in_block() as u64),
        )
        .unwrap();
    assert_eq!(
        transaction(&store, &parent_hash, &config)
            .unwrap()
            .unwrap()
            .min_replace_fee,
        Some(expected)
    );
    assert_eq!(
        detail(&store, &parent_hash, &config)
            .unwrap()
            .descendants_count,
        1
    );
    assert_eq!(
        entry_info(&store, &config).unwrap().pending[&parent_hash].descendants_size,
        (parent.data().serialized_size_in_block() + readers[0].data().serialized_size_in_block())
            as u64
    );
}

#[test]
fn history_is_hidden_from_live_queries_but_exposed_in_conflicted_entries() {
    let store = store();
    let victim = entry(&store, tx(6004), Source::Local);
    let victim = victim.with_phase(Phase::Replaced(RecoveryTriggers::RetryInputs(
        BTreeSet::new(),
    )));
    insert(&store, Arc::clone(&victim));
    let pending = entry(&store, tx(6005), Source::Local);
    insert(&store, Arc::clone(&pending));
    let accepted = accept(&store, tx(6006), 1000, 9, Status::Pending);
    assert!(transaction_status(&store, &victim.hash()).is_none());
    assert!(
        transaction(&store, &victim.hash(), &config())
            .unwrap()
            .is_none()
    );
    assert!(
        transaction(&store, &pending.hash(), &config())
            .unwrap()
            .is_none()
    );
    assert_eq!(ids(&store).pending, vec![accepted]);
    let info = summary(&store, &config()).unwrap();
    assert_eq!(info.pending_size, 1);
    assert_eq!(info.verify_queue_size, 1);
    assert_eq!(info.orphan_size, 0);
    assert_eq!(
        entry_info(&store, &config()).unwrap().conflicted,
        vec![victim.hash()]
    );
    assert_eq!(
        store.filter_fresh_proposals(vec![
            victim.proposal(),
            pending.proposal(),
            tx(6007).proposal_short_id()
        ]),
        vec![tx(6007).proposal_short_id()]
    );
}

#[test]
fn fresh_proposal_filter_preserves_absent_id_order_and_duplicates_across_all_phases() {
    let store = store();
    let mut present = Vec::new();
    for (index, status) in [Status::Pending, Status::Gap, Status::Proposed]
        .into_iter()
        .enumerate()
    {
        let hash = accept(&store, tx(6600 + index as u32), 1, 1, status);
        present.push(ProposalShortId::from_tx_hash(&hash));
    }
    let resolving = entry(&store, tx(6603), Source::Local);
    let verifying = entry(&store, tx(6604), Source::Local);
    let verifying = verifying.with_phase(Phase::Verify(Arc::clone(
        verification_fixture(&store, &verifying, 1, 1, Status::Pending).resolved(),
    )));
    let waiting =
        entry(&store, tx(6605), Source::Local).with_phase(Phase::Waiting(BTreeSet::from([
            crate::authority::model::DependencyKey::Cell(OutPoint::new(tx(6606).hash(), 0)),
        ])));
    let replaced = entry(&store, tx(6607), Source::Local).with_phase(Phase::Replaced(
        RecoveryTriggers::RetryInputs(BTreeSet::new()),
    ));
    for owner in [resolving, verifying, waiting, replaced] {
        present.push(owner.proposal());
        insert(&store, owner);
    }
    let unknown = [tx(6608).proposal_short_id(), tx(6609).proposal_short_id()];
    let mut ids = vec![unknown[1].clone(), present[0].clone(), unknown[0].clone()];
    ids.extend(present);
    ids.push(unknown[1].clone());
    assert_eq!(
        store.filter_fresh_proposals(ids),
        vec![unknown[1].clone(), unknown[0].clone(), unknown[1].clone()]
    );
    assert!(store.filter_fresh_proposals(Vec::new()).is_empty());
}

#[test]
fn live_overlay_compact_payloads_and_raw_hash_cycle_lookup_keep_their_boundaries() {
    let store = store();
    let parent = TransactionBuilder::default()
        .version(6008u32)
        .output(CellOutput::default())
        .output_data(Bytes::from_static(b"spent").pack())
        .output(CellOutput::default())
        .output_data(Bytes::from_static(b"live").pack())
        .build();
    let hash = accept(&store, parent.clone(), 1, 17, Status::Pending);
    accept(
        &store,
        spend(6009, &[OutPoint::new(hash.clone(), 0)], &[]),
        1,
        1,
        Status::Pending,
    );
    let pending = entry(&store, output_tx(6010), Source::Local);
    insert(&store, Arc::clone(&pending));
    assert!(live_cell(&store, &OutPoint::new(hash.clone(), 0), true).is_unknown());
    let CellStatus::Live(cell) = live_cell(&store, &OutPoint::new(hash.clone(), 1), true) else {
        panic!("unspent output is live")
    };
    let CellStatus::Live(without_data) = live_cell(&store, &OutPoint::new(hash.clone(), 1), false)
    else {
        panic!("unspent output is live without data")
    };
    assert_eq!(without_data.cell_output, cell.cell_output);
    assert_eq!(without_data.data_bytes, cell.data_bytes);
    assert_eq!(without_data.out_point, cell.out_point);
    assert_eq!(cell.out_point, OutPoint::new(hash.clone(), 1));
    assert_eq!(cell.data_bytes, 4);
    assert_eq!(
        cell.mem_cell_data_hash,
        Some(CellOutput::calc_data_hash(b"live"))
    );
    assert!(without_data.mem_cell_data.is_none());
    assert!(without_data.mem_cell_data_hash.is_none());
    assert_eq!(cell.mem_cell_data.unwrap().as_ref(), b"live");
    assert!(live_cell(&store, &OutPoint::new(pending.hash(), 0), true).is_unknown());
    let compact = compact_transactions(&store, &[parent.proposal_short_id(), pending.proposal()]);
    assert_eq!(compact.len(), 2);
    assert_eq!(compact[&pending.proposal()], *pending.transaction);
    let mut alternate = hash.as_slice().to_vec();
    alternate[31] ^= 1;
    let alternate = Byte32::from_slice(&alternate).unwrap();
    assert_eq!(
        ProposalShortId::from_tx_hash(&alternate),
        parent.proposal_short_id()
    );
    assert!(
        transaction(&store, &alternate, &config())
            .unwrap()
            .is_none()
    );
    assert!(live_cell(&store, &OutPoint::new(alternate.clone(), 1), true).is_unknown());
    assert!(accepted_with_cycles(&store, &[alternate]).is_empty());
    assert_eq!(accepted_with_cycles(&store, &[hash]), vec![(parent, 17)]);
}

#[test]
fn verification_owner_and_declared_cycles_are_hidden_until_acceptance() {
    let base = crate::test_support::genesis_snapshot();
    let transaction = tx(6011);
    let backing = ckb_test_chain_utils::MockStore::default();
    let snapshot = Arc::new(Snapshot::new(
        base.tip_header().clone(),
        base.total_difficulty().clone(),
        base.epoch_ext().clone(),
        backing.store().get_snapshot(),
        ckb_proposal_table::ProposalView::new([], [transaction.proposal_short_id()]),
        base.cloned_consensus(),
    ));
    let store = Store::new(snapshot, &config()).unwrap();
    let candidate = entry(
        &store,
        transaction,
        super::super::ingress::remote_source(5.into(), 42).unwrap(),
    );
    let candidate = candidate.with_phase(Phase::Verify(Arc::clone(
        verification_fixture(&store, &candidate, 1, 42, Status::Proposed).resolved(),
    )));
    insert(&store, Arc::clone(&candidate));
    assert_eq!(summary(&store, &config()).unwrap().verify_queue_size, 1);
    let mut selected = store
        .pop(WorkStage::Verify, WorkSelection::Any)
        .unwrap()
        .unwrap();
    assert_eq!(summary(&store, &config()).unwrap().verify_queue_size, 0);
    assert!(transaction_status(&store, &candidate.hash()).is_none());
    assert!(
        super::transaction(&store, &candidate.hash(), &config())
            .unwrap()
            .is_none()
    );
    let (plan, reject) = admission(&store, &candidate, 1, 42, Status::Proposed, &config()).unwrap();
    assert!(reject.is_none());
    store.apply(plan).unwrap();
    selected.mark_handled();
    assert_eq!(
        transaction_status(&store, &candidate.hash()),
        Some((TxStatus::Proposed, Some(42)))
    );
    assert_eq!(
        super::transaction(&store, &candidate.hash(), &config())
            .unwrap()
            .unwrap()
            .tx_status,
        TxStatus::Proposed
    );
    let info = summary(&store, &config()).unwrap();
    assert_eq!(info.proposed_size, 1);
    assert_eq!(info.verify_queue_size, 0);
}

#[test]
fn waiting_dependencies_remain_rpc_orphans_across_proposal_windows() {
    use crate::authority::{ingress, jobs};
    use ckb_proposal_table::ProposalView;

    let (backing, base) = chain_store(Arc::new(ckb_test_chain_utils::always_success_consensus()));
    let transaction = funded_tx(OutPoint::new(tx(6012).hash(), 0), 19_999_999_000);
    let proposal = transaction.proposal_short_id();
    for proposals in [
        ProposalView::default(),
        ProposalView::new([proposal.clone()], []),
        ProposalView::new([], [proposal]),
    ] {
        let snapshot = Arc::new(Snapshot::new(
            base.tip_header().clone(),
            base.total_difficulty().clone(),
            base.epoch_ext().clone(),
            backing.store().get_snapshot(),
            proposals,
            base.cloned_consensus(),
        ));
        let store = Store::new(snapshot, &config()).unwrap();
        let candidate = entry(
            &store,
            transaction.clone(),
            ingress::remote_source(5.into(), 42).unwrap(),
        );
        let jobs::Resolution::Waiting(keys, reads) =
            jobs::resolve(&store, &candidate, &config()).unwrap()
        else {
            panic!("the missing producer must prevent admission in every proposal window");
        };
        let mut plan = Plan::new(store.snapshot().0, Class::Remote, reads);
        plan.edit(None, Some(candidate.with_phase(Phase::Waiting(keys))), None)
            .unwrap();
        store.apply(plan).unwrap();
        let info = summary(&store, &config()).unwrap();
        assert_eq!(
            (info.pending_size, info.proposed_size, info.orphan_size),
            (0, 0, 1)
        );
        assert_eq!(
            detail(&store, &candidate.hash(), &config())
                .unwrap()
                .entry_status,
            "unknown"
        );
        assert!(transaction_status(&store, &candidate.hash()).is_none());
        assert!(ids(&store).pending.is_empty());
        assert!(ids(&store).proposed.is_empty());
    }
}

#[test]
fn canonical_storage_fallback_and_fee_target_validation_remain_available() {
    let store = Store::new(chain_snapshot(), &config()).unwrap();
    assert!(
        live_cell(
            &store,
            &ckb_test_chain_utils::create_always_success_out_point(),
            true
        )
        .is_live()
    );
    assert!(estimate_fee(&store, &config(), crate::constants::MIN_ESTIMATE_TARGET - 1).is_err());
    assert!(estimate_fee(&store, &config(), crate::constants::MAX_ESTIMATE_TARGET + 1).is_err());
    assert_eq!(
        estimate_fee(&store, &config(), crate::constants::MIN_ESTIMATE_TARGET).unwrap(),
        FeeRate::zero()
    );
}

#[test]
fn fee_samples_match_accepted_rpc_entries() {
    let store = store();
    for (index, status) in [Status::Pending, Status::Gap, Status::Proposed]
        .into_iter()
        .enumerate()
    {
        accept(
            &store,
            tx(7200 + index as u32),
            1000 + index as u64,
            9,
            status,
        );
    }
    insert(&store, entry(&store, tx(7210), Source::Local));
    let projection = entry_info(&store, &config()).unwrap();
    let mut expected: Vec<_> = projection
        .pending
        .into_values()
        .chain(projection.proposed.into_values())
        .map(|value| FeeSample::new(value.size as usize, value.cycles, value.fee))
        .collect();
    expected.sort_unstable();
    let mut actual = fee_samples(&store);
    actual.sort_unstable();
    assert_eq!(actual, expected);
}

#[test]
fn fee_estimate_preserves_capacity_boundaries_block_targets_and_minimum_rate() {
    let size = tx(0).data().serialized_size_in_block();
    let cycles = 1_000;
    let config = TxPoolConfig {
        min_fee_rate: FeeRate::from_u64(42),
        ..config()
    };
    // Six equally sized independent entries have descending fees 6,000..1,000.
    // At an exact three-entry limit, fee 4,000 sets the first threshold and
    // 2,000 the second. With any extra capacity the first threshold is 3,000.
    for (block_bytes, block_cycles, blocks, threshold_fee) in [
        (10_000, 10_000_000, 1, None),
        (3 * size, 10_000_000, 1, Some(4_000)),
        (3 * size + 1, 10_000_000, 1, Some(3_000)),
        (3 * size, 10_000_000, 2, Some(2_000)),
        (3 * size, 10_000_000, 3, None),
        (10_000, 3 * cycles, 1, Some(4_000)),
        (10_000, 3 * cycles + 1, 1, Some(3_000)),
        (10_000, 3 * cycles, 2, Some(2_000)),
        (5 * size, 3 * cycles, 1, Some(4_000)),
        (3 * size, 5 * cycles, 1, Some(4_000)),
    ] {
        let consensus = ckb_chain_spec::consensus::ConsensusBuilder::default()
            .max_block_bytes(block_bytes as u64)
            .max_block_cycles(block_cycles)
            .build();
        let target = consensus.tx_proposal_window().closest() + blocks;
        let (_, snapshot) = chain_store(Arc::new(consensus));
        let store = Store::new(snapshot, &config).unwrap();
        for rank in 1..=6 {
            accept(
                &store,
                tx(rank),
                u64::from(rank) * 1_000,
                cycles,
                Status::Pending,
            );
        }
        let expected = threshold_fee.map_or(config.min_fee_rate, |fee| {
            FeeRate::calculate(
                Capacity::shannons(fee),
                get_transaction_weight(size, cycles),
            )
        });
        assert_eq!(
            estimate_fee(&store, &config, target).unwrap(),
            expected,
            "bytes={block_bytes}, cycles={block_cycles}, blocks={blocks}"
        );
    }
}

#[test]
fn replacement_fee_recapture_hides_a_target_returned_to_resolution() {
    let store = store();
    let hash = accept(&store, tx(6012), 1000, 17, Status::Pending);
    let (old_snapshot, old) = store.point(&hash);
    let old = old.unwrap();
    assert_eq!(
        live_transaction(&old, &old_snapshot).unwrap().cycles,
        Some(17)
    );
    replace(&store, old, Phase::Resolve);
    // The accepted point observation is stale after the target returns to
    // resolution. Retaining its payload does not retain a public acceptance.
    assert!(
        transaction_with_replacement_fee(&store, &hash, &rbf_config())
            .unwrap()
            .is_none()
    );
    assert!(transaction_status(&store, &hash).is_none());
    assert_eq!(summary(&store, &config()).unwrap().verify_queue_size, 1);
}

#[test]
fn replacement_fee_walks_only_target_descendants_and_counts_shared_fanout_once() {
    let store = store();
    let parent = output_tx(6020);
    let hash = accept(&store, parent.clone(), 10, 1, Status::Pending);
    let point = OutPoint::new(hash.clone(), 0);
    let mut readers = Vec::new();
    // A query must include more descendants than an ordinary mutation may remove.
    for nonce in 0..=crate::constants::MAX_POOL_MUTATION_CANDIDATES {
        let reader = spend(6100 + nonce as u32, &[], std::slice::from_ref(&point));
        accept(&store, reader.clone(), 1, 1, Status::Pending);
        readers.push(reader);
    }
    let shared = spend(
        6300,
        &[],
        &[
            OutPoint::new(readers[0].hash(), 0),
            OutPoint::new(readers[1].hash(), 0),
        ],
    );
    accept(&store, shared, 7, 1, Status::Pending);
    accept(&store, output_tx(6301), u64::MAX, 1, Status::Pending);
    // Deliberately narrow the query's ancestor policy: a full aggregate pass
    // would reject this existing graph. Descendant fee lookup has no such walk.
    let configuration = TxPoolConfig {
        max_ancestors_count: 1,
        ..rbf_config()
    };
    let (_, captured) = store.capture_descendants(&hash).unwrap();
    assert_eq!(captured.len(), readers.len() + 2);
    assert_eq!(captured.first().unwrap().hash(), hash);
    assert!(
        captured
            .iter()
            .all(|entry| entry.hash() != output_tx(6301).hash())
    );
    let found = transaction(&store, &hash, &configuration).unwrap().unwrap();
    assert_eq!(
        found.min_replace_fee,
        Some(Capacity::shannons(
            10 + readers.len() as u64
                + 7
                + configuration
                    .min_rbf_rate
                    .fee(parent.data().serialized_size_in_block() as u64)
                    .as_u64()
        ))
    );
}

#[test]
fn detail_rank_preserves_ordering_equivalence_arrival_hash_and_status() {
    let store = store();
    let mut children = Vec::new();
    for (nonce, parent_fee, child_fee, status) in [
        (6400, 0, 200, Status::Pending),
        (6410, 10, 190, Status::Gap),
    ] {
        let parent = accept(&store, output_tx(nonce), parent_fee, 1, Status::Pending);
        children.push(accept(
            &store,
            spend(nonce + 1, &[OutPoint::new(parent, 0)], &[]),
            child_fee,
            1,
            status,
        ));
    }
    let proposed = accept(&store, output_tx(6420), 10000, 1, Status::Proposed);
    let owners = store.capture_accepted().owners;
    let members: Members = owners
        .into_iter()
        .map(|entry| (entry.hash(), entry))
        .collect();
    let totals = membership::aggregates(&members, config().max_ancestors_count).unwrap();
    let keys: Vec<_> = children
        .iter()
        .map(|hash| score(members[hash].accepted().unwrap(), totals[hash].ancestors).unwrap())
        .collect();
    assert_ne!(keys[0], keys[1]);
    assert_eq!(keys[0].cmp(&keys[1]), std::cmp::Ordering::Equal);
    for (index, hash) in children.iter().enumerate() {
        let info = detail(&store, hash, &config()).unwrap();
        assert_eq!(info.rank_in_pending, index + 1, "arrival breaks score ties");
        assert_eq!((info.pending_count, info.proposed_count), (4, 1));
    }
    for hash in &children {
        let old = store.point(hash).1.unwrap();
        let after = Arc::new(Entry {
            arrival: 100,
            ..old.as_ref().clone()
        });
        let mut plan = Plan::new(store.snapshot().0, Class::Trusted, Default::default());
        plan.edit(Some(old), Some(after), None).unwrap();
        store.apply(plan).unwrap();
    }
    children.sort_unstable();
    for (index, hash) in children.iter().enumerate() {
        assert_eq!(
            detail(&store, hash, &config()).unwrap().rank_in_pending,
            index + 1
        );
    }
    let info = detail(&store, &proposed, &config()).unwrap();
    assert_eq!(info.rank_in_pending, 0);
    assert_eq!((info.pending_count, info.proposed_count), (4, 1));
    assert_eq!(
        detail(&store, &tx(6421).hash(), &config())
            .unwrap()
            .entry_status,
        "unknown"
    );
}

#[test]
fn summary_tracks_phase_and_snapshot_changes_without_retaining_removed_maximum() {
    let store = store();
    let a = accept(&store, tx(6490), 1, 7, Status::Pending);
    let before = store.point(&a).1.unwrap();
    let mut accepted = before.accepted().unwrap().clone();
    accepted.forced_status = None;
    accepted.timestamp = 77;
    let a = replace(&store, before, Phase::Accepted(accepted));
    let b = accept(&store, tx(6491), 1, 11, Status::Proposed);
    let before = store.point(&b).1.unwrap();
    let mut accepted = before.accepted().unwrap().clone();
    accepted.timestamp = 88;
    let b = replace(&store, before, Phase::Accepted(accepted));
    let waiting = entry(&store, tx(6492), Source::Local).with_phase(Phase::Waiting(
        [super::super::model::DependencyKey::Cell(OutPoint::new(
            tx(6493).hash(),
            0,
        ))]
        .into(),
    ));
    insert(&store, Arc::clone(&waiting));
    let bytes = a.accepted().unwrap().size + b.accepted().unwrap().size;
    let info = summary(&store, &config()).unwrap();
    assert_eq!(
        (info.pending_size, info.proposed_size, info.orphan_size),
        (1, 1, 1)
    );
    assert_eq!(
        (
            info.total_tx_size,
            info.total_tx_cycles,
            info.last_txs_updated_at
        ),
        (bytes, 18, 88)
    );

    // The accepted Arc is unchanged by this snapshot-only publication.
    let Captured {
        view,
        snapshot: base,
        reads,
        ..
    } = store.capture_all();
    let backing = ckb_test_chain_utils::MockStore::default();
    let next = Arc::new(Snapshot::new(
        base.tip_header().clone(),
        base.total_difficulty().clone(),
        base.epoch_ext().clone(),
        backing.store().get_snapshot(),
        ckb_proposal_table::ProposalView::new([], [a.proposal()]),
        base.cloned_consensus(),
    ));
    let mut plan = Plan::new(view, Class::Critical, reads);
    plan.chain(next, []);
    store.apply(plan).unwrap();
    assert!(Arc::ptr_eq(&store.point(&a.hash()).1.unwrap(), &a));
    let info = summary(&store, &config()).unwrap();
    assert_eq!(
        (info.pending_size, info.proposed_size, info.orphan_size),
        (0, 2, 1)
    );
    assert_eq!(
        (
            info.total_tx_size,
            info.total_tx_cycles,
            info.last_txs_updated_at
        ),
        (bytes, 18, 88)
    );

    replace(&store, waiting, Phase::Resolve);
    let mut removal = Plan::new(store.snapshot().0, Class::Trusted, Default::default());
    removal.edit(Some(Arc::clone(&b)), None, None).unwrap();
    store.apply(removal).unwrap();
    let info = summary(&store, &config()).unwrap();
    assert_eq!(
        (
            info.pending_size,
            info.proposed_size,
            info.orphan_size,
            info.verify_queue_size
        ),
        (0, 1, 0, 1)
    );
    assert_eq!(
        (
            info.total_tx_size,
            info.total_tx_cycles,
            info.last_txs_updated_at
        ),
        (a.accepted().unwrap().size, 7, 77)
    );

    let mut stale = Plan::new(store.snapshot().0, Class::Trusted, Default::default());
    stale.edit(Some(b), None, None).unwrap();
    assert!(matches!(store.apply(stale), Err(Error::Stale)));
    assert_eq!(summary(&store, &config()).unwrap().proposed_size, 1);
    store
        .apply(super::super::chain::clear(&store, None, ClearScope::All).unwrap())
        .unwrap();
    let info = summary(&store, &config()).unwrap();
    assert_eq!(
        (
            info.pending_size,
            info.proposed_size,
            info.orphan_size,
            info.verify_queue_size
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(
        (
            info.total_tx_size,
            info.total_tx_cycles,
            info.last_txs_updated_at
        ),
        (0, 0, 0)
    );
}

#[test]
fn replacement_fee_zero_increment_keeps_saturated_descendant_fee() {
    let store = store();
    let parent = output_tx(6590);
    let hash = accept(&store, parent, u64::MAX - 1, 1, Status::Pending);
    accept(
        &store,
        spend(6591, &[], &[OutPoint::new(hash.clone(), 0)]),
        1,
        1,
        Status::Pending,
    );
    // Each child's ancestor sum fits u64, while their shared parent's total
    // descendant fee exceeds u64. Both admissions are individually valid.
    accept(
        &store,
        spend(6592, &[], &[OutPoint::new(hash.clone(), 0)]),
        1,
        1,
        Status::Pending,
    );
    let configuration = TxPoolConfig {
        min_rbf_rate: FeeRate::from_u64(1),
        ..config()
    };
    let size = store.point(&hash).1.unwrap().accepted().unwrap().size;
    assert_eq!(
        configuration.min_rbf_rate.fee(size as u64),
        Capacity::zero()
    );
    let found = transaction(&store, &hash, &configuration).unwrap().unwrap();
    assert_eq!(found.min_replace_fee, Some(Capacity::shannons(u64::MAX)));
}
