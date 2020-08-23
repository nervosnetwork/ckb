use crate::utils::message::{
    build_block, build_block_transactions, build_compact_block, build_compact_block_with_prefilled,
    build_header, build_headers,
};
use crate::utils::{clear_messages, wait_until};
use crate::{Net, Spec, TestProtocol, DEFAULT_TX_PROPOSAL_WINDOW};
use ckb_dao::DaoCalculator;
use ckb_network::{bytes::Bytes, SupportProtocols};
use ckb_test_chain_utils::MockStore;
use ckb_types::{
    core::{
        cell::{resolve_transaction, ResolvedTransaction},
        BlockBuilder, HeaderBuilder, HeaderView, TransactionBuilder,
    },
    h256,
    packed::{self, CellInput, GetHeaders, RelayMessage, SyncMessage},
    prelude::*,
    H256,
};
use std::collections::HashSet;
use std::time::Duration;

pub struct CompactBlockEmptyParentUnknown;

impl Spec for CompactBlockEmptyParentUnknown {
    crate::name!("compact_block_empty_parent_unknown");

    crate::setup!(protocols: vec![TestProtocol::sync(), TestProtocol::relay()]);

    // Case: Sent to node0 a parent-unknown empty block, node0 should be unable to reconstruct
    // it and send us back a `GetHeaders` message
    fn run(&self, net: &mut Net) {
        net.exit_ibd_mode();
        let node = net.node(0);
        net.connect(node);
        let (peer_id, _, _) = net.receive();

        node.generate_block();
        let _ = net.receive();

        let parent_unknown_block = node
            .new_block_builder(None, None, None)
            .header(
                HeaderBuilder::default()
                    .parent_hash(h256!("0x123456").pack())
                    .build(),
            )
            .build();
        let tip_block = node.get_tip_block();
        net.send(
            SupportProtocols::Relay.protocol_id(),
            peer_id,
            build_compact_block(&parent_unknown_block),
        );
        let ret = wait_until(10, move || node.get_tip_block() != tip_block);
        assert!(!ret, "Node0 should reconstruct empty block failed");

        net.should_receive(
            |data: &Bytes| {
                SyncMessage::from_slice(&data)
                    .map(|message| message.to_enum().item_name() == GetHeaders::NAME)
                    .unwrap_or(false)
            },
            "Node0 should send back GetHeaders message for unknown parent header",
        );
    }
}

pub struct CompactBlockEmpty;

impl Spec for CompactBlockEmpty {
    crate::name!("compact_block_empty");

    crate::setup!(protocols: vec![TestProtocol::sync(), TestProtocol::relay()]);

    // Case: Send to node0 a parent-known empty block, node0 should be able to reconstruct it
    fn run(&self, net: &mut Net) {
        let node = net.node(0);
        net.exit_ibd_mode();
        net.connect(node);
        let (peer_id, _, _) = net.receive();

        let new_empty_block = node.new_block(None, None, None);
        net.send(
            SupportProtocols::Relay.protocol_id(),
            peer_id,
            build_compact_block(&new_empty_block),
        );
        let ret = wait_until(10, move || node.get_tip_block() == new_empty_block);
        assert!(ret, "Node0 should reconstruct empty block successfully");
    }
}

pub struct CompactBlockPrefilled;

impl Spec for CompactBlockPrefilled {
    crate::name!("compact_block_prefilled");

    crate::setup!(protocols: vec![TestProtocol::sync(), TestProtocol::relay()]);

    // Case: Send to node0 a block with all transactions prefilled, node0 should be able to reconstruct it
    fn run(&self, net: &mut Net) {
        let node = net.node(0);
        net.exit_ibd_mode();
        net.connect(node);
        let (peer_id, _, _) = net.receive();
        node.generate_blocks((DEFAULT_TX_PROPOSAL_WINDOW.1 + 2) as usize);

        // Proposal a tx, and grow up into proposal window
        let new_tx = node.new_transaction(node.get_tip_block().transactions()[0].hash());
        node.submit_block(
            &node
                .new_block_builder(None, None, None)
                .proposal(new_tx.proposal_short_id())
                .build(),
        );
        node.generate_blocks(3);

        // Relay a block contains `new_tx` as committed
        let new_block = node
            .new_block_builder(None, None, None)
            .transaction(new_tx)
            .build();
        net.send(
            SupportProtocols::Relay.protocol_id(),
            peer_id,
            build_compact_block_with_prefilled(&new_block, vec![1]),
        );
        let ret = wait_until(10, move || node.get_tip_block() == new_block);
        assert!(
            ret,
            "Node0 should reconstruct all-prefilled block successfully"
        );
    }
}

