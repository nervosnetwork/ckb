use crate::relayer::tests::helper::{MockProtocolContext, build_chain, new_transaction};
use crate::relayer::transaction_hashes_process::TransactionHashesProcess;
use ckb_network::{PeerIndex, SupportProtocols};
use ckb_types::{packed, prelude::*};
use std::sync::Arc;
use std::time::Duration;

/// The tx-pool result receiver owns mandatory local projection updates. A
/// temporary lack of relay peers must not leave the bounded result channel
/// undrained or omit local accepted membership from the known-tx filter.
#[test]
fn committed_tx_result_is_consumed_without_relay_peers() {
    let (_chain, relayer, always_success_out_point) = build_chain(1);
    let tx = new_transaction(&relayer, 1, &always_success_out_point);
    let tx_hash = tx.hash();
    let accepted = relayer
        .shared
        .shared()
        .tx_pool_controller()
        .submit_local_tx(tx)
        .expect("local submission reaches the tx-pool service");
    assert!(accepted.is_ok());

    let mock = Arc::new(MockProtocolContext::new(SupportProtocols::RelayV3));
    let nc: Arc<dyn ckb_network::CKBProtocolContext + Sync> = mock.clone();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !relayer.shared.state().already_known_tx(&tx_hash) {
                relayer.send_bulk_of_tx_hashes(&nc).await;
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("committed result is consumed without a connected peer");

        // The first drain consumed the bounded tx-pool sink, but it must have
        // retained the one-shot broadcast intent. A later peer receives the
        // hash without a second tx-pool event.
        let peer = PeerIndex::from(7);
        mock.set_full_relay_peers(vec![peer]);
        relayer.send_bulk_of_tx_hashes(&nc).await;
        let content = packed::RelayTransactionHashes::new_builder()
            .tx_hashes(vec![tx_hash])
            .build();
        let message = packed::RelayMessage::new_builder().set(content).build();
        assert!(mock.has_sent(
            SupportProtocols::RelayV3.protocol_id(),
            peer,
            message.as_bytes(),
        ));
    });
}

/// A terminal Reject is a release of the local relay projection, not a
/// permanent negative cache entry. Once the old owner is gone, an
/// announcement from another peer must become requestable again.
#[test]
fn rejected_tx_can_be_requested_again_from_another_peer() {
    let (_chain, relayer, always_success_out_point) = build_chain(1);
    let tx_hash = new_transaction(&relayer, 2, &always_success_out_point).hash();
    let state = relayer.shared.state();

    state.mark_as_known_tx(tx_hash.clone());
    state.reject_pending_relay_tx(&tx_hash);
    assert!(!state.already_known_tx(&tx_hash));

    let replacement_peer = PeerIndex::from(8);
    let content = packed::RelayTransactionHashes::new_builder()
        .tx_hashes(vec![tx_hash.clone()])
        .build();
    // The isolated fixture has no registered sessions, so the protocol
    // status may be `Ignored` after the hash is queued. The authoritative
    // assertion is the request projection below.
    let _status =
        TransactionHashesProcess::new(content.as_reader(), &relayer, replacement_peer).execute();
    assert_eq!(
        state.pop_ask_for_txs().get(&replacement_peer),
        Some(&vec![tx_hash])
    );
}
