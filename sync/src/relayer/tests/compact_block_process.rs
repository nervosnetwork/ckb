use crate::relayer::compact_block_process::CompactBlockProcess;
use crate::relayer::tests::helper::{
    MockProtocolContext, build_chain, gen_block, new_header_builder,
};
use crate::{Status, StatusCode};
use ckb_chain::start_chain_services;
use ckb_network::{PeerIndex, SupportProtocols};
use ckb_shared::ChainServicesBuilder;
use ckb_shared::block_status::BlockStatus;
use ckb_store::ChainStore;
use ckb_systemtime::unix_time_as_millis;
use ckb_tx_pool::{PlugTarget, TxEntry};
use ckb_types::core::{BlockView, HeaderView};
use ckb_types::prelude::*;
use ckb_types::{
    bytes::Bytes,
    core::{BlockBuilder, Capacity, EpochNumberWithFraction, HeaderBuilder, TransactionBuilder},
    packed::{self, CellInput, CellOutputBuilder, CompactBlock, OutPoint, ProposalShortId},
};
use ckb_verification_traits::Switch;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[test]
fn test_in_block_status_map() {
    let (relayer, _) = build_chain(5);
    let header = {
        let shared = relayer.shared.shared();
        let parent = shared
            .store()
            .get_block_hash(4)
            .and_then(|block_hash| shared.store().get_block(&block_hash))
            .unwrap();
        new_header_builder(relayer.shared.shared(), &parent.header()).build()
    };
    let block = BlockBuilder::default()
        .transaction(TransactionBuilder::default().build())
        .header(header)
        .build();

    let mut prefilled_transactions_indexes = HashSet::new();
    prefilled_transactions_indexes.insert(0);
    let compact_block = CompactBlock::build_from_block(&block, &prefilled_transactions_indexes);

    let mock_protocol_context = MockProtocolContext::new(SupportProtocols::RelayV2);
    let nc = Arc::new(mock_protocol_context);
    let peer_index: PeerIndex = 1.into();

    let compact_block_process = CompactBlockProcess::new(
        compact_block.as_reader(),
        &relayer,
        Arc::<MockProtocolContext>::clone(&nc),
        peer_index,
    );

    // BLOCK_INVALID in block_status_map
    {
        relayer
            .shared
            .shared()
            .insert_block_status(block.header().hash(), BlockStatus::BLOCK_INVALID);
    }

    assert_eq!(
        compact_block_process.execute(),
        StatusCode::BlockIsInvalid.into(),
    );

    let compact_block_process = CompactBlockProcess::new(
        compact_block.as_reader(),
        &relayer,
        Arc::<MockProtocolContext>::clone(&nc),
        peer_index,
    );

    // BLOCK_STORED in block_status_map
    {
        relayer
            .shared
            .shared()
            .insert_block_status(block.header().hash(), BlockStatus::BLOCK_STORED);
    }

    assert_eq!(
        compact_block_process.execute(),
        StatusCode::CompactBlockAlreadyStored.into(),
    );

    let compact_block_process = CompactBlockProcess::new(
        compact_block.as_reader(),
        &relayer,
        Arc::<MockProtocolContext>::clone(&nc),
        peer_index,
    );

    // BLOCK_RECEIVED in block_status_map
    {
        relayer
            .shared
            .shared()
            .insert_block_status(block.header().hash(), BlockStatus::BLOCK_RECEIVED);
    }

    assert_eq!(compact_block_process.execute(), Status::ignored());
}

