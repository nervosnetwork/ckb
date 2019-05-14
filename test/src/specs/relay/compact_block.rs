use crate::utils::{
    build_compact_block, build_compact_block_with_prefilled, clear_messages, wait_until,
};
use crate::{Net, Spec, TestProtocol};
use ckb_chain_spec::ChainSpec;
use ckb_core::header::HeaderBuilder;
use ckb_network::PeerIndex;
use ckb_protocol::{get_root, RelayMessage, RelayPayload, SyncMessage, SyncPayload};
use ckb_sync::NetworkProtocol;
use log::info;
use numext_fixed_hash::{h256, H256};

pub struct CompactBlockBasic;

impl CompactBlockBasic {
    // Case: Sent to node0 a parent-unknown empty block, node0 should be unable to reconstruct
    // it and send us back a `GetHeaders` message
    pub fn test_empty_parent_unknown_compact_block(&self, net: &Net, peer_ids: &[PeerIndex]) {
        // `node` generate 1 block to exit IBD mode.
        // `net` ignore relay message from `node0`
        let node0 = &net.nodes[0];
        let peer_id0 = peer_ids[0];
        node0.generate_block();
        let _ = net.receive();

        let parent_unknown_block = node0
            .new_block_builder(None, None, None)
            .header_builder(HeaderBuilder::default().parent_hash(h256!("0x123456")))
            .build();
        let tip_block = node0.get_tip_block();
        net.send(
            NetworkProtocol::RELAY.into(),
            peer_id0,
            build_compact_block(&parent_unknown_block),
        );
        let ret = wait_until(10, move || node0.get_tip_block() != tip_block);
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
    pub fn test_empty_compact_block(&self, net: &Net, peer_ids: &[PeerIndex]) {
        // `node` generate 1 block to exit IBD mode.
        // `net` ignore relay message from `node0`
        let node0 = &net.nodes[0];
        let peer_id0 = peer_ids[0];
        node0.generate_block();
        let _ = net.receive();

        let new_empty_block = node0.new_block(None, None, None);
        net.send(
            NetworkProtocol::RELAY.into(),
            peer_id0,
            build_compact_block(&new_empty_block),
        );
        let ret = wait_until(10, move || node0.get_tip_block() == new_empty_block);
        assert!(ret, "Node0 should reconstruct empty block successfully");

        clear_messages(net);
    }

    // Case: Send to node0 a block with all transactions prefilled, node0 should be able to reconstruct it
    pub fn test_all_prefilled_compact_block(&self, net: &Net, peer_ids: &[PeerIndex]) {
        // `node` generate 1 block to exit IBD mode.
        // `net` ignore relay message from `node0`
        let node0 = &net.nodes[0];
        let peer_id0 = peer_ids[0];
        node0.generate_block();
        let _ = net.receive();

        // Proposal a tx, and grow up into proposal window
        let new_tx = node0.new_transaction(node0.get_tip_block().transactions()[0].hash().clone());
        node0.submit_block(
            &node0
                .new_block_builder(None, None, None)
                .proposal(new_tx.proposal_short_id())
                .build(),
        );
        (0..3).for_each(|_| {
            node0.generate_block();
        });

        // Relay a block contains `new_tx` as committed
        let new_block = node0
            .new_block_builder(None, None, None)
            .transaction(new_tx)
            .build();
        net.send(
            NetworkProtocol::RELAY.into(),
            peer_id0,
            build_compact_block_with_prefilled(&new_block, vec![1]),
        );
        let ret = wait_until(50, move || node0.get_tip_block() == new_block);
        assert!(
            ret,
            "Node0 should reconstruct all-prefilled block successfully"
        );

        clear_messages(net);
    }

    // Case: Send to node0 a block which missing a tx, node0 should send `GetBlockTransactions`
    // back for requesting these missing txs
    pub fn test_missing_txs_compact_block(&self, net: &Net, peer_ids: &[PeerIndex]) {
        let node0 = &net.nodes[0];
        let peer_id0 = peer_ids[0];

        // Proposal a tx, and grow up into proposal window
        let new_tx = node0.new_transaction(node0.get_tip_block().transactions()[0].hash().clone());
        node0.submit_block(
            &node0
                .new_block_builder(None, None, None)
                .proposal(new_tx.proposal_short_id())
                .build(),
        );
        (0..3).for_each(|_| {
            node0.generate_block();
        });

        // Net consume and ignore the recent blocks
        (0..4).for_each(|_| {
            net.receive();
        });

        // Relay a block contains `new_tx` as committed, but not include in prefilled
        let new_block = node0
            .new_block_builder(None, None, None)
            .transaction(new_tx)
            .build();
        net.send(
            NetworkProtocol::RELAY.into(),
            peer_id0,
            build_compact_block(&new_block),
        );
        let ret = wait_until(10, move || node0.get_tip_block() == new_block);
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
}

impl Spec for CompactBlockBasic {
    fn run(&self, net: Net) {
        info!("Running CompactBlockBasic");

        info!("Connect nodes");
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
        self.test_empty_compact_block(&net, &peer_ids);
        self.test_empty_parent_unknown_compact_block(&net, &peer_ids);
        self.test_all_prefilled_compact_block(&net, &peer_ids);
        self.test_missing_txs_compact_block(&net, &peer_ids);
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
