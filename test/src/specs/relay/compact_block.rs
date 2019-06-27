use crate::utils::{
    build_block, build_block_transactions, build_compact_block, build_compact_block_with_prefilled,
    build_header, clear_messages, wait_until,
};
use crate::{Net, Node, Spec, TestProtocol};
use ckb_chain_spec::ChainSpec;
use ckb_core::block::BlockBuilder;
use ckb_core::header::HeaderBuilder;
use ckb_core::transaction::{CellInput, TransactionBuilder};
use ckb_network::PeerIndex;
use ckb_protocol::{get_root, RelayMessage, RelayPayload, SyncMessage, SyncPayload};
use ckb_sync::NetworkProtocol;
use log::info;
use numext_fixed_hash::{h256, H256};
use std::time::Duration;

pub struct CompactBlockBasic;

impl CompactBlockBasic {
    // Case: Sent to node0 a parent-unknown empty block, node0 should be unable to reconstruct
    // it and send us back a `GetHeaders` message
    pub fn test_empty_parent_unknown_compact_block(
        &self,
        net: &Net,
        node: &Node,
        peer_id: PeerIndex,
    ) {
        node.generate_block();
        let _ = net.receive();

        let parent_unknown_block = node
            .new_block_builder(None, None, None)
            .header_builder(HeaderBuilder::default().parent_hash(h256!("0x123456")))
            .build();
        let tip_block = node.get_tip_block();
        net.send(
            NetworkProtocol::RELAY.into(),
            peer_id,
            build_compact_block(&parent_unknown_block),
        );
        let ret = wait_until(10, move || node.get_tip_block() != tip_block);
        assert!(!ret, "Node0 should reconstruct empty block failed");

        let (_, _, data) = net.receive();
        let message = get_root::<SyncMessage>(&data).unwrap();
        assert_eq!(
            message.payload_type(),
            SyncPayload::GetHeaders,
            "Node0 should send back GetHeaders message for unknown parent header"
        );
    }

    // Case: Send to node0 a parent-known empty block, node0 should be able to reconstruct it
    pub fn test_empty_compact_block(&self, net: &Net, node: &Node, peer_id: PeerIndex) {
        node.generate_block();
        let _ = net.receive();

        let new_empty_block = node.new_block(None, None, None);
        net.send(
            NetworkProtocol::RELAY.into(),
            peer_id,
            build_compact_block(&new_empty_block),
        );
        let ret = wait_until(10, move || node.get_tip_block() == new_empty_block);
        assert!(ret, "Node0 should reconstruct empty block successfully");

        clear_messages(net);
    }

    // Case: Send to node0 a block with all transactions prefilled, node0 should be able to reconstruct it
    pub fn test_all_prefilled_compact_block(&self, net: &Net, node: &Node, peer_id: PeerIndex) {
        node.generate_block();
        let _ = net.receive();

        // Proposal a tx, and grow up into proposal window
        let new_tx = node.new_transaction(node.get_tip_block().transactions()[0].hash().clone());
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
            NetworkProtocol::RELAY.into(),
            peer_id,
            build_compact_block_with_prefilled(&new_block, vec![1]),
        );
        let ret = wait_until(10, move || node.get_tip_block() == new_block);
        assert!(
            ret,
            "Node0 should reconstruct all-prefilled block successfully"
        );

