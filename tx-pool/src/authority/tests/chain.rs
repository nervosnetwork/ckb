use super::*;
use crate::authority::{ingress, model::Status, tests::common::*};
use ckb_proposal_table::ProposalView;
use ckb_test_chain_utils::MockStore;
use ckb_types::{
    bytes::Bytes,
    core::{BlockBuilder, BlockView},
    packed::{Byte32, OutPoint, ProposalShortId},
    prelude::*,
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
    ChainReorgArgs::Detailed {
        detached_blocks: VecDeque::new(),
        snapshot: snapshot(&base, &block, HashSet::new(), HashSet::new()),
        attached_blocks: [block].into(),
    }
}
fn apply(store: &Store, command: &ChainReorgArgs) {
    let _pause = store.begin_chain().unwrap();
    let plan = reconcile(store, command, &config()).unwrap();
    store.apply(plan).unwrap();
}
fn accepted_hashes(store: &Store) -> BTreeSet<Byte32> {
    store
        .capture(true)
        .2
        .into_iter()
        .map(|entry| entry.hash())
        .collect()
}

#[test]
fn discarded_clear_and_stale_chain_plans_expose_no_prefix() {
    let store = store();
    let hash = accept(&store, output_tx(5001), 1, 1, Status::Pending);
    let (view, old_snapshot) = store.snapshot();
    let discarded = clear(&store, None, false).unwrap();
    drop(discarded);
    assert_eq!(store.snapshot().0, view);
    assert!(Arc::ptr_eq(&store.snapshot().1, &old_snapshot));
    assert_eq!(accepted_hashes(&store), [hash].into());
    let stale = reconcile(&store, &command(&store, Vec::new()), &config()).unwrap();
    accept(&store, output_tx(5002), 1, 1, Status::Pending);
    let before = accepted_hashes(&store);
    assert!(matches!(store.apply(stale), Err(Error::Stale)));
    assert_eq!(accepted_hashes(&store), before);
    assert_eq!(store.snapshot().0, view);
}

#[test]
fn clear_pipeline_preserves_accepted_membership_but_invalidates_old_view() {
    let store = store();
    let accepted = accept(&store, output_tx(5003), 1, 1, Status::Pending);
    let waiting = entry(&store, tx(5004), Source::Recovery);
    insert(&store, Arc::clone(&waiting));
    let old = store.snapshot().0;
    store.apply(clear(&store, None, true).unwrap()).unwrap();
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
        let (plan, reject) = admission(
            &store,
            &replacement,
            10_000,
            1,
            Status::Pending,
            &configuration,
        )
        .unwrap();
        assert!(reject.is_none());
        store.apply(plan).unwrap();
        let history = store.point(&old).1.unwrap();
        assert_eq!(history.source, Source::Recovery);
        assert!(matches!(history.phase, Phase::Replaced { .. }));
        let expired = store.expired(std::time::Instant::now(), u64::MAX, 32);
        assert!(expired.iter().any(|entry| entry.hash() == parent));
        assert!(!expired.iter().any(|entry| entry.hash() == old));
        apply(&store, &command(&store, Vec::new()));
        assert!(Arc::ptr_eq(&store.point(&old).1.unwrap(), &history));

        if wake_before_clear {
            let current = store.point(&replacement.hash()).1.unwrap();
            store
                .apply(membership::removal(&store, &current, &configuration, None).unwrap())
                .unwrap();
            let mut cursor = None;
            let page = crate::authority::waiting::wake(&store, &mut cursor)
                .unwrap()
                .unwrap();
            store.apply(page).unwrap();
            let recovered = store.point(&old).1.unwrap();
            assert_eq!(recovered.source, Source::Recovery);
            assert!(matches!(recovered.phase, Phase::Resolve));
            assert!(recovered.accepted().is_none());
        }
        store.apply(clear(&store, None, true).unwrap()).unwrap();
        assert!(store.point(&old).1.is_none());
        assert!(store.point(&parent).1.unwrap().accepted().is_some());
        assert!(!store.is_faulted());
    }
}

