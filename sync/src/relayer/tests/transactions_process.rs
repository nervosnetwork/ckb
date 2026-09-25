//! Negative production refinement witnesses for the pre-authority Remote handoff.

use crate::relayer::tests::helper::{MockProtocolContext, build_chain, new_transaction};
use crate::relayer::{
    MAX_RELAY_PEERS, transaction_hashes_process::TransactionHashesProcess,
    transactions_process::TransactionsProcess,
};
use crate::types::KnownRemoteBatch;
use ckb_network::{CKBProtocolContext, PeerIndex, SupportProtocols};
use ckb_types::{packed, prelude::*};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

#[test]
fn requested_bodies_require_the_current_source_and_an_unknown_hash() {
    let (_chain, relayer, always_success) = build_chain(1);
    let state = relayer.shared.state();
    let peer = PeerIndex::from(1);
    let other = PeerIndex::from(2);
    let transactions: Vec<_> = (0..3)
        .map(|nonce| new_transaction(&relayer, 800 + nonce, &always_success))
        .collect();
    state.add_ask_for_txs(peer, vec![transactions[0].hash()]);
    state.add_ask_for_txs(other, vec![transactions[1].hash()]);
    state.pop_ask_for_txs();
    let bodies = || {
        transactions
            .iter()
            .cloned()
            .map(|transaction| (transaction, 1))
    };
    assert_eq!(
        state.requested_transactions(peer, bodies()),
        vec![(transactions[0].clone(), 1)]
    );
    state.mark_as_known_tx(transactions[0].hash());
    assert!(state.requested_transactions(peer, bodies()).is_empty());
}

#[test]
fn relay_rejects_a_cycle_declaration_above_consensus_before_tx_pool_handoff() {
    let (_chain, relayer, always_success_out_point) = build_chain(1);
    let transaction = new_transaction(&relayer, 700, &always_success_out_point);
    let hash = transaction.hash();
    let source_peer = PeerIndex::from(6usize);
    let state = relayer.shared.state();
    state.add_ask_for_txs(source_peer, vec![hash.clone()]);
    assert_eq!(
        state.pop_ask_for_txs().get(&source_peer),
        Some(&vec![hash.clone()])
    );

    let declared_cycles = relayer
        .shared
        .consensus()
        .max_block_cycles()
        .checked_add(1)
        .expect("the consensus maximum leaves one hostile declaration");
    let relay_transaction = packed::RelayTransaction::new_builder()
        .cycles(declared_cycles)
        .transaction(transaction.data())
        .build();
    let content = packed::RelayTransactions::new_builder()
        .transactions(
            packed::RelayTransactionVec::new_builder()
                .set(vec![relay_transaction])
                .build(),
        )
        .build();
    let context = Arc::new(MockProtocolContext::new(SupportProtocols::RelayV3));
    let context_handle = Arc::clone(&context);
    let protocol_context: Arc<dyn CKBProtocolContext + Sync> = context_handle;
    TransactionsProcess::new(content.as_reader(), &relayer, protocol_context, source_peer)
        .execute();

    assert_eq!(
        context.banned_peer_reasons(),
        vec![(
            source_peer,
            String::from("relay declared cycles greater than max_block_cycles"),
        )]
    );
    assert!(
        !state.already_known_tx(&hash),
        "the precheck returns before the relay publishes a known mark or tx-pool handoff"
    );
}

