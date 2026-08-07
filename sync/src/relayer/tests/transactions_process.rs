//! Negative production refinement witnesses for the pre-authority Remote handoff.

use crate::relayer::tests::helper::{MockProtocolContext, build_chain, new_transaction};
use crate::relayer::{
    transaction_hashes_process::TransactionHashesProcess, transactions_process::TransactionsProcess,
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

/// This test intentionally records the current production divergence from the
/// required handoff model. The required observation after a closed controller
/// is `Released`; current Remote relay instead leaves the raw hash known. M4
/// must invert this witness to equality when enqueue acknowledgement owns the
/// projection transition.
#[test]
fn counterexample_remote_closed_controller_retains_known_projection() {
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

    assert!(
        state.already_known_tx(&hash),
        "current Remote code marks known before the closed enqueue and has no inverse"
    );
    let announcement = packed::RelayTransactionHashes::new_builder()
        .tx_hashes(vec![hash.clone()])
        .build();
    let _ = TransactionHashesProcess::new(announcement.as_reader(), &relayer, replacement_peer)
        .execute();
    assert!(
        !state.pop_ask_for_txs().contains_key(&replacement_peer),
        "the stale known projection suppresses a refetch from another peer"
    );
}
