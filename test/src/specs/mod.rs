mod block_relay;
mod block_sync;
mod mining;
mod net;
mod pool;
mod transaction_relay;
mod sync_protocol;

pub use block_relay::BlockRelayBasic;
pub use block_sync::BlockSyncBasic;
pub use mining::MiningBasic;
pub use pool::{PoolReconcile, PoolTrace};
pub use transaction_relay::TransactionRelayBasic;
pub use net::{Disconnect, Discovery};
pub use sync_protocol::MalformedMessage;

use crate::{sleep, Net};

pub trait Spec {
    fn run(&self, net: Net);

    fn num_nodes(&self) -> usize {
        3
    }

    fn connect_all(&self) -> bool {
        true
    }

    fn setup_net(&self, binary: &str, start_port: u16) -> Net {
        let mut net = Net::new(binary, self.num_nodes(), start_port);

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