#[test]
fn remote_closed_controller_releases_known_projection() {
    let (mut chain, relayer, always_success_out_point) = build_chain(1);
    let transaction = new_transaction(&relayer, 701, &always_success_out_point);
    let hash = transaction.hash();
    let source_peer = PeerIndex::from(7usize);
    let replacement_peer = PeerIndex::from(8usize);
    let state = relayer.shared.state();
    state.add_ask_for_txs(source_peer, vec![hash.clone()]);
    assert_eq!(
        state.pop_ask_for_txs().get(&source_peer),
        Some(&vec![hash.clone()])
    );

    chain.stop_tx_pool();
    let controller = relayer.shared.shared().tx_pool_controller().clone();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("the boundary probe runtime builds");
    let probe = runtime.block_on(controller.submit_remote_tx(transaction.clone(), 0, source_peer));
    assert!(
        probe
            .expect_err("the stopped controller has no payload owner")
            .to_string()
            .contains("channel closed")
    );

    let relay_transaction = packed::RelayTransaction::new_builder()
        .cycles(0u64)
        .transaction(transaction.data())
        .build();
    let content = packed::RelayTransactions::new_builder()
        .transactions(
            packed::RelayTransactionVec::new_builder()
                .set(vec![relay_transaction])
                .build(),
        )
        .build();
    let context: Arc<dyn CKBProtocolContext + Sync> =
        Arc::new(MockProtocolContext::new(SupportProtocols::RelayV3));
    TransactionsProcess::new(content.as_reader(), &relayer, context, source_peer).execute();

    let release_deadline = Instant::now() + Duration::from_secs(5);
    while state.already_known_tx(&hash) && Instant::now() < release_deadline {
        std::thread::yield_now();
    }
    assert!(
        !state.already_known_tx(&hash),
        "a failed Remote handoff releases its exact known-filter mark"
    );
    let announcement = packed::RelayTransactionHashes::new_builder()
        .tx_hashes(vec![hash])
        .build();
    let _ = TransactionHashesProcess::new(announcement.as_reader(), &relayer, replacement_peer)
        .execute();
    assert!(
        state.pop_ask_for_txs().contains_key(&replacement_peer),
        "another peer can reannounce a transaction whose handoff failed"
    );
}

#[test]
fn remote_batch_admission_exhaustion_releases_known_without_spawning() {
    let (_chain, relayer, always_success_out_point) = build_chain(1);
    let transaction = new_transaction(&relayer, 705, &always_success_out_point);
    let hash = transaction.hash();
    let source_peer = PeerIndex::from(10usize);
    let state = relayer.shared.state();
    state.add_ask_for_txs(source_peer, vec![hash.clone()]);
    assert_eq!(
        state.pop_ask_for_txs().get(&source_peer),
        Some(&vec![hash.clone()])
    );

    let permits: Vec<_> = (0..MAX_RELAY_PEERS)
        .map(|_| {
            Arc::clone(&relayer.remote_batch_admission)
                .try_acquire_owned()
                .expect("the test owns every remote batch admission")
        })
        .collect();
    let relay_transaction = packed::RelayTransaction::new_builder()
        .cycles(0u64)
        .transaction(transaction.data())
        .build();
    let content = packed::RelayTransactions::new_builder()
        .transactions(
            packed::RelayTransactionVec::new_builder()
                .set(vec![relay_transaction])
                .build(),
        )
        .build();
    let context: Arc<dyn CKBProtocolContext + Sync> =
        Arc::new(MockProtocolContext::new(SupportProtocols::RelayV3));
    TransactionsProcess::new(content.as_reader(), &relayer, context, source_peer).execute();

    assert!(
        !state.already_known_tx(&hash),
        "a batch rejected before spawn releases every tentative known mark synchronously"
    );
    drop(permits);
}

#[test]
fn remote_known_batch_drop_releases_only_the_uncommitted_suffix() {
    let (_chain, relayer, always_success_out_point) = build_chain(1);
    let first = new_transaction(&relayer, 703, &always_success_out_point).hash();
    let second = new_transaction(&relayer, 704, &always_success_out_point).hash();
    let state = relayer.shared.state();

    drop(KnownRemoteBatch::mark(
        Arc::clone(&relayer.shared),
        vec![first.clone(), second.clone()],
    ));
    assert!(!state.already_known_tx(&first));
    assert!(!state.already_known_tx(&second));

    let mut known = KnownRemoteBatch::mark(
        Arc::clone(&relayer.shared),
        vec![first.clone(), second.clone()],
    );
    assert!(state.already_known_tx(&first));
    assert!(state.already_known_tx(&second));
    known.complete_prefix(1);
    drop(known);
    assert!(
        state.already_known_tx(&first),
        "the committed canonical prefix remains known"
    );
    assert!(
        !state.already_known_tx(&second),
        "the uncommitted suffix is released on drop"
    );
    state.remove_from_known_txs(&first);
}

