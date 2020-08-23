use crate::utils::message::build_compact_block;
use crate::utils::wait_until;
use crate::{Net, Spec, TestProtocol};
use ckb_network::SupportProtocols;

pub struct LastCommonHeaderForPeerWithWorseChain;

impl Spec for LastCommonHeaderForPeerWithWorseChain {
    crate::name!("last_common_header_for_peer_with_worse_chain");

    crate::setup!(num_nodes: 1, connect_all: false, protocols: vec![TestProtocol::sync(), TestProtocol::relay()]);

    // As for the peers of which main chain is worse than ours, we should ensure the
    // last_common_header updating as well.
    fn run(&self, net: &mut Net) {
        let node0 = &net.nodes[0];

        // Node0's main chain tip is 5
        node0.generate_blocks(5);
        let worse = (1..=4)
            .map(|number| node0.get_block_by_number(number))
            .collect::<Vec<_>>();

        // Net relay blocks[1..4] to node0, let node0 knows our best chain is at 4.
        net.connect(node0);
        let (peer_id, _, _) = net.receive();
        for block in worse {
            net.send(
                SupportProtocols::Relay.protocol_id(),
                peer_id,
                build_compact_block(&block),
            );
        }

        // peer.last_common_header is expect to be advanced to peer.best_known_header
        let last_common_header_synced = wait_until(10, || {
            let sync_state = node0
                .rpc_client()
                .get_peers()
                .into_iter()
                .filter(|remote_node| remote_node.node_id == net.node_id())
                .last()
                .and_then(|node| node.sync_state);
            if sync_state
                .as_ref()
                .map(|sync_state| sync_state.last_common_header_number == Some(4.into()))
                .unwrap_or(false)
            {
                return true;
            }
            false
        });
        assert!(last_common_header_synced);
    }
}
