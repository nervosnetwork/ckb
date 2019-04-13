// use crate::peer_store::Behaviour;
use crate::{NetworkController, NetworkState};
use log::{debug, trace, warn};
use std::collections::HashMap;
use std::sync::Arc;

use p2p::{
    multiaddr::{Multiaddr, Protocol},
    secio::PeerId,
    service::SessionType,
    utils::{is_reachable, multiaddr_to_socketaddr},
};
use p2p_identify::{Callback, MisbehaveResult, Misbehavior};

const MAX_RETURN_LISTEN_ADDRS: usize = 10;

#[derive(Clone)]
pub(crate) struct IdentifyCallback {
    network_controller: NetworkController,
    // local listen addresses for scoring and for rpc output
    remote_listen_addrs: HashMap<PeerId, Vec<Multiaddr>>,
}

impl IdentifyCallback {
    pub(crate) fn new(network_controller: NetworkController) -> IdentifyCallback {
        IdentifyCallback {
            network_controller,
            remote_listen_addrs: HashMap::default(),
        }
    }

    fn listen_addrs(&self) -> Vec<Multiaddr> {
        let mut addrs = self
            .network_controller
            .listened_addresses(MAX_RETURN_LISTEN_ADDRS * 2);
        addrs.sort_by(|a, b| a.1.cmp(&b.1));
        addrs
            .into_iter()
            .take(MAX_RETURN_LISTEN_ADDRS)
            .map(|(addr, _)| addr)
            .collect::<Vec<_>>()
    }
}

impl Callback for IdentifyCallback {
    /// Get local listen addresses
    fn local_listen_addrs(&mut self) -> Vec<Multiaddr> {
        self.listen_addrs()
    }

    fn add_remote_listen_addrs(&mut self, peer_id: &PeerId, addrs: Vec<Multiaddr>) {
        trace!(
            target: "network",
            "got remote listen addrs from peer_id={:?}, addrs={:?}",
            peer_id,
            addrs,
        );
        self.remote_listen_addrs
            .insert(peer_id.clone(), addrs.clone());
        for addr in addrs {
            self.network_controller
                .add_discovered_addr(peer_id.clone(), addr);
        }
    }

    fn add_observed_addr(
        &mut self,
        peer_id: &PeerId,
        addr: Multiaddr,
        ty: SessionType,
    ) -> MisbehaveResult {
        debug!(
            target: "network",
            "peer({:?}, {:?}) reported observed addr {}",
            peer_id,
            ty,
            addr,
        );

        if ty.is_inbound() {
            // The address already been discovered by other peer
            return MisbehaveResult::Continue;
        }

        for transformed_addr in self
            .listen_addrs()
            .into_iter()
            .filter_map(|listen_addr| multiaddr_to_socketaddr(&listen_addr))
            .filter(|socket_addr| is_reachable(socket_addr.ip()))
            .map(|socket_addr| socket_addr.port())
            .map(|listen_port| {
                addr.iter()
                    .filter_map(|proto| match proto {
                        // Replace only it's an outbound connnection
                        Protocol::P2p(_) => None,
                        Protocol::Tcp(_) => Some(Protocol::Tcp(listen_port)),
                        value => Some(value),
                    })
                    .collect::<Multiaddr>()
            })
        {
            debug!(target: "network", "identify add transformed addr: {:?}", transformed_addr);
            let local_peer_id = self.network_controller.local_peer_id();
            self.network_controller
                .add_discovered_addr(local_peer_id.to_owned(), transformed_addr);
        }
        // NOTE: for future usage
        MisbehaveResult::Continue
    }

    fn misbehave(&mut self, _peer_id: &PeerId, _kind: Misbehavior) -> MisbehaveResult {
        MisbehaveResult::Disconnect
    }
}
