//! Negative production refinement witnesses for the pre-authority Remote handoff.

use crate::relayer::tests::helper::{MockProtocolContext, build_chain, new_transaction};
use crate::relayer::{
    MAX_RELAY_PEERS,
    transaction_hashes_process::TransactionHashesProcess,
    transactions_process::{KnownRemoteBatch, TransactionsProcess},
};
use ckb_network::{CKBProtocolContext, PeerIndex, SupportProtocols};
use ckb_types::{packed, prelude::*};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

fn stop_tx_pool(relayer: &crate::relayer::Relayer) {
    let controller = relayer.shared.shared().tx_pool_controller();
    let start_deadline = Instant::now() + Duration::from_secs(5);
    while !controller.service_started() && Instant::now() < start_deadline {
        std::thread::yield_now();
    }
    assert!(
        controller.service_started(),
        "the fixture tx-pool reaches Running"
    );
    controller.stop();
    let stop_deadline = Instant::now() + Duration::from_secs(5);
    while controller.service_started() && Instant::now() < stop_deadline {
        std::thread::yield_now();
    }
    assert!(
        !controller.service_started(),
        "the fixture controller stops"
    );
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
    let (_chain, relayer, always_success_out_point) = build_chain(1);
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

    stop_tx_pool(&relayer);
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

    state.mark_as_known_txs([first.clone(), second.clone()].into_iter());
    drop(KnownRemoteBatch::new(
        Arc::clone(&relayer.shared),
        vec![first.clone(), second.clone()],
    ));
    assert!(!state.already_known_tx(&first));
    assert!(!state.already_known_tx(&second));

    state.mark_as_known_txs([first.clone(), second.clone()].into_iter());
    let mut known = KnownRemoteBatch::new(
        Arc::clone(&relayer.shared),
        vec![first.clone(), second.clone()],
    );
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
fn remote_batch_task_cancellation_returns_admission_and_releases_known() {
    let (_chain, relayer, always_success_out_point) = build_chain(1);
    let hash = new_transaction(&relayer, 706, &always_success_out_point).hash();
    let state = relayer.shared.state();
    state.mark_as_known_txs(std::iter::once(hash.clone()));
    let admission = Arc::clone(&relayer.remote_batch_admission)
        .try_acquire_owned()
        .expect("one remote batch admission remains available");
    let known = KnownRemoteBatch::new(Arc::clone(&relayer.shared), vec![hash.clone()]);
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
