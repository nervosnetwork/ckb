use crate::StatusCode;
use crate::relayer::get_transactions_process::GetTransactionsProcess;
use crate::relayer::tests::helper::{MockProtocolContext, build_chain, new_transaction};
use ckb_network::{PeerIndex, SupportProtocols};
use ckb_types::packed;
use ckb_types::prelude::*;
use std::collections::HashSet;
use std::sync::Arc;

fn alternate_hash(tx_hash: &packed::Byte32) -> packed::Byte32 {
    let mut bytes = tx_hash.as_slice().to_vec();
    bytes[31] ^= 1;
    packed::Byte32::from_slice(&bytes).expect("valid transaction hash")
}

fn relay_message(tx: &ckb_types::core::TransactionView, cycles: u64) -> packed::RelayMessage {
    let relay_tx = packed::RelayTransaction::new_builder()
        .cycles(cycles)
        .transaction(tx.data())
        .build();
    packed::RelayMessage::new_builder()
        .set(
            packed::RelayTransactions::new_builder()
                .transactions(
                    packed::RelayTransactionVec::new_builder()
                        .set(vec![relay_tx])
                        .build(),
                )
                .build(),
        )
        .build()
}

#[test]
fn test_duplicate() {
    let (_chain, relayer, always_success_out_point) = build_chain(5);

    let tx = new_transaction(&relayer, 1, &always_success_out_point);
    let tx_hash = tx.hash();
    let content = packed::GetRelayTransactions::new_builder()
        .tx_hashes(vec![tx_hash.clone(), tx_hash])
        .build();
    let mock_protocol_context = MockProtocolContext::new(SupportProtocols::RelayV3);
    let nc = Arc::new(mock_protocol_context);
    let peer_index: PeerIndex = 1.into();
    let process = GetTransactionsProcess::new(content.as_reader(), &relayer, nc, peer_index);

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    assert_eq!(
        rt.block_on(process.execute()),
        StatusCode::RequestDuplicate.with_context("Request duplicate transaction")
    );
}

#[test]
fn test_fetch_transactions_by_hash() {
    let (_chain, relayer, always_success_out_point) = build_chain(5);
    let tx = new_transaction(&relayer, 1, &always_success_out_point);
    let tx_hash = tx.hash();
    let alternate_hash = alternate_hash(&tx_hash);
    assert_eq!(
        packed::ProposalShortId::from_tx_hash(&tx_hash),
        packed::ProposalShortId::from_tx_hash(&alternate_hash)
    );

    relayer
        .shared
        .shared()
        .tx_pool_controller()
        .submit_local_tx(tx.clone())
        .expect("submit request")
        .expect("resident transaction accepted");

    let content = packed::GetRelayTransactions::new_builder()
        .tx_hashes(vec![alternate_hash.clone()])
        .build();
    let nc = Arc::new(MockProtocolContext::new(SupportProtocols::RelayV3));
    let process = GetTransactionsProcess::new(
        content.as_reader(),
        &relayer,
        Arc::<MockProtocolContext>::clone(&nc),
        1.into(),
    );
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    assert_eq!(rt.block_on(process.execute()), crate::Status::ok());
    assert_eq!(nc.sent_messages_len(), 0);

    let alternate_fetched = rt
        .block_on(
            relayer
                .shared
                .shared()
                .tx_pool_controller()
                .fetch_txs_with_cycles(HashSet::from([alternate_hash.clone()])),
        )
        .expect("fetch response");
    assert!(alternate_fetched.is_empty());

    let fetched = rt
        .block_on(
            relayer
                .shared
                .shared()
                .tx_pool_controller()
                .fetch_txs_with_cycles(HashSet::from([tx_hash.clone()])),
        )
        .expect("fetch response");
    let cycles = fetched.first().expect("resident transaction returned").1;
    let expected = relay_message(&tx, cycles).as_bytes();
    let peer_index: PeerIndex = 1.into();

    for requested in [vec![tx_hash.clone()], vec![tx_hash, alternate_hash]] {
        let content = packed::GetRelayTransactions::new_builder()
            .tx_hashes(requested)
            .build();
        let nc = Arc::new(MockProtocolContext::new(SupportProtocols::RelayV3));
        let process = GetTransactionsProcess::new(
            content.as_reader(),
            &relayer,
            Arc::<MockProtocolContext>::clone(&nc),
            peer_index,
        );
        assert_eq!(rt.block_on(process.execute()), crate::Status::ok());
        assert_eq!(nc.sent_messages_len(), 1);
        assert!(nc.has_sent(
            SupportProtocols::RelayV3.protocol_id(),
            peer_index,
            expected.clone(),
        ));
    }
}
