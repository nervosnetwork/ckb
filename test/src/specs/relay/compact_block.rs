use crate::{sleep, Net, Spec, TestProtocol};
use bytes::Bytes;
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::header::HeaderBuilder;
use ckb_protocol::{get_root, RelayMessage, SyncMessage, SyncPayload};
use ckb_sync::NetworkProtocol;
use flatbuffers::FlatBufferBuilder;
use log::info;
use numext_fixed_hash::{h256, H256};
use std::collections::HashSet;

pub struct CompactBlockBasic;

impl Spec for CompactBlockBasic {
    fn run(&self, net: Net) {
        info!("Running CompactBlockBasic");

        info!("Connect node0");
        let node0 = &net.nodes[0];
        net.connect(node0);
        // get peer_id from GetHeaders message
        let (peer_id, _, _) = net.receive();
        // generate 1 block on node0, to exit IBD mode.
        node0.generate_block();
        // ignore block relay message
        let _ = net.receive();

        info!("Send unknown parent hash compact block message to node0");
        let header_builder = HeaderBuilder::default().parent_hash(h256!("0x1"));
        let block = BlockBuilder::from_header_builder(header_builder).build();
        net.send(
            NetworkProtocol::RELAY.into(),
            peer_id,
            build_compact_block(&block),
        );

        info!("Node0 should send back GetHeaders message");
        let (_, _, data) = net.receive();
        let message = get_root::<SyncMessage>(&data).unwrap();
        assert_eq!(message.payload_type(), SyncPayload::GetHeaders);

        info!("Send valid compact block message to node0");
        let block = node0.new_block(None, None, None);
        net.send(
            NetworkProtocol::RELAY.into(),
            peer_id,
            build_compact_block(&block),
        );

        sleep(5);
        info!("Node0 should reconstruct block successfully");
        let tip_block = node0.get_tip_block();
        assert_eq!(block, tip_block);
    }

    fn num_nodes(&self) -> usize {
        1
    }

    fn test_protocols(&self) -> Vec<TestProtocol> {
        vec![TestProtocol::sync(), TestProtocol::relay()]
    }
}

fn build_compact_block(block: &Block) -> Bytes {
    let fbb = &mut FlatBufferBuilder::new();
    let message = RelayMessage::build_compact_block(fbb, &block, &HashSet::new());
    fbb.finish(message, None);
    fbb.finished_data().into()
}
