mod block_relay;
mod block_sync;
mod mining;
mod p2p;
mod pool;
mod protocols;
mod transaction_relay;

pub use block_relay::BlockRelayBasic;
pub use block_sync::BlockSyncBasic;
pub use mining::MiningBasic;
pub use p2p::{Disconnect, Discovery};
pub use pool::{PoolReconcile, PoolTrace};
pub use protocols::MalformedMessage;
pub use transaction_relay::TransactionRelayBasic;

use crate::{sleep, Net};
use ckb_network::{ProtocolId, ProtocolVersion};

pub trait Spec {
    fn run(&self, net: Net);

    fn num_nodes(&self) -> usize {
        3
    }

    fn connect_all(&self) -> bool {
        true
    }

    fn test_protocols(&self) -> Vec<TestProtocol> {
        vec![]
    }

    fn setup_net(&self, binary: &str, start_port: u16) -> Net {
        let mut net = Net::new(binary, self.num_nodes(), start_port, self.test_protocols());

        // start all nodes
        net.nodes.iter_mut().for_each(|node| {
            node.start();
        });

        // connect the nodes as a linear chain: node0 <-> node1 <-> node2 <-> ...
        if self.connect_all() {
            net.nodes
                .windows(2)
                .for_each(|nodes| nodes[0].connect(&nodes[1]));

            // workaround: waiting for all nodes connected
            // TODO: add getpeerinfo rpc
            sleep(5);
        }

        net
    }
}

pub struct TestProtocol {
    pub id: ProtocolId,
    pub protocol_name: String,
    pub supported_versions: Vec<ProtocolVersion>,
}