#[test]
fn duplicate_remote_body_in_a_completed_prefix_survives_suffix_cancellation() {
    let (_chain, relayer, always_success) = build_chain(1);
    let transaction = new_transaction(&relayer, 710, &always_success);
    let hash = transaction.hash();
    let state = relayer.shared.state();
    let peer = PeerIndex::from(11);
    state.add_ask_for_txs(peer, vec![hash.clone()]);
    state.pop_ask_for_txs();
    // The real body gate permits repeated requested transactions in one batch.
    let bodies = state.requested_transactions(
        peer,
        [(transaction.clone(), 1), (transaction, 1)].into_iter(),
    );
    assert_eq!(bodies.len(), 2);
    let mut batch = KnownRemoteBatch::mark(
        Arc::clone(&relayer.shared),
        bodies.iter().map(|(transaction, _)| transaction.hash()),
    );
    batch.complete_prefix(1);
    drop(batch);
    assert!(state.already_known_tx(&hash));

    state.remove_from_known_txs(&hash);
    drop(KnownRemoteBatch::mark(
        Arc::clone(&relayer.shared),
        vec![hash.clone(), hash.clone()],
    ));
    assert!(!state.already_known_tx(&hash));
}

#[test]
fn overlapping_remote_batches_keep_known_until_all_cancel_or_any_completes() {
    let (_chain, relayer, always_success) = build_chain(1);
    let hash = new_transaction(&relayer, 711, &always_success).hash();
    let state = relayer.shared.state();
    for completed in [[false, false], [true, false], [false, true], [true, true]] {
        for order in [[0, 1], [1, 0]] {
            state.reset_known_txs();
            let mut batches = [0, 1].map(|_| {
                Some(KnownRemoteBatch::mark(
                    Arc::clone(&relayer.shared),
                    vec![hash.clone()],
                ))
            });
            for (step, index) in order.into_iter().enumerate() {
                let mut batch = batches[index].take().unwrap();
                if completed[index] {
                    batch.complete_prefix(1);
                }
                drop(batch);
                assert_eq!(
                    state.already_known_tx(&hash),
                    step == 0 || completed.into_iter().any(|done| done),
                    "completion={completed:?}, order={order:?}, step={step}"
                );
            }
        }
    }
}

