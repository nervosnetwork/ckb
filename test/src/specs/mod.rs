mod alert;
mod consensus;
mod dao;
mod indexer;
mod mining;
mod p2p;
mod relay;
mod rpc;
mod sync;
mod tx_pool;

pub use alert::*;
pub use consensus::*;
pub use dao::*;
pub use indexer::*;
pub use mining::*;
pub use p2p::*;
pub use relay::*;
pub use rpc::*;
pub use sync::*;
pub use tx_pool::*;

use crate::Net;
use ckb_app_config::CKBAppConfig;
use ckb_chain_spec::ChainSpec;
use ckb_fee_estimator::FeeRate;
use ckb_network::{ProtocolId, ProtocolVersion};
use ckb_sync::NetworkProtocol;

#[macro_export]
macro_rules! name {
    ($name:literal) => {
        fn name(&self) -> &'static str {
            $name
        }
    };
}

#[macro_export]
macro_rules! setup {
    ($($setup:tt)*) => {
        fn setup(&self) -> $crate::Setup{ crate::setup_internal!($($setup)*) }
    };
}

#[macro_export]
macro_rules! setup_internal {
    ($field:ident: $value:expr,) => {
        crate::setup_internal!($field: $value)
    };
    ($field:ident: $value:expr) => {
        $crate::Setup{ $field: $value, ..Default::default() }
    };
    ($field:ident: $value:expr, $($rest:tt)*) =>  {
        $crate::Setup{ $field: $value, ..crate::setup_internal!($($rest)*) }
    };
}

pub struct Setup {
    pub num_nodes: usize,
    pub connect_all: bool,
    pub protocols: Vec<TestProtocol>,
    pub retry_failed: usize,
}

impl Default for Setup {
    fn default() -> Self {
        Setup {
            num_nodes: 1,
            connect_all: true,
            protocols: vec![],
            retry_failed: 0,
        }
    }
}

pub trait Spec {
    fn name(&self) -> &'static str;

    fn setup(&self) -> Setup {
        Setup::default()
    }

    fn init_config(&self, net: &mut Net) {
        net.nodes.iter_mut().for_each(|node| {
            node.edit_config_file(self.modify_chain_spec(), self.modify_ckb_config());
        });
    }

    fn before_run(&self, _net: &mut Net) {}

    fn run(&self, net: &mut Net);

    fn modify_chain_spec(&self) -> Box<dyn Fn(&mut ChainSpec) -> ()> {
        Box::new(|_| ())
    }

    fn modify_ckb_config(&self) -> Box<dyn Fn(&mut CKBAppConfig) -> ()> {
        // disable outbound peer service
        Box::new(|config| {
            config.network.connect_outbound_interval_secs = 0;
            config.network.discovery_local_address = true;
            config.tx_pool.min_fee_rate = FeeRate::zero();
        })
    }

    fn start_node(&self, net: &mut Net) {
        // start all nodes
        net.nodes.iter_mut().for_each(|node| {
            node.start();
        });

        // connect the nodes as a linear chain: node0 <-> node1 <-> node2 <-> ...
        if self.setup().connect_all {
            net.connect_all();
        }
    }
}

#[derive(Clone)]
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