pub struct CompactBlockMissingFreshTxs;

impl Spec for CompactBlockMissingFreshTxs {
    crate::name!("compact_block_missing_fresh_txs");

    crate::setup!(protocols: vec![TestProtocol::sync(), TestProtocol::relay()]);

    // Case: Send to node0 a block which missing a tx, which is a fresh tx for
    // tx_pool, node0 should send `GetBlockTransactions` back for requesting
    // these missing txs
    fn run(&self, net: &mut Net) {
        let node = net.node(0);
        net.exit_ibd_mode();
        net.connect(node);
        let (peer_id, _, _) = net.receive();

        node.generate_blocks((DEFAULT_TX_PROPOSAL_WINDOW.1 + 2) as usize);
        let new_tx = node.new_transaction(node.get_tip_block().transactions()[0].hash());
        node.submit_block(
            &node
                .new_block_builder(None, None, None)
                .proposal(new_tx.proposal_short_id())
                .build(),
        );
        node.generate_blocks(3);

        // Net consume and ignore the recent blocks
        (0..(DEFAULT_TX_PROPOSAL_WINDOW.1 + 6)).for_each(|_| {
            net.receive();
        });

        // Relay a block contains `new_tx` as committed, but not include in prefilled
        let new_block = node
            .new_block_builder(None, None, None)
            .transaction(new_tx)
            .build();
        net.send(
            SupportProtocols::Relay.protocol_id(),
            peer_id,
            build_compact_block(&new_block),
        );
        let ret = wait_until(10, move || node.get_tip_block() == new_block);
        assert!(!ret, "Node0 should be unable to reconstruct the block");

        net.should_receive(
            |data: &Bytes| {
                let get_block_txns = RelayMessage::from_slice(&data)
                    .map(|message| {
                        message.to_enum().item_name() == packed::GetBlockTransactions::NAME
                    })
                    .unwrap_or(false);
                let get_block = SyncMessage::from_slice(&data)
                    .map(|message| message.to_enum().item_name() == packed::GetBlocks::NAME)
                    .unwrap_or(false);
                get_block_txns || get_block
            },
            "Node0 should send GetBlockTransactions message for missing transactions",
        );
    }
}

pub struct CompactBlockMissingNotFreshTxs;

impl Spec for CompactBlockMissingNotFreshTxs {
    crate::name!("compact_block_missing_not_fresh_txs");

    crate::setup!(protocols: vec![TestProtocol::sync(), TestProtocol::relay()]);

    // Case: As for the missing transactions of a compact block, we should try to find it from
    //       tx_pool. If we find out, we can reconstruct the target block without any requests
    //       to the peer.
    // 1. Put the target tx into tx_pool, and proposal it. Then move it into proposal window
    // 2. Relay target block which contains the target transaction as committed transaction. Expect
    //    successful to reconstruct the target block and grow up.
    fn run(&self, net: &mut Net) {
        let node = net.node(0);
        net.exit_ibd_mode();
        net.connect(node);
        let (peer_id, _, _) = net.receive();
        node.generate_blocks((DEFAULT_TX_PROPOSAL_WINDOW.1 + 2) as usize);

        // Build the target transaction
        let new_tx = node.new_transaction(node.get_tip_block().transactions()[0].hash());
        node.submit_block(
            &node
                .new_block_builder(None, None, None)
                .proposal(new_tx.proposal_short_id())
                .build(),
        );
        node.generate_blocks(3);

        // Generate the target block which contains the target transaction as a committed transaction
        let new_block = node
            .new_block_builder(None, None, None)
            .transaction(new_tx.clone())
            .build();

        // Put `new_tx` as an not fresh tx into tx_pool
        node.rpc_client().send_transaction(new_tx.data().into());

        // Relay the target block
        clear_messages(&net);
        net.send(
            SupportProtocols::Relay.protocol_id(),
            peer_id,
            build_compact_block(&new_block),
        );
        let ret = wait_until(10, move || node.get_tip_block() == new_block);
        assert!(ret, "Node0 should be able to reconstruct the block");
    }
}

pub struct CompactBlockLoseGetBlockTransactions;

impl Spec for CompactBlockLoseGetBlockTransactions {
    crate::name!("compact_block_lose_get_block_transactions");