#[test]
fn a_batch_reinserting_its_own_evicted_hash_cannot_settle_or_release_the_old_identity() {
    use crate::types::TtlFilter;

    let (_chain, relayer, always_success) = build_chain(1);
    let first = new_transaction(&relayer, 719, &always_success).hash();
    let second = new_transaction(&relayer, 720, &always_success).hash();
    let third = new_transaction(&relayer, 721, &always_success).hash();
    let state = relayer.shared.state();
    for prefix in [0, 1, 4] {
        for other_completes in [false, true] {
            for order in [[0, 1], [1, 0]] {
                *state.tx_filter() = TtlFilter::new(2, crate::types::FILTER_TTL);
                let mut batches = [
                    Some(KnownRemoteBatch::mark(
                        Arc::clone(&relayer.shared),
                        vec![first.clone(), second.clone(), third.clone(), first.clone()],
                    )),
                    Some(KnownRemoteBatch::mark(
                        Arc::clone(&relayer.shared),
                        vec![first.clone()],
                    )),
                ];
                for (step, index) in order.into_iter().enumerate() {
                    let mut batch = batches[index].take().unwrap();
                    batch.complete_prefix(if index == 0 {
                        prefix
                    } else {
                        usize::from(other_completes)
                    });
                    drop(batch);
                    assert_eq!(
                        state.already_known_tx(&first),
                        step == 0 || prefix == 4 || other_completes,
                        "evicted first occurrence is not the replacement: prefix={prefix}, other={other_completes}, order={order:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn accepted_known_marks_survive_older_and_later_batch_cleanup() {
    let (_chain, relayer, always_success) = build_chain(1);
    let hash = new_transaction(&relayer, 712, &always_success).hash();
    let state = relayer.shared.state();
    for complete in [false, true] {
        state.reset_tx_pool_relay_projection();
        let mut older = KnownRemoteBatch::mark(Arc::clone(&relayer.shared), vec![hash.clone()]);
        state.record_accepted_tx(hash.clone(), Some(12.into()));
        let later = KnownRemoteBatch::mark(Arc::clone(&relayer.shared), vec![hash.clone()]);
        if complete {
            older.complete_prefix(1);
        }
        drop(older);
        drop(later);
        assert!(state.already_known_tx(&hash));
        assert_eq!(
            state.take_pending_relay_txs(1),
            vec![(hash.clone(), Some(12.into()))]
        );
    }
}

#[test]
fn reset_and_rejection_prevent_late_completion_or_cleanup_from_changing_a_new_claim() {
    let (_chain, relayer, always_success) = build_chain(1);
    let hash = new_transaction(&relayer, 713, &always_success).hash();
    let state = relayer.shared.state();
    for reset in [false, true] {
        for replace in [false, true] {
            for complete in [false, true] {
                let mut older =
                    KnownRemoteBatch::mark(Arc::clone(&relayer.shared), vec![hash.clone()]);
                if reset {
                    state.reset_tx_pool_relay_projection();
                } else {
                    state.reject_pending_relay_tx(&hash);
                }
                assert!(!state.already_known_tx(&hash));
                let later = replace.then(|| {
                    KnownRemoteBatch::mark(Arc::clone(&relayer.shared), vec![hash.clone()])
                });
                if complete {
                    older.complete_prefix(1);
                }
                drop(older);
                assert_eq!(state.already_known_tx(&hash), replace);
                drop(later);
                assert!(
                    !state.already_known_tx(&hash),
                    "old completion cannot settle a successor: reset={reset}, complete={complete}"
                );
            }
        }
    }
}

#[test]
fn remote_claim_settlement_and_cancellation_preserve_ttl_without_reviving_expired_marks() {
    use crate::types::TtlFilter;

    let (_chain, relayer, always_success) = build_chain(1);
    let first = new_transaction(&relayer, 714, &always_success).hash();
    let second = new_transaction(&relayer, 715, &always_success).hash();
    let state = relayer.shared.state();
    *state.tx_filter() = TtlFilter::new(4, 3);
    let start = ckb_systemtime::unix_time().as_secs() * 1000;
    let clock = ckb_systemtime::faketime();
    clock.set_faketime(start);
    let mut older = KnownRemoteBatch::mark(
        Arc::clone(&relayer.shared),
        vec![first.clone(), second.clone()],
    );
    clock.set_faketime(start + 3_000);
    state.tx_filter().remove_expired();
    assert!(state.already_known_tx(&first));
    assert!(state.already_known_tx(&second));
    older.complete_prefix(1);
    clock.set_faketime(start + 4_000);
    state.tx_filter().remove_expired();
    assert!(
        !state.already_known_tx(&first),
        "completion cannot extend TTL"
    );
    assert!(!state.already_known_tx(&second));
    let later = KnownRemoteBatch::mark(Arc::clone(&relayer.shared), vec![second.clone()]);
    older.complete_prefix(2);
    drop(older);
    assert!(state.already_known_tx(&second));
    drop(later);
    assert!(
        !state.already_known_tx(&second),
        "expired identity cannot settle a successor"
    );
    assert!(!state.already_known_tx(&first));

    // Cancellation of one overlapping claimant must not renew the survivor.
    let cancelled = KnownRemoteBatch::mark(Arc::clone(&relayer.shared), vec![first.clone()]);
    let mut survivor = KnownRemoteBatch::mark(Arc::clone(&relayer.shared), vec![first.clone()]);
    clock.set_faketime(start + 7_000);
    drop(cancelled);
    clock.set_faketime(start + 8_000);
    state.tx_filter().remove_expired();
    assert!(
        !state.already_known_tx(&first),
        "cancellation cannot extend TTL"
    );
    survivor.complete_prefix(1);
    drop(survivor);
    assert!(
        !state.already_known_tx(&first),
        "late completion cannot recreate an expired mark"
    );
}

#[test]
fn remote_claim_settlement_and_cancellation_do_not_refresh_lru_or_revive_evicted_marks() {
    use crate::types::TtlFilter;

    let (_chain, relayer, always_success) = build_chain(1);
    let first = new_transaction(&relayer, 716, &always_success).hash();
    let second = new_transaction(&relayer, 717, &always_success).hash();
    let newest = new_transaction(&relayer, 718, &always_success).hash();
    let state = relayer.shared.state();
    for complete in [false, true] {
        *state.tx_filter() = TtlFilter::new(2, crate::types::FILTER_TTL);
        let mut earlier = KnownRemoteBatch::mark(Arc::clone(&relayer.shared), vec![first.clone()]);
        let mut overlapping =
            KnownRemoteBatch::mark(Arc::clone(&relayer.shared), vec![first.clone()]);
        let other = KnownRemoteBatch::mark(Arc::clone(&relayer.shared), vec![second.clone()]);
        if complete {
            earlier.complete_prefix(1);
        }
        drop(earlier);
        state.mark_as_known_tx(newest.clone());
        assert!(
            !state.already_known_tx(&first),
            "settling/releasing must preserve LRU order"
        );
        assert!(state.already_known_tx(&second));
        assert!(state.already_known_tx(&newest));
        overlapping.complete_prefix(1);
        drop(overlapping);
        assert!(
            !state.already_known_tx(&first),
            "late completion cannot recreate an evicted mark"
        );
        drop(other);
    }
}

#[test]
fn remote_batch_task_cancellation_returns_admission_and_releases_known() {
    let (_chain, relayer, always_success_out_point) = build_chain(1);
    let hash = new_transaction(&relayer, 706, &always_success_out_point).hash();
    let state = relayer.shared.state();
    let admission = Arc::clone(&relayer.remote_batch_admission)
        .try_acquire_owned()
        .expect("one remote batch admission remains available");
    let known = KnownRemoteBatch::mark(Arc::clone(&relayer.shared), vec![hash.clone()]);
    let task = relayer.shared.shared().async_handle().spawn(async move {
        let _admission = admission;
        let _known = known;
        std::future::pending::<()>().await;
    });
    assert_eq!(
        relayer.remote_batch_admission.available_permits(),
        MAX_RELAY_PEERS - 1
    );
    task.abort();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !task.is_finished() && Instant::now() < deadline {
        std::thread::yield_now();
    }
    assert!(task.is_finished(), "the cancelled task is dropped");
    assert_eq!(
        relayer.remote_batch_admission.available_permits(),
        MAX_RELAY_PEERS,
        "task cancellation returns the linear admission"
    );
    assert!(
        !state.already_known_tx(&hash),
        "task cancellation drops the guard and releases every uncommitted known mark"
    );
}

#[test]
fn duplicate_unknown_hash_from_one_peer_has_one_request_source() {
    let (_chain, relayer, always_success_out_point) = build_chain(1);
    let hash = new_transaction(&relayer, 702, &always_success_out_point).hash();
    let peer = PeerIndex::from(9usize);
    let state = relayer.shared.state();

    state.add_ask_for_txs(peer, vec![hash.clone(), hash]);
    let mut priority = state
        .unknown_tx_hashes()
        .peek()
        .map(|(_, priority)| priority.clone())
        .expect("one unique hash remains queued");

    assert_eq!(priority.next_request_peer(), Some(peer));
    assert_eq!(
        priority.next_request_peer(),
        None,
        "one peer cannot amplify one unknown hash into repeated request slots"
    );
}
