mod block_relay;
mod block_sync;
mod mining;
mod p2p;
mod pool;
mod protocols;
mod transaction_relay;
mod tx_pool;

pub use block_relay::BlockRelayBasic;
pub use block_sync::BlockSyncBasic;
pub use mining::MiningBasic;
pub use p2p::{Disconnect, Discovery};
pub use pool::{PoolReconcile, PoolTrace};
pub use protocols::MalformedMessage;
pub use transaction_relay::TransactionRelayBasic;
pub use tx_pool::{CellbaseImmatureTx, DepentTxInSameBlock};

use crate::Net;
use ckb_core::BlockNumber;
use ckb_network::{ProtocolId, ProtocolVersion};

pub trait Spec {
    fn run(&self, net: Net);

    fn num_nodes(&self) -> usize {
        3
    }

    fn cellbase_maturity(&self) -> Option<BlockNumber> {
        None
    }

    fn connect_all(&self) -> bool {
        true
    }

    fn test_protocols(&self) -> Vec<TestProtocol> {
        vec![]
    }

    fn setup_net(&self, binary: &str, start_port: u16) -> Net {
        let mut net = Net::new(
            binary,
            self.num_nodes(),
            start_port,
            self.test_protocols(),
            self.cellbase_maturity(),
        );

        // start all nodes
        net.nodes.iter_mut().for_each(|node| {
            node.start();
        });

        // connect the nodes as a linear chain: node0 <-> node1 <-> node2 <-> ...
        if self.connect_all() {
            net.nodes
                .windows(2)
                .for_each(|nodes| nodes[0].connect(&nodes[1]));
        }

        net
    }
}

pub struct TestProtocol {
    pub id: ProtocolId,
    pub protocol_name: String,
    pub supported_versions: Vec<ProtocolVersion>,
}