    crate::setup!(
        num_nodes: 2,
        connect_all: false,
        protocols: vec![TestProtocol::sync(), TestProtocol::relay()],
    );

    fn run(&self, net: &mut Net) {
        net.exit_ibd_mode();
        let node0 = net.node(0);
        net.connect(node0);
        let (peer_id0, _, _) = net.receive();
        let node1 = net.node(1);
        net.connect(node1);
        let _ = net.receive();
        node0.generate_blocks((DEFAULT_TX_PROPOSAL_WINDOW.1 + 2) as usize);

        let new_tx = node0.new_transaction(node0.get_tip_block().transactions()[0].hash());
        node0.submit_block(
            &node0
                .new_block_builder(None, None, None)
                .proposal(new_tx.proposal_short_id())
                .build(),
        );
        // Proposal a tx, and grow up into proposal window
        node0.generate_blocks(6);

        // Make node0 and node1 reach the same height
        node1.generate_block();
        node0.connect(node1);
        node0.waiting_for_sync(node1, node0.get_tip_block().header().number());

        // Net consume and ignore the recent blocks
        clear_messages(&net);

        // Construct a new block contains one transaction
        let block = node0
            .new_block_builder(None, None, None)
            .transaction(new_tx)
            .build();

        // Net send the compact block to node0, but dose not send the corresponding missing
        // block transactions. It will make node0 unable to reconstruct the complete block
        net.send(
            SupportProtocols::Relay.protocol_id(),
            peer_id0,
            build_compact_block(&block),
        );

        net.should_receive(
            |data: &Bytes| {
                let get_block_txns = RelayMessage::from_slice(&data)
                    .map(|message| {
                        message.to_enum().item_name() == packed::GetBlockTransactions::NAME
                    })
                    .unwrap_or(false);
                let get_block = SyncMessage::from_slice(&data)
                    .map(|message| message.to_enum().item_name() == packed::GetBlocks::NAME)
                    .unwrap_or(false);
                get_block_txns || get_block
            },
            "Node0 should send GetBlockTransactions message for missing transactions",
        );

        // Submit the new block to node1. We expect node1 will relay the new block to node0.
        node1.submit_block(&block);
        node1.waiting_for_sync(node0, node1.get_tip_block().header().number());
    }
}

pub struct CompactBlockRelayParentOfOrphanBlock;

impl Spec for CompactBlockRelayParentOfOrphanBlock {
    crate::name!("compact_block_relay_parent_of_orphan_block");

    crate::setup!(protocols: vec![TestProtocol::sync(), TestProtocol::relay()]);

