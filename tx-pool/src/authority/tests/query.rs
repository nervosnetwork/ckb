use super::*;
use crate::authority::{
    model::{Phase, Source},
    notice::Class,
    queue::WorkStage,
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
    let mut plan = Plan::new(store.snapshot().0, Class::Trusted);
    plan.edit(Some(before), None).unwrap();
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
    let victim = victim.with_phase(Phase::Replaced {
        triggers: BTreeSet::new(),
        require_all: false,
    });
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
        fresh_proposals(
            &store,
            vec![
                victim.proposal(),
                pending.proposal(),
                tx(6007).proposal_short_id()
            ]
        ),
        vec![tx(6007).proposal_short_id()]
    );
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
        verified(&store, &candidate, 1, 42, Status::Proposed).resolved(),
    )));
    insert(&store, Arc::clone(&candidate));
    assert_eq!(summary(&store, &config()).unwrap().verify_queue_size, 1);
    let mut selected = store.pop(WorkStage::Verify, false).unwrap().unwrap();
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
    selected.complete();
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
    assert_eq!(
        estimate_fee(&store, &config(), crate::constants::MIN_ESTIMATE_TARGET).unwrap(),
        FeeRate::zero()
    );
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
    let (_, _, owners, _) = store.capture(true);
    let members: Members = owners
        .into_iter()
        .map(|entry| (entry.hash(), entry))
        .collect();
    let totals = membership::aggregates(&members, config().max_ancestors_count).unwrap();
    let keys: Vec<_> = children
        .iter()
        .map(|hash| score(members[hash].accepted().unwrap(), totals[hash].0).unwrap())
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
        let mut plan = Plan::new(store.snapshot().0, Class::Trusted);
        plan.edit(Some(old), Some(after)).unwrap();
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
    let (view, base, _, reads) = store.capture(false);
    let backing = ckb_test_chain_utils::MockStore::default();
    let next = Arc::new(Snapshot::new(
        base.tip_header().clone(),
        base.total_difficulty().clone(),
        base.epoch_ext().clone(),
        backing.store().get_snapshot(),
        ckb_proposal_table::ProposalView::new([], [a.proposal()]),
        base.cloned_consensus(),
    ));
    let mut plan = Plan::new(view, Class::Critical);
    plan.reads = reads;
    plan.snapshot = Some(next);
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
    let mut removal = Plan::new(store.snapshot().0, Class::Trusted);
    removal.edit(Some(Arc::clone(&b)), None).unwrap();
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

    let mut stale = Plan::new(store.snapshot().0, Class::Trusted);
    stale.edit(Some(b), None).unwrap();
    assert!(matches!(store.apply(stale), Err(Error::Stale)));
    assert_eq!(summary(&store, &config()).unwrap().proposed_size, 1);
    store
        .apply(super::super::chain::clear(&store, None, false).unwrap())
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
