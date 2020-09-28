use crate::node::exit_ibd_mode;
use crate::{Net, Node, Spec};
use ckb_network::{bytes::Bytes, SupportProtocols};
use ckb_types::{
    core::UncleBlockView,
    packed::{self, RelayMessage},
    prelude::*,
};

pub struct MissingUncleRequest;

impl Spec for MissingUncleRequest {
    // Case: Send to node GetBlockTransactions with missing uncle index, node should response BlockTransactions with uncles
    fn run(&self, nodes: &mut Vec<Node>) {
        exit_ibd_mode(nodes);
        let node = &nodes[0];
        let net = Net::new(
            self.name(),
            node.consensus().clone(),
            vec![SupportProtocols::Sync, SupportProtocols::Relay],
        );
        net.connect(node);
        let (peer_id, _, _) = net.receive();

        node.generate_block();
        let _ = net.receive();

        let builder = node.new_block_builder(None, None, None);
        let block1 = builder.clone().nonce(0.pack()).build();
        let block2 = builder.nonce(1.pack()).build();
        node.submit_block(&block1);
        node.submit_block(&block2);

        let builder = node.new_block_builder(None, None, None);
        let block = builder
            .set_uncles(vec![block2.as_uncle()])
            .nonce(0.pack())
            .build();
        node.submit_block(&block);

        let content = packed::GetBlockTransactions::new_builder()
            .block_hash(block.hash())
            .uncle_indexes(vec![0u32].pack())
            .build();
        let message = packed::RelayMessage::new_builder().set(content).build();

        (0..3).for_each(|_| {
            net.receive(); // ignore three new block announce
        });

        net.send(
            SupportProtocols::Relay.protocol_id(),
            peer_id,
            message.as_bytes(),
        );

        net.should_receive(
            |data: &Bytes| {
                RelayMessage::from_slice(&data)
                    .map(|message| message.to_enum().item_name() == packed::BlockTransactions::NAME)
                    .unwrap_or(false)
            },
            "Node should response BlockTransactions message",
        );

        if let packed::RelayMessageUnionReader::BlockTransactions(reader) =
            message.to_enum().as_reader()
        {
            let block_transactions = reader.to_entity();
            let received_uncles: Vec<UncleBlockView> = block_transactions
                .uncles()
                .into_iter()
                .map(|uncle| uncle.into_view())
                .collect();
            assert_eq!(received_uncles[0], block2.as_uncle());
        }
    }
}