    // Case: A <- B, A == B.parent
    // 1. Sync B to node0. Node0 will put B into orphan_block_pool since B's parent unknown
    // 2. Relay A to node0. Node0 will handle A, and by the way process B, which is in
    // orphan_block_pool now
    fn run(&self, net: &mut Net) {
        let node = net.node(0);
        net.exit_ibd_mode();

        node.generate_blocks((DEFAULT_TX_PROPOSAL_WINDOW.1 + 2) as usize);
        // Proposal a tx, and grow up into proposal window
        let new_tx = node.new_transaction_spend_tip_cellbase();
        node.submit_block(
            &node
                .new_block_builder(None, None, None)
                .proposal(new_tx.proposal_short_id())
                .build(),
        );
        node.generate_blocks(6);

        let consensus = node.consensus();
        let mock_store = MockStore::default();
        for i in 0..=node.get_tip_block_number() {
            let block = node.get_block_by_number(i);
            mock_store.insert_block(&block, consensus.genesis_epoch_ext());
        }

        let parent = node
            .new_block_builder(None, None, None)
            .transaction(new_tx)
            .build();
        let mut seen_inputs = HashSet::new();
        let transactions = parent.transactions();
        let rtxs: Vec<ResolvedTransaction> = transactions
            .into_iter()
            .map(|tx| resolve_transaction(tx, &mut seen_inputs, &mock_store, &mock_store).unwrap())
            .collect();
        let calculator = DaoCalculator::new(&consensus, mock_store.store());
        let dao = calculator
            .dao_field(&rtxs, &node.get_tip_block().header())
            .unwrap();
        let header = parent.header().as_advanced_builder().dao(dao).build();
        let parent = parent.as_advanced_builder().header(header).build();
        mock_store.insert_block(&parent, consensus.genesis_epoch_ext());

        let fakebase = node.new_block(None, None, None).transactions()[0].clone();
        let output = fakebase
            .outputs()
            .as_reader()
            .get(0)
            .unwrap()
            .to_entity()
            .as_builder()
            .capacity(
                calculator
                    .base_block_reward(&parent.header())
                    .unwrap()
                    .pack(),
            )
            .build();
        let output_data = fakebase
            .outputs_data()
            .as_reader()
            .get(0)
            .unwrap()
            .to_entity();

        let cellbase = TransactionBuilder::default()
            .output(output)
            .output_data(output_data)
            .witness(fakebase.witnesses().as_reader().get(0).unwrap().to_entity())
            .input(CellInput::new_cellbase_input(parent.header().number() + 1))
            .build();
        let rtxs = vec![resolve_transaction(
            cellbase.clone(),
            &mut HashSet::new(),
            &mock_store,
            &mock_store,
        )
        .unwrap()];
        let dao = DaoCalculator::new(&consensus, mock_store.store())
            .dao_field(&rtxs, &parent.header())
            .unwrap();
        let block = BlockBuilder::default()
            .transaction(cellbase)
            .header(
                parent
                    .header()
                    .as_advanced_builder()
                    .number((parent.header().number() + 1).pack())
                    .timestamp((parent.header().timestamp() + 1).pack())
                    .parent_hash(parent.hash())
                    .dao(dao)
                    .epoch(
                        consensus
                            .genesis_epoch_ext()
                            .number_with_fraction(parent.header().number() + 1)
                            .pack(),
                    )
                    .build(),
            )
            .build();
        let old_tip = node.get_tip_block().header().number();

        net.connect(node);
        let (peer_id, _, _) = net.receive();

        net.send(
            SupportProtocols::Relay.protocol_id(),
            peer_id,
            build_compact_block(&parent),
        );

        net.send(
            SupportProtocols::Sync.protocol_id(),
            peer_id,
            build_header(&parent.header()),
        );
        net.send(
            SupportProtocols::Sync.protocol_id(),
            peer_id,
            build_header(&block.header()),
        );

        net.send(
            SupportProtocols::Relay.protocol_id(),
            peer_id,
            build_block_transactions(&parent),
        );

        clear_messages(&net);
        net.send(
            SupportProtocols::Sync.protocol_id(),
            peer_id,
            build_block(&block),
        );

        let ret = wait_until(20, move || {
            node.get_tip_block().header().number() == old_tip + 2
        });
        assert!(
            ret,
            "relayer should process the two blocks, including the orphan block"
        );
    }
}

pub struct CompactBlockRelayLessThenSharedBestKnown;

impl Spec for CompactBlockRelayLessThenSharedBestKnown {
    crate::name!("compact_block_relay_less_then_shared_best_known");

    crate::setup!(
        num_nodes: 2,
        connect_all: false,
        protocols: vec![TestProtocol::sync(), TestProtocol::relay()],
    );

    // Case: Relay a compact block which has lower total difficulty than shared_best_known
    // 1. Synchronize Headers[Tip+1, Tip+10]
    // 2. Relay CompactBlock[Tip+1]
    fn run(&self, net: &mut Net) {
        let node0 = net.node(0);
        let node1 = net.node(1);
        net.exit_ibd_mode();
        net.connect(node0);
        let (peer_id, _, _) = net.receive();

        assert_eq!(node0.get_tip_block(), node1.get_tip_block());
        let old_tip = node1.get_tip_block_number();
        node1.generate_blocks(10);
        let headers: Vec<HeaderView> = (old_tip + 1..node1.get_tip_block_number())
            .map(|i| node1.rpc_client().get_header_by_number(i).unwrap().into())
            .collect();
        net.send(
            SupportProtocols::Sync.protocol_id(),
            peer_id,
            build_headers(&headers),
        );
        {
            let (_, _, data) = net.receive_timeout(Duration::from_secs(5)).expect("");
            assert_eq!(
                SyncMessage::from_slice(&data)
                    .unwrap()
                    .to_enum()
                    .item_name(),
                packed::GetBlocks::NAME,
                "Node0 should send GetBlocks message",
            );
        }

        let new_block = node0.new_block(None, None, None);
        net.send(
            SupportProtocols::Relay.protocol_id(),
            peer_id,
            build_compact_block(&new_block),
        );
        assert!(
            wait_until(20, move || node0.get_tip_block().header().number() == old_tip + 1),
            "node0 should process the new block, even its difficulty is less than best_shared_known",
        );
    }
}