// send_getheaders_to_peer when UnknownParent
#[test]
fn test_unknow_parent() {
    let (relayer, _) = build_chain(5);

    // UnknownParent
    let block = BlockBuilder::default()
        .header(
            HeaderBuilder::default()
                .number(5.pack())
                .epoch(EpochNumberWithFraction::new(1, 5, 1000).pack())
                .timestamp(unix_time_as_millis().pack())
                .build(),
        )
        .transaction(TransactionBuilder::default().build())
        .build();

    let mut prefilled_transactions_indexes = HashSet::new();
    prefilled_transactions_indexes.insert(0);
    let compact_block = CompactBlock::build_from_block(&block, &prefilled_transactions_indexes);

    let mock_protocol_context = MockProtocolContext::new(SupportProtocols::RelayV2);
    let nc = Arc::new(mock_protocol_context);
    let peer_index: PeerIndex = 1.into();

    let compact_block_process = CompactBlockProcess::new(
        compact_block.as_reader(),
        &relayer,
        Arc::<MockProtocolContext>::clone(&nc),
        peer_index,
    );
    assert_eq!(
        compact_block_process.execute(),
        StatusCode::CompactBlockRequiresParent.into()
    );

    let active_chain = relayer.shared.active_chain();
    let header = active_chain.tip_header();
    let locator_hash = active_chain.get_locator((&header).into());

    let content = packed::GetHeaders::new_builder()
        .block_locator_hashes(locator_hash.pack())
        .hash_stop(packed::Byte32::zero())
        .build();
    let message = packed::SyncMessage::new_builder().set(content).build();
    let data = message.as_bytes();

    // send_getheaders_to_peer
    assert!(nc.has_sent(SupportProtocols::Sync.protocol_id(), peer_index, data));
}

#[test]
fn test_accept_not_a_better_block() {
    let (relayer, _) = build_chain(5);
    let tip_header = {
        let active_chain = relayer.shared.active_chain();
        active_chain.tip_header()
    };
    let second_to_last_header: HeaderView = {
        let tip_header: HeaderView = relayer.shared().store().get_tip_header().unwrap();
        let second_to_last_header = relayer
            .shared()
            .store()
            .get_block_header(&tip_header.data().raw().parent_hash())
            .unwrap();
        second_to_last_header
    };

    let uncle_block: BlockView = gen_block(
        &second_to_last_header,
        relayer.shared().shared(),
        1,
        1,
        None,
    );
    //uncle_block's block_hash must not equal to tip's block_hash
    assert_ne!(uncle_block.header().hash(), tip_header.hash());
    // uncle_block's difficulty must less than tip's difficulty
    assert!(uncle_block.difficulty().lt(&tip_header.difficulty()));

    let mut prefilled_transactions_indexes = HashSet::new();
    prefilled_transactions_indexes.insert(0);
    let compact_block =
        CompactBlock::build_from_block(&uncle_block, &prefilled_transactions_indexes);

    let mock_protocol_context = MockProtocolContext::new(SupportProtocols::RelayV2);
    let nc = Arc::new(mock_protocol_context);
    let peer_index: PeerIndex = 1.into();

    let compact_block_process = CompactBlockProcess::new(
        compact_block.as_reader(),
        &relayer,
        Arc::<MockProtocolContext>::clone(&nc),
        peer_index,
    );
    assert_eq!(compact_block_process.execute(), Status::ok());

    // wait chain_service processed the compact block, check block hash in snapshot
    {
        let now = std::time::Instant::now();
        loop {
            std::thread::sleep(std::time::Duration::from_millis(100));
            if now.elapsed().as_secs() > 5 {
                panic!("wait chain_service processed the compact block timeout");
            }
            let snapshot = relayer.shared.shared().snapshot();
            if snapshot.get_block(&uncle_block.header().hash()).is_some() {
                break;
            }
        }
    }
}

