use crate::StatusCode;
use crate::relayer::get_block_transactions_process::GetBlockTransactionsProcess;
use crate::relayer::tests::helper::{MockProtocolContext, build_chain};
use crate::relayer::{MAX_RELAY_TXS_BYTES_PER_BATCH, MAX_RELAY_TXS_NUM_PER_BATCH};
use ckb_network::{PeerIndex, SupportProtocols};
use ckb_store::ChainStore;
use ckb_types::packed;
use ckb_types::prelude::*;
use std::sync::Arc;

#[test]
fn test_reject_duplicate_transaction_indexes() {
    let (_chain, relayer, _) = build_chain(5);

    let tip_hash = relayer.shared.active_chain().tip_hash();
    let tip_block = relayer.shared.store().get_block(&tip_hash).unwrap();
    let repeated_tx = tip_block.transactions().first().unwrap().data();
    let repeat_count = MAX_RELAY_TXS_BYTES_PER_BATCH / repeated_tx.total_size() + 1;
    assert!(repeat_count <= MAX_RELAY_TXS_NUM_PER_BATCH);

    let content = packed::GetBlockTransactions::new_builder()
        .block_hash(tip_hash.clone())
        .indexes(vec![0u32; repeat_count])
        .build();
    let mock_protocol_context = MockProtocolContext::new(SupportProtocols::RelayV3);
    let nc = Arc::new(mock_protocol_context);
    let peer_index: PeerIndex = 1.into();
    let process = GetBlockTransactionsProcess::new(
        content.as_reader(),
        &relayer,
        Arc::<MockProtocolContext>::clone(&nc),
        peer_index,
    );

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    // Duplicate indexes are rejected with RequestDuplicate before any response is built
    assert_eq!(
        rt.block_on(process.execute()),
        StatusCode::RequestDuplicate.into(),
    );
}

#[test]
fn test_reject_duplicate_uncle_indexes() {
    let (_chain, relayer, _) = build_chain(5);

    let tip_hash = relayer.shared.active_chain().tip_hash();
    let content = packed::GetBlockTransactions::new_builder()
        .block_hash(tip_hash.clone())
        .uncle_indexes(vec![0u32, 0u32])
        .build();
    let mock_protocol_context = MockProtocolContext::new(SupportProtocols::RelayV3);
    let nc = Arc::new(mock_protocol_context);
    let peer_index: PeerIndex = 1.into();
    let process = GetBlockTransactionsProcess::new(
        content.as_reader(),
        &relayer,
        Arc::<MockProtocolContext>::clone(&nc),
        peer_index,
    );

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    assert_eq!(
        rt.block_on(process.execute()),
        StatusCode::RequestDuplicate.into(),
    );
}
