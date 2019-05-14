mod mining;
mod p2p;
mod relay;
mod sync;
mod tx_pool;

pub use mining::*;
pub use p2p::*;
pub use relay::*;
pub use sync::*;
pub use tx_pool::*;

use crate::Net;
use ckb_app_config::CKBAppConfig;
use ckb_chain_spec::ChainSpecConfig;
use ckb_network::{ProtocolId, ProtocolVersion};
use ckb_sync::NetworkProtocol;

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

    fn modify_chain_spec(&self) -> Box<dyn Fn(&mut ChainSpecConfig) -> ()> {
        Box::new(|_| ())
    }

    fn modify_ckb_config(&self) -> Box<dyn Fn(&mut CKBAppConfig) -> ()> {
        Box::new(|config| config.network.connect_outbound_interval_secs = 1)
    }

    fn setup_net(&self, binary: &str, start_port: u16) -> Net {
        let mut net = Net::new(binary, self.num_nodes(), start_port, self.test_protocols());

        // start all nodes
        net.nodes.iter_mut().for_each(|node| {
            node.start(self.modify_chain_spec(), self.modify_ckb_config());
        });

        // connect the nodes as a linear chain: node0 <-> node1 <-> node2 <-> ...
        if self.connect_all() {
            net.connect_all();
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