#[test]
fn test_header_invalid() {
    let (relayer, _) = build_chain(5);
    let parent = {
        let active_chain = relayer.shared.active_chain();
        active_chain.tip_header()
    };

    // Better block but block number is invalid
    let header = new_header_builder(relayer.shared.shared(), &parent)
        .number(4.pack())
        .build();

    let block = BlockBuilder::default()
        .header(header)
        .transaction(TransactionBuilder::default().build())
        .build();

    let mut prefilled_transactions_indexes = HashSet::new();
    prefilled_transactions_indexes.insert(0);
    let compact_block = CompactBlock::build_from_block(&block, &prefilled_transactions_indexes);

    let mock_protocol_context = MockProtocolContext::new(SupportProtocols::RelayV2);
    let nc = Arc::new(mock_protocol_context);
    let peer_index: PeerIndex = 1.into();

    let compact_block_process = CompactBlockProcess::new(
        compact_block.as_reader(),
        &relayer,
        Arc::<MockProtocolContext>::clone(&nc),
        peer_index,
    );
    assert_eq!(
        compact_block_process.execute(),
        StatusCode::CompactBlockHasInvalidHeader.into(),
    );
    // Assert block_status_map update
    assert_eq!(
        relayer
            .shared()
            .active_chain()
            .get_block_status(&block.header().hash()),
        BlockStatus::BLOCK_INVALID
    );
}

#[test]
fn test_send_missing_indexes() {
    let (relayer, _) = build_chain(5);
    let parent = {
        let active_chain = relayer.shared.active_chain();
        active_chain.tip_header()
    };

    let header = new_header_builder(relayer.shared.shared(), &parent).build();

    let proposal_id = ProposalShortId::new([1u8; 10]);

    let uncle = BlockBuilder::default().build();

    // Better block including one missing transaction
    let block = BlockBuilder::default()
        .header(header)
        .transaction(TransactionBuilder::default().build())
        .transaction(
            TransactionBuilder::default()
                .output(
                    CellOutputBuilder::default()
                        .capacity(Capacity::bytes(1).unwrap().pack())
                        .build(),
                )
                .output_data(Bytes::new().pack())
                .build(),
        )
        .uncle(uncle.as_uncle())
        .proposal(proposal_id.clone())
        .build();

    let mut prefilled_transactions_indexes = HashSet::new();
    prefilled_transactions_indexes.insert(0);
    let compact_block = CompactBlock::build_from_block(&block, &prefilled_transactions_indexes);

    let mock_protocol_context = MockProtocolContext::new(SupportProtocols::RelayV2);
    let nc = Arc::new(mock_protocol_context);
    let peer_index: PeerIndex = 100.into();

    let compact_block_process = CompactBlockProcess::new(
        compact_block.as_reader(),
        &relayer,
        Arc::<MockProtocolContext>::clone(&nc),
        peer_index,
    );

    assert!(
        !relayer
            .shared
            .state()
            .contains_inflight_proposal(&proposal_id)
    );
    assert_eq!(
        compact_block_process.execute(),
        StatusCode::CompactBlockRequiresFreshTransactions.into()
    );

    let content = packed::GetBlockTransactions::new_builder()
        .block_hash(block.header().hash())
        .indexes([1u32].pack())
        .uncle_indexes([0u32].pack())
        .build();
    let message = packed::RelayMessage::new_builder().set(content).build();
    let data = message.as_bytes();

    // send missing indexes messages
    assert!(nc.has_sent(SupportProtocols::RelayV2.protocol_id(), peer_index, data));

    // insert inflight proposal
    assert!(
        relayer
            .shared
            .state()
            .contains_inflight_proposal(&proposal_id)
    );

    let content = packed::GetBlockProposal::new_builder()
        .block_hash(block.header().hash())
        .proposals(vec![proposal_id].into_iter().pack())
        .build();
    let message = packed::RelayMessage::new_builder().set(content).build();
    let data = message.as_bytes();

    // send proposal request
    assert!(nc.has_sent(SupportProtocols::RelayV2.protocol_id(), peer_index, data));
}

