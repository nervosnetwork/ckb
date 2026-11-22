use crate::util::mining::out_ibd_mode;
use crate::utils::{sleep, wait_until};
use crate::{Node, Spec};
use ckb_app_config::SupportProtocol;
use ckb_logger::info;
use ckb_network::SupportProtocols;

fn protocol_ids(node: &Node, peer_node: &Node) -> Vec<u64> {
    node.rpc_client()
        .get_peers()
        .into_iter()
        .find(|peer| peer.node_id == peer_node.node_id())
        .map(|peer| peer.protocols.into_iter().map(|p| p.id.value()).collect())
        .unwrap_or_default()
}

/// A dialer must be able to finish negotiating its protocols against a receiver
/// whose optional protocol list is shorter than its own (#5341).
///
/// The receiver drops `Time`, `LightClient`, `Filter` and `HolePunching`, so it
/// registers fewer yamux streams worth of demand than the dialer asks for. Before
/// the fix the stream limit followed the receiver's own protocol count, the extra
/// inbound SYN frames were rejected, and a protocol both nodes support (`Relay`)
/// could stay closed.
pub struct SupportProtocolNegotiationWithShorterReceiver;

impl Spec for SupportProtocolNegotiationWithShorterReceiver {
    crate::setup!(num_nodes: 2);

    fn run(&self, nodes: &mut Vec<Node>) {
        // An outbound peer is dropped while the dialer is in initial block
        // download, so leave IBD before connecting. Both nodes stay in the
        // vector at this point so mining can synchronise them.
        out_ibd_mode(nodes);

        let mut receiver = nodes.pop().unwrap();
        let dialer = nodes.pop().unwrap();

        receiver.stop();
        receiver.modify_app_config(|config| {
            config.network.support_protocols = vec![
                SupportProtocol::Ping,
                SupportProtocol::Discovery,
                SupportProtocol::Identify,
                SupportProtocol::Feeler,
                SupportProtocol::DisconnectMessage,
                SupportProtocol::Sync,
                SupportProtocol::Relay,
            ];
        });
        receiver.start();
        // The receiver has to re-bind its P2P port after the restart.
        sleep(5);

        info!("Dial the receiver that registers a shorter protocol list");
        dialer.connect(&receiver);

        let common_ids = [
            SupportProtocols::Ping.protocol_id().value() as u64,
            SupportProtocols::Discovery.protocol_id().value() as u64,
            SupportProtocols::Identify.protocol_id().value() as u64,
            SupportProtocols::Sync.protocol_id().value() as u64,
            SupportProtocols::RelayV3.protocol_id().value() as u64,
        ];

        // Give the remaining protocols time to finish negotiating, then require
        // both sides to report every protocol the two nodes have in common.
        sleep(5);
        let ret = wait_until(30, || {
            let dialer_ids = protocol_ids(&dialer, &receiver);
            let receiver_ids = protocol_ids(&receiver, &dialer);
            common_ids.iter().all(|id| dialer_ids.contains(id))
                && common_ids.iter().all(|id| receiver_ids.contains(id))
        });
        assert!(
            ret,
            "All protocols supported by both nodes should be negotiated"
        );

        // The connection must also stay up: nothing was rejected so hard that
        // the session was torn down.
        let ret = wait_until(10, || {
            dialer.rpc_client().get_peers().len() == 1
                && receiver.rpc_client().get_peers().len() == 1
        });
        assert!(ret, "The connection should stay established");
    }
}

/// The ordinary configuration: both nodes run the full default protocol list.
pub struct SupportProtocolNegotiationWithDefaultLists;

impl Spec for SupportProtocolNegotiationWithDefaultLists {
    crate::setup!(num_nodes: 2);

    fn run(&self, nodes: &mut Vec<Node>) {
        out_ibd_mode(nodes);

        let receiver = nodes.pop().unwrap();
        let dialer = nodes.pop().unwrap();

        dialer.connect(&receiver);

        let base_ids = [
            SupportProtocols::Identify.protocol_id().value() as u64,
            SupportProtocols::Sync.protocol_id().value() as u64,
        ];
        let ret = wait_until(30, || {
            let dialer_ids = protocol_ids(&dialer, &receiver);
            let receiver_ids = protocol_ids(&receiver, &dialer);
            base_ids.iter().all(|id| dialer_ids.contains(id))
                && base_ids.iter().all(|id| receiver_ids.contains(id))
        });
        assert!(ret, "Identify and Sync should be negotiated on both sides");
    }
}
