use crate::StatusCode;
use crate::relayer::get_transactions_process::GetTransactionsProcess;
use crate::relayer::tests::helper::{MockProtocolContext, build_chain, new_transaction};
use ckb_network::{PeerIndex, SupportProtocols};
use ckb_types::packed;
use ckb_types::prelude::*;
use std::sync::Arc;

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

    assert_eq!(
        process.execute(),
        StatusCode::RequestDuplicate.with_context("Request duplicate transaction")
    );
}