#[test]
fn test_accept_block() {
    let _log_guard = ckb_logger_service::init_for_test("info,ckb-chain=debug").expect("init log");

    let (relayer, _) = build_chain(5);
    let parent = {
        let active_chain = relayer.shared.active_chain();
        active_chain.tip_header()
    };

    let uncle = gen_block(&parent, relayer.shared().shared(), 0, 1, None);

    let block = gen_block(
        &parent,
        relayer.shared().shared(),
        0,
        1,
        Some(uncle.as_uncle()),
    );

    let mock_block_1 = BlockBuilder::default()
        .number(4.pack())
        .epoch(EpochNumberWithFraction::new(1, 4, 1000).pack())
        .build();
    let mock_compact_block_1 = CompactBlock::build_from_block(&mock_block_1, &Default::default());

    let mock_block_2 = block.as_advanced_builder().number(7.pack()).build();
    let mock_compact_block_2 = CompactBlock::build_from_block(&mock_block_2, &Default::default());
    {
        let mut pending_compact_blocks = relayer.shared.state().pending_compact_blocks();
        pending_compact_blocks.insert(
            mock_block_1.header().hash(),
            (
                mock_compact_block_1,
                HashMap::from_iter(vec![(1.into(), (vec![1], vec![0]))]),
                ckb_systemtime::unix_time_as_millis(),
            ),
        );

        pending_compact_blocks.insert(
            mock_block_2.header().hash(),
            (
                mock_compact_block_2,
                HashMap::from_iter(vec![(1.into(), (vec![1], vec![0]))]),
                ckb_systemtime::unix_time_as_millis(),
            ),
        );
    }

    {
        let proposal_table = ckb_proposal_table::ProposalTable::new(
            relayer.shared().shared().consensus().tx_proposal_window(),
        );
        let chain_service_builder = ChainServicesBuilder {
            shared: relayer.shared().shared().to_owned(),
            proposal_table,
        };

        let chain_controller = start_chain_services(chain_service_builder);

        chain_controller
            .blocking_process_block_with_switch(Arc::new(uncle), Switch::DISABLE_EXTENSION)
            .unwrap();
    }

    let mut prefilled_transactions_indexes = HashSet::new();
    prefilled_transactions_indexes.insert(0);
    let compact_block = CompactBlock::build_from_block(&block, &prefilled_transactions_indexes);

    let mock_protocol_context = MockProtocolContext::new(SupportProtocols::RelayV2);
    let nc = Arc::new(mock_protocol_context);
    let peer_index: PeerIndex = 100.into();

    let compact_block_process = CompactBlockProcess::new(
        compact_block.as_reader(),
        &relayer,
        Arc::<MockProtocolContext>::clone(&nc),
        peer_index,
    );
    assert_eq!(compact_block_process.execute(), Status::ok(),);

    let pending_compact_blocks = relayer.shared.state().pending_compact_blocks();
    assert!(
        pending_compact_blocks
            .get(&mock_block_1.header().hash())
            .is_none()
    );
    assert!(
        pending_compact_blocks
            .get(&mock_block_2.header().hash())
            .is_some()
    );
}

#[test]
fn test_ignore_a_too_old_block() {
    let (relayer, _) = build_chain(1804);

    let snapshot = relayer.shared.shared().snapshot();
    let parent = snapshot.tip_header();
    let parent = snapshot.get_ancestor(&parent.hash(), 2).unwrap();

    let too_old_block = new_header_builder(relayer.shared.shared(), &parent).build();

    let block = BlockBuilder::default()
        .header(too_old_block)
        .transaction(TransactionBuilder::default().build())
        .build();

    let mut prefilled_transactions_indexes = HashSet::new();
    prefilled_transactions_indexes.insert(0);
    let compact_block = CompactBlock::build_from_block(&block, &prefilled_transactions_indexes);

    let mock_protocol_context = MockProtocolContext::new(SupportProtocols::RelayV2);
    let nc = Arc::new(mock_protocol_context);
    let peer_index: PeerIndex = 1.into();

    let compact_block_process = CompactBlockProcess::new(
        compact_block.as_reader(),
        &relayer,
        Arc::<MockProtocolContext>::clone(&nc),
        peer_index,
    );

    assert_eq!(
        compact_block_process.execute(),
        StatusCode::CompactBlockIsStaled.into(),
    );
}

