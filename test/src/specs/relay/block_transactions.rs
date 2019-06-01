use crate::{
    utils::{build_block_transactions, build_compact_block, clear_messages},
    Net, Node, Spec, TestProtocol,
};
use ckb_chain_spec::ChainSpec;
use ckb_network::PeerIndex;
use ckb_protocol::{get_root, RelayMessage, RelayPayload};
use ckb_sync::NetworkProtocol;
use std::thread::sleep;
use std::time::Duration;

pub struct RelayBlockTransactions;

impl RelayBlockTransactions {
    pub fn test_compact_block_contains_invalid_missing_transactions(
        &self,
        net: &Net,
        node: &Node,
        peer_id: PeerIndex,
    ) {
        node.generate_block();
        let _ = net.receive();

        // Construct a new block contains a invalid transaction
        let tip_block = node.get_tip_block();
        let new_tx = node.new_transaction(tip_block.transactions()[0].hash().clone());
        let new_block = node
            .new_block_builder(None, None, None)
            .transaction(new_tx)
            .build();

        // Net send the compact block to node0, but dose not send the corresponding missing
        // block transactions. It will make node0 unable to reconstruct the complete block
        net.send(
            NetworkProtocol::RELAY.into(),
            peer_id,
            build_compact_block(&new_block),
        );
        let (_, _, data) = net
            .receive_timeout(Duration::from_secs(10))
            .expect("receive GetBlockTransactions");
        let message = get_root::<RelayMessage>(&data).unwrap();
        assert_eq!(
            message.payload_type(),
            RelayPayload::GetBlockTransactions,
            "Node should send GetBlockTransactions message for missing transactions",
        );

        // Net send the corresponding invalid transactions to node
        // And then, we should be banned by node
        net.send(
            NetworkProtocol::RELAY.into(),
            peer_id,
            build_block_transactions(&new_block),
        );

        // Let's attempt to confirm that we were baned by node
        sleep(Duration::from_secs(5));
        node.generate_block();
        assert!(
            net.receive_timeout(Duration::from_secs(5)).is_err(),
            "Node should ban us, so we cannot receive node's new block"
        );
    }
}

impl Spec for RelayBlockTransactions {
    fn run(&self, net: Net) {
        log::info!("Running RelayBlockTransactions");
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

        // node0 ban us in this case
        self.test_compact_block_contains_invalid_missing_transactions(
            &net,
            &net.nodes[0],
            peer_ids[0],
        );
    }

    fn test_protocols(&self) -> Vec<TestProtocol> {
        vec![TestProtocol::sync(), TestProtocol::relay()]
    }

    fn connect_all(&self) -> bool {
        false
    }

    fn modify_chain_spec(&self) -> Box<dyn Fn(&mut ChainSpec) -> ()> {
        Box::new(|mut spec_config| {
            // Test cases of relaying block transactions care about the validity of transactions,
            // so here give the realistic parameters
            spec_config.params.cellbase_maturity = 12;
        })
    }
}
