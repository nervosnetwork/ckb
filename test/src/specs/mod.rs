mod mining;
mod p2p;
mod relay;
mod sync;
mod tx_pool;
mod utils;

pub use mining::*;
pub use p2p::*;
pub use relay::*;
pub use sync::*;
pub use tx_pool::*;
pub use utils::*;

use crate::Net;
use ckb_core::BlockNumber;
use ckb_network::{ProtocolId, ProtocolVersion};
use ckb_sync::NetworkProtocol;

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

impl TestProtocol {
    pub fn sync() -> Self {
        Self {
            id: NetworkProtocol::SYNC.into(),
            protocol_name: "syn".to_string(),
            supported_versions: vec!["1".to_string()],
        }
    }

    pub fn relay() -> Self {
        Self {
            id: NetworkProtocol::RELAY.into(),
            protocol_name: "rel".to_string(),
            supported_versions: vec!["1".to_string()],
        }
    }
}