#[test]
fn test_invalid_transaction_root() {
    let (relayer, _) = build_chain(5);
    let parent = {
        let active_chain = relayer.shared.active_chain();
        active_chain.tip_header()
    };

    let header = new_header_builder(relayer.shared.shared(), &parent).build();

    let block = BlockBuilder::default()
        .header(header)
        .transaction(TransactionBuilder::default().build())
        .build_unchecked();

    let mut prefilled_transactions_indexes = HashSet::new();
    prefilled_transactions_indexes.insert(0);
    let compact_block = CompactBlock::build_from_block(&block, &prefilled_transactions_indexes);

    let mock_protocol_context = MockProtocolContext::new(SupportProtocols::RelayV2);
    let nc = Arc::new(mock_protocol_context);
    let peer_index: PeerIndex = 100.into();

    let compact_block_process = CompactBlockProcess::new(
        compact_block.as_reader(),
        &relayer,
        Arc::<MockProtocolContext>::clone(&nc),
        peer_index,
    );
    assert_eq!(
        compact_block_process.execute(),
        StatusCode::CompactBlockHasUnmatchedTransactionRootWithReconstructedBlock.into(),
    );
}

#[test]
fn test_collision() {
    let (relayer, _) = build_chain(5);

    let last_block = relayer
        .shared
        .store()
        .get_block(&relayer.shared.active_chain().tip_hash())
        .unwrap();
    let last_cellbase = last_block.transactions().first().cloned().unwrap();

    let missing_tx = TransactionBuilder::default()
        .output(
            CellOutputBuilder::default()
                .capacity(Capacity::bytes(1000).unwrap().pack())
                .build(),
        )
        .input(CellInput::new(OutPoint::new(last_cellbase.hash(), 0), 0))
        .output_data(Bytes::new().pack())
        .build();

    let fake_hash = missing_tx
        .hash()
        .as_builder()
        .nth31(0u8.into())
        .nth30(0u8.into())
        .nth29(0u8.into())
        .nth28(0u8.into())
        .build();
    // Fake tx with the same ProposalShortId but different hash with missing_tx
    let fake_tx = missing_tx.clone().fake_hash(fake_hash);

    assert_eq!(missing_tx.proposal_short_id(), fake_tx.proposal_short_id());
    assert_ne!(missing_tx.hash(), fake_tx.hash());

    let parent = {
        let tx_pool = relayer.shared.shared().tx_pool_controller();
        let entry = TxEntry::dummy_resolve(missing_tx, 0, Capacity::shannons(0), 0);
        tx_pool
            .plug_entry(vec![entry], PlugTarget::Pending)
            .unwrap();
        relayer.shared.active_chain().tip_header()
    };

    let header = new_header_builder(relayer.shared.shared(), &parent).build();

    let proposal_id = ProposalShortId::new([1u8; 10]);

    let block = BlockBuilder::default()
        .header(header)
        .transaction(TransactionBuilder::default().build())
        .transaction(fake_tx)
        .proposal(proposal_id.clone())
        .build_unchecked();

    let mut prefilled_transactions_indexes = HashSet::new();
    prefilled_transactions_indexes.insert(0);
    let compact_block = CompactBlock::build_from_block(&block, &prefilled_transactions_indexes);

    let mock_protocol_context = MockProtocolContext::new(SupportProtocols::RelayV2);
    let nc = Arc::new(mock_protocol_context);
    let peer_index: PeerIndex = 100.into();

    let compact_block_process = CompactBlockProcess::new(
        compact_block.as_reader(),
        &relayer,
        Arc::<MockProtocolContext>::clone(&nc),
        peer_index,
    );

    assert!(
        !relayer
            .shared
            .state()
            .contains_inflight_proposal(&proposal_id)
    );
    assert_eq!(
        compact_block_process.execute(),
        StatusCode::CompactBlockMeetsShortIdsCollision.into(),
    );

    let content = packed::GetBlockTransactions::new_builder()
        .block_hash(block.header().hash())
        .indexes([1u32].pack())
        .build();
    let message = packed::RelayMessage::new_builder().set(content).build();
    let data = message.as_bytes();

    // send missing indexes messages
    assert!(nc.has_sent(SupportProtocols::RelayV2.protocol_id(), peer_index, data));
}