        clear_messages(net);
    }

    // Case: Send to node0 a block which missing a tx, node0 should send `GetBlockTransactions`
    // back for requesting these missing txs
    pub fn test_missing_txs_compact_block(&self, net: &Net, node: &Node, peer_id: PeerIndex) {
        // Proposal a tx, and grow up into proposal window
        let new_tx = node.new_transaction(node.get_tip_block().transactions()[0].hash().clone());
        node.submit_block(
            &node
                .new_block_builder(None, None, None)
                .proposal(new_tx.proposal_short_id())
                .build(),
        );
        node.generate_blocks(3);

        // Net consume and ignore the recent blocks
        (0..4).for_each(|_| {
            net.receive();
        });

        // Relay a block contains `new_tx` as committed, but not include in prefilled
        let new_block = node
            .new_block_builder(None, None, None)
            .transaction(new_tx)
            .build();
        net.send(
            NetworkProtocol::RELAY.into(),
            peer_id,
            build_compact_block(&new_block),
        );
        let ret = wait_until(10, move || node.get_tip_block() == new_block);
        assert!(!ret, "Node0 should be unable to reconstruct the block");

        let (_, _, data) = net.receive();
        let message = get_root::<RelayMessage>(&data).unwrap();
        assert_eq!(
            message.payload_type(),
            RelayPayload::GetBlockTransactions,
            "Node0 should send GetBlockTransactions message for missing transactions",
        );

        clear_messages(net);
    }

    pub fn test_lose_get_block_transactions(
        &self,
        net: &Net,
        node0: &Node,
        node1: &Node,
        peer_id0: PeerIndex,
    ) {
        node0.generate_block();
        let new_tx = node0.new_transaction(node0.get_tip_block().transactions()[0].hash().clone());
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
        clear_messages(net);

        // Construct a new block contains one transaction
        let block = node0
            .new_block_builder(None, None, None)
            .transaction(new_tx)
            .build();

        // Net send the compact block to node0, but dose not send the corresponding missing
        // block transactions. It will make node0 unable to reconstruct the complete block
        net.send(
            NetworkProtocol::RELAY.into(),
            peer_id0,
            build_compact_block(&block),
        );
        let (_, _, data) = net
            .receive_timeout(Duration::from_secs(10))
            .expect("receive GetBlockTransactions");
        let message = get_root::<RelayMessage>(&data).unwrap();
        assert_eq!(
            message.payload_type(),
            RelayPayload::GetBlockTransactions,
            "Node0 should send GetBlockTransactions message for missing transactions",
        );

        // Submit the new block to node1. We expect node1 will relay the new block to node0.
        node1.submit_block(&block);
        node1.waiting_for_sync(node0, node1.get_tip_block().header().number());
    }

    // Case: A <- B, A == B.parent
    // 1. Sync B to node0. Node0 will put B into orphan_block_pool since B's parent unknown
    // 2. Relay A to node0. Node0 will handle A, and by the way process B, which is in
    // orphan_block_pool now
    pub fn test_relay_parent_of_orphan_block(&self, net: &Net, node: &Node, peer_id: PeerIndex) {
        node.generate_block();

        // Proposal a tx, and grow up into proposal window
        let new_tx = node.new_transaction_spend_tip_cellbase();
        node.submit_block(
            &node
                .new_block_builder(None, None, None)
                .proposal(new_tx.proposal_short_id())
                .build(),
        );
        node.generate_blocks(6);

        let parent = node
            .new_block_builder(None, None, None)
            .transaction(new_tx)
            .build();
        let fakebase = node.new_block(None, None, None).transactions()[0].clone();
        let cellbase = TransactionBuilder::default()
            .output(fakebase.outputs()[0].clone())
            .witness(fakebase.witnesses()[0].clone())
            .input(CellInput::new_cellbase_input(parent.header().number() + 1))
            .build();
        let block = BlockBuilder::default()
            .transaction(cellbase)
            .header_builder(
                HeaderBuilder::from_header(parent.header().to_owned())
                    .number(parent.header().number() + 1)
                    .timestamp(parent.header().timestamp() + 1)
                    .parent_hash(parent.header().hash().to_owned()),
            )
            .build();
        let old_tip = node.get_tip_block().header().number();

        net.send(
            NetworkProtocol::SYNC.into(),
            peer_id,
            build_header(parent.header()),
        );
        net.send(
            NetworkProtocol::RELAY.into(),
            peer_id,
            build_compact_block(&parent),
        );
        net.send(
            NetworkProtocol::SYNC.into(),
            peer_id,
            build_header(block.header()),
        );

        // Wait until node0 send GetBlocks
        loop {
            let (_, _, data) = net
                .receive_timeout(Duration::from_secs(5))
                .expect("wait GetBlocks");
            if let Ok(message) = get_root::<SyncMessage>(&data) {
                if message.payload_type() == SyncPayload::GetBlocks {
                    break;
                }
            }
        }
        net.send(NetworkProtocol::SYNC.into(), peer_id, build_block(&block));
        net.send(
            NetworkProtocol::RELAY.into(),
            peer_id,
            build_block_transactions(&parent),
        );

        let ret = wait_until(10, move || {
            node.get_tip_block().header().number() == old_tip + 2
        });
        assert!(
            ret,
            "relayer should process the two blocks, including the orphan block"
        );
    }
}

impl Spec for CompactBlockBasic {
    fn run(&self, net: Net) {
        info!("Running CompactBlockBasic");

        let peer_ids = net
            .nodes
            .iter()
            .map(|node| {
                net.connect(node);
                let (peer_id, _, _) = net.receive();
                peer_id
            })
            .collect::<Vec<PeerIndex>>();

        clear_messages(&net);
        self.test_empty_compact_block(&net, &net.nodes[0], peer_ids[0]);
        self.test_empty_parent_unknown_compact_block(&net, &net.nodes[0], peer_ids[0]);
        self.test_all_prefilled_compact_block(&net, &net.nodes[0], peer_ids[0]);
        self.test_missing_txs_compact_block(&net, &net.nodes[0], peer_ids[0]);
        self.test_relay_parent_of_orphan_block(&net, &net.nodes[0], peer_ids[0]);
        self.test_lose_get_block_transactions(&net, &net.nodes[0], &net.nodes[1], peer_ids[0]);
    }

    fn test_protocols(&self) -> Vec<TestProtocol> {
        vec![TestProtocol::sync(), TestProtocol::relay()]
    }

    fn connect_all(&self) -> bool {
        false
    }

    fn modify_chain_spec(&self) -> Box<dyn Fn(&mut ChainSpec) -> ()> {
        Box::new(|mut spec_config| {
            spec_config.params.cellbase_maturity = 5;
        })
    }
}
