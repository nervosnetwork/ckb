use super::helper::{MockProtocolContext, build_chain, new_transaction};
use crate::Status;
use ckb_network::{CKBProtocolContext, Peer, SessionType, SupportProtocols};
use ckb_store::ChainStore;
use ckb_types::{packed, prelude::*};
use std::sync::Arc;

fn context(session_type: SessionType) -> Arc<MockProtocolContext> {
    Arc::new(
        MockProtocolContext::new(SupportProtocols::RelayV3).with_peer(Peer::new(
            1.into(),
            session_type,
            "/ip4/127.0.0.1/tcp/8115".parse().unwrap(),
            false,
        )),
    )
}

#[test]
fn block_only_ignores_transaction_messages_but_serves_block_transactions() {
    let (_chain, mut relayer, out_point) = build_chain(5);
    let tx = new_transaction(&relayer, 1, &out_point);
    relayer
        .shared
        .shared()
        .tx_pool_controller()
        .submit_local_tx(tx.clone())
        .unwrap()
        .unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let nc = context(SessionType::BlockRelayOnly);
    let messages = [
        packed::RelayMessage::new_builder()
            .set(
                packed::RelayTransactionHashes::new_builder()
                    .tx_hashes(vec![tx.hash()])
                    .build(),
            )
            .build(),
        packed::RelayMessage::new_builder()
            .set(
                packed::RelayTransactions::new_builder()
                    .transactions(vec![
                        packed::RelayTransaction::new_builder()
                            .transaction(tx.data())
                            .cycles(0u64)
                            .build(),
                    ])
                    .build(),
            )
            .build(),
        packed::RelayMessage::new_builder()
            .set(
                packed::GetRelayTransactions::new_builder()
                    .tx_hashes(vec![tx.hash()])
                    .build(),
            )
            .build(),
    ];
    for message in &messages {
        assert_eq!(
            rt.block_on(relayer.try_process(nc.clone(), 1.into(), message.as_reader().to_enum())),
            Status::ignored()
        );
    }
    assert!(relayer.shared.state().unknown_tx_hashes().is_empty());
    assert!(!relayer.shared.state().tx_filter().contains(&tx.hash()));
    assert_eq!(nc.sent_messages_len(), 0);

    // The same transaction query remains available on a full-relay connection.
    let full = context(SessionType::Outbound);
    assert_eq!(
        rt.block_on(relayer.try_process(full.clone(), 1.into(), messages[2].as_reader().to_enum())),
        Status::ok()
    );
    assert_eq!(full.sent_messages_len(), 1);

    let tip_hash = relayer.shared.active_chain().tip_hash();
    let block = relayer.shared.store().get_block(&tip_hash).unwrap();
    let request = packed::RelayMessage::new_builder()
        .set(
            packed::GetBlockTransactions::new_builder()
                .block_hash(tip_hash.clone())
                .indexes(vec![0u32])
                .build(),
        )
        .build();
    assert_eq!(
        rt.block_on(relayer.try_process(nc.clone(), 1.into(), request.as_reader().to_enum())),
        Status::ok()
    );
    let response = packed::RelayMessage::new_builder()
        .set(
            packed::BlockTransactions::new_builder()
                .block_hash(tip_hash)
                .transactions(vec![block.transactions()[0].data()])
                .build(),
        )
        .build();
    assert!(nc.has_sent(
        SupportProtocols::RelayV3.protocol_id(),
        1.into(),
        response.as_bytes()
    ));
}

/// A block-relay-only peer may still ask for proposals, it needs them to reconstruct
/// the compact blocks we send it. What it must not get is the pending-request cache:
/// that would turn one GetBlockProposal into a standing subscription pushing every
/// matching transaction to it as soon as it lands in the pool.
#[test]
fn block_only_get_block_proposal_serves_pool_but_leaves_no_subscription() {
    let (_chain, mut relayer, out_point) = build_chain(5);
    let in_pool = new_transaction(&relayer, 1, &out_point);
    relayer
        .shared
        .shared()
        .tx_pool_controller()
        .submit_local_tx(in_pool.clone())
        .unwrap()
        .unwrap();
    let not_in_pool = new_transaction(&relayer, 2, &out_point);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let tip_hash = relayer.shared.active_chain().tip_hash();
    let request = |ids: Vec<packed::ProposalShortId>| {
        packed::RelayMessage::new_builder()
            .set(
                packed::GetBlockProposal::new_builder()
                    .block_hash(tip_hash.clone())
                    .proposals(ids)
                    .build(),
            )
            .build()
    };
    let ids = vec![in_pool.proposal_short_id(), not_in_pool.proposal_short_id()];

    let nc = context(SessionType::BlockRelayOnly);
    assert_eq!(
        rt.block_on(relayer.try_process(
            nc.clone(),
            1.into(),
            request(ids.clone()).as_reader().to_enum()
        )),
        Status::ok()
    );
    // Served from the pool, but nothing cached for later push.
    assert_eq!(nc.sent_messages_len(), 1);
    assert!(
        relayer
            .shared
            .state()
            .drain_get_block_proposals()
            .is_empty()
    );

    let full = context(SessionType::Outbound);
    assert_eq!(
        rt.block_on(relayer.try_process(
            full.clone(),
            1.into(),
            request(ids).as_reader().to_enum()
        )),
        Status::ok()
    );
    assert_eq!(full.sent_messages_len(), 1);
    let pending = relayer.shared.state().drain_get_block_proposals();
    assert_eq!(pending.len(), 1);
    assert!(pending.contains_key(&not_in_pool.proposal_short_id()));
}

#[test]
fn transaction_requests_skip_block_only_peers() {
    let (_chain, relayer, out_point) = build_chain(5);
    relayer.shared.state().peers().relay_connected(1.into());
    let tx = new_transaction(&relayer, 1, &out_point);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for session_type in [SessionType::BlockRelayOnly, SessionType::Outbound] {
        relayer.shared.state().unknown_tx_hashes().clear();
        assert_eq!(
            relayer
                .shared
                .state()
                .add_ask_for_txs(1.into(), vec![tx.hash()]),
            Status::ok()
        );
        let nc = context(session_type);
        let protocol: Arc<dyn CKBProtocolContext + Sync> = nc.clone();
        rt.block_on(relayer.ask_for_txs(&protocol));
        assert_eq!(
            nc.sent_messages_len(),
            usize::from(session_type == SessionType::Outbound)
        );
    }
}