#[test]
fn mismatched_attached_tip_is_rejected_before_any_mutation() {
    let store = store();
    let hash = accept(&store, output_tx(5005), 1, 1, Status::Pending);
    let command = ChainReorgArgs::Detailed {
        detached_blocks: VecDeque::new(),
        attached_blocks: [BlockBuilder::default().build()].into(),
        snapshot: store.snapshot().1,
    };
    assert!(matches!(
        reconcile(&store, &command, &config()),
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
    assert_eq!(committed, vec![(parent.proposal_short_id(), ph)]);
}

#[test]
fn chain_commit_preserves_supplied_order_for_colliding_compact_ids() {
    let store = store();
    let id = ProposalShortId::new([9; 10]);
    let first = Byte32::new([1; 32]);
    let second = Byte32::new([2; 32]);
    let command = command(&store, Vec::new());
    let _pause = store.begin_chain().unwrap();
    let mut plan = reconcile(&store, &command, &config()).unwrap();
    // Inject a compact collision at the same checked boundary that receives
    // ordered block transaction facts; constructing a hash collision is unnecessary.
    plan.committed = vec![(id.clone(), first), (id.clone(), second.clone())];
    store.apply(plan).unwrap();
    let (_, live, committed) = store.compact_lookup(std::slice::from_ref(&id));
    assert!(live.is_empty());
    assert_eq!(committed, vec![(id, second)]);
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
        Phase::Waiting([DependencyKey::Cell(OutPoint::new(tx(5997).hash(), 0))].into()),
    );
    apply(&store, &command(&store, vec![spend(5014, &[input], &[])]));
    assert!(store.point(&waiting.hash()).1.is_none());
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
    let command = ChainReorgArgs::Detailed {
        detached_blocks: [detached].into(),
        attached_blocks: VecDeque::new(),
        snapshot: store.snapshot().1,
    };
    apply(&store, &command);
    let restored = store.point(&parent.hash()).1.unwrap();
    assert_eq!(restored.transaction.witness_hash(), changed.witness_hash());
    assert!(matches!(restored.source, Source::Recovery));
    assert!(matches!(restored.phase, Phase::Resolve));
    assert!(restored.arrival < store.point(&child.hash()).1.unwrap().arrival);
}

#[test]
fn reorg_moves_context_sensitive_accepted_closure_to_recovery() {
    let store = store();
    let root = accept(&store, output_tx(5017), 1, 1, Status::Pending);
    let owner = store.point(&root).1.unwrap();
    let mut value = owner.accepted().unwrap().clone();
    value.context_sensitive = true;
    replace(&store, owner, Phase::Accepted(value));
    let child = accept(
        &store,
        spend(5018, &[OutPoint::new(root.clone(), 0)], &[]),
        1,
        1,
        Status::Pending,
    );
    apply(
        &store,
        &ChainReorgArgs::Detailed {
            detached_blocks: [BlockBuilder::default().transaction(tx(0)).build()].into(),
            attached_blocks: VecDeque::new(),
            snapshot: store.snapshot().1,
        },
    );
    for hash in [root, child] {
        let owner = store.point(&hash).1.unwrap();
        assert!(matches!(owner.source, Source::Recovery));
        assert!(matches!(owner.phase, Phase::Resolve));
    }
    assert!(accepted_hashes(&store).is_empty());
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
        let command = ChainReorgArgs::Detailed {
            detached_blocks: [detached].into(),
            attached_blocks: [attached].into(),
            snapshot: Arc::clone(&next),
        };
        let _pause = store.begin_chain().unwrap();
        let plan = if bounded {
            recover_bounded(&store, &command)
        } else {
            reconcile(&store, &command, &config())
        }
        .unwrap();
        store.apply(plan).unwrap();
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
        assert_eq!(store.capture(false).2.len(), 2);
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
        let apply = |store: &Store, command: &ChainReorgArgs| {
            let plan = if bounded {
                recover_bounded(store, command)
            } else {
                reconcile(store, command, &config())
            }
            .unwrap();
            store.apply(plan).unwrap();
        };
        apply(
            &store,
            &ChainReorgArgs::Detailed {
                detached_blocks: VecDeque::new(),
                attached_blocks: VecDeque::new(),
                snapshot: proposed,
            },
        );
        let promoted = store.point(&owner.hash()).1.unwrap();
        assert!(matches!(
            promoted.source,
            Source::Proposal { remote: Some(_) }
        ));
        assert_eq!(promoted.source.residency_peer(), source.residency_peer());
        assert_eq!(promoted.source.deadline(), source.deadline());
        apply(
            &store,
            &ChainReorgArgs::Detailed {
                detached_blocks: VecDeque::new(),
                attached_blocks: VecDeque::new(),
                snapshot: base,
            },
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
    let command = ChainReorgArgs::Detailed {
        detached_blocks: [BlockBuilder::default().transaction(tx(0)).build()].into(),
        attached_blocks: VecDeque::new(),
        snapshot: store.snapshot().1,
    };
    let _pause = store.begin_chain().unwrap();
    let detailed = reconcile(&store, &command, &configuration).unwrap();
    assert!(matches!(store.apply(detailed), Err(Error::Full(_))));
    assert_eq!(accepted_hashes(&store).len(), 20);
    store
        .apply(recover_bounded(&store, &command).unwrap())
        .unwrap();
    let restored = store.capture(false).2;
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
        store
            .apply(
                ingress::prepare(
                    &store,
                    Arc::new(transaction.clone()),
                    Source::Proposal { remote: None },
                )
                .unwrap(),
            )
            .unwrap();
        let hash = transaction.hash();
        assert!(matches!(
            store.point(&hash).1.unwrap().phase,
            Phase::Resolve
        ));
        let base = store.snapshot().1;
        let update = |next| {
            let args = ChainReorgArgs::Detailed {
                detached_blocks: VecDeque::new(),
                attached_blocks: VecDeque::new(),
                snapshot: next,
            };
            let plan = if bounded {
                recover_bounded(&store, &args)
            } else {
                reconcile(&store, &args, &config())
            }
            .unwrap();
            store.apply(plan).unwrap();
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
    let command = ChainReorgArgs::Detailed {
        detached_blocks: [detached].into(),
        attached_blocks: [next.clone()].into(),
        snapshot: snapshot(&base, &next, HashSet::new(), HashSet::new()),
    };
    apply(&store, &command);
    for hash in [parent, child] {
        let owner = store.point(&hash).1.unwrap();
        assert!(matches!(owner.phase, Phase::Resolve));
        assert_eq!(owner.source, Source::Recovery);
    }
    assert!(Arc::ptr_eq(&stable, &store.point(&unrelated).1.unwrap()));
}

#[test]
fn script_hardfork_transition_requeues_the_current_accepted_proof_without_changing_payload() {
    use ckb_chain_spec::consensus::ConsensusBuilder;
    use ckb_types::{
        U256,
        core::{EpochNumberWithFraction, hardfork::HardForks},
    };
    let hardfork = HardForks::new_mirana();
    let boundary = hardfork.ckb2023.vm_version_2_and_syscalls_3();
    let consensus = Arc::new(
        ConsensusBuilder::default()
            .hardfork_switch(hardfork)
            .build(),
    );
    let at = |epoch| {
        let block = BlockBuilder::default()
            .number(1)
            .epoch(EpochNumberWithFraction::new(epoch, 0, 1000))
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
    let (_, old_snapshot) = at(boundary - 1);
    let store = Store::new(Arc::clone(&old_snapshot), &config()).unwrap();
    let hash = accept(&store, output_tx(8603), 1, 1, Status::Pending);
    let old = store.point(&hash).1.unwrap();
    let (block, new_snapshot) = at(boundary);
    assert_ne!(
        ScriptVerificationRules::from_env(&consensus, &environment(Status::Pending, &old_snapshot)),
        ScriptVerificationRules::from_env(&consensus, &environment(Status::Pending, &new_snapshot))
    );
    apply(
        &store,
        &ChainReorgArgs::Detailed {
            detached_blocks: Default::default(),
            attached_blocks: [block].into(),
            snapshot: new_snapshot,
        },
    );
    let current = store.point(&hash).1.unwrap();
    assert!(matches!(current.phase, Phase::Resolve));
    assert_eq!(current.source, Source::Recovery);
    assert!(Arc::ptr_eq(&old.transaction, &current.transaction));
    assert!(!Arc::ptr_eq(&old, &current));
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
    let plan = reconcile(&store, &command(&store, Vec::new()), &configuration).unwrap();
    assert!(plan.effects.iter().all(|effect| effect.callback.is_none()));
    store.apply(plan).unwrap();
    // A real accepted conflict still requires the old graph snapshot, including
    // its ancestor bound; the lazy gate must not suppress required computation.
    assert!(matches!(
        reconcile(
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
    let plan = reconcile(
        &store,
        &command(&store, vec![spend(6513, &[external], &[])]),
        &config(),
    )
    .unwrap();
    let callbacks: BTreeMap<_, _> = plan
        .effects
        .iter()
        .filter_map(|effect| match &effect.callback {
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
    let command = ChainReorgArgs::Detailed {
        detached_blocks: VecDeque::new(),
        snapshot: snapshot(
            &store.snapshot().1,
            &block,
            HashSet::new(),
            [child.proposal_short_id()].into(),
        ),
        attached_blocks: [block].into(),
    };
    let plan = reconcile(&store, &command, &config()).unwrap();
    let callbacks: Vec<_> = plan
        .effects
        .iter()
        .filter_map(|effect| match &effect.callback {
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
