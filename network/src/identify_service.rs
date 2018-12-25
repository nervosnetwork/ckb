#![allow(clippy::needless_pass_by_value)]

use super::Network;
use super::PeerId;
use crate::peers_registry::PeerIdentifyInfo;
use crate::protocol::Protocol;
use crate::protocol_service::ProtocolService;
use crate::transport::TransportOutput;
use futures::future::{self, Future};
use futures::Stream;
use libp2p::core::Multiaddr;
use libp2p::core::SwarmController;
use libp2p::core::{upgrade, MuxedTransport};
use libp2p::identify::IdentifyProtocolConfig;
use libp2p::identify::{IdentifyInfo, IdentifyOutput};
use libp2p::{self, Transport};
use log::{debug, error, trace, warn};
use std::boxed::Box;
use std::io::{Error as IoError, ErrorKind as IoErrorKind};
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::timer::Interval;

pub struct IdentifyService {
    pub client_version: String,
    pub protocol_version: String,
    pub identify_timeout: Duration,
    pub identify_interval: Duration,
}

impl IdentifyService {
    fn process_identify_info(
        &self,
        network: Arc<Network>,
        peer_id: &PeerId,
        info: &IdentifyInfo,
        observed_addr: &Multiaddr,
    ) -> Result<(), IoError> {
        trace!("process identify for peer_id {:?} with {:?}", peer_id, info);
        // set identify info to peer
        {
            let identify_info = PeerIdentifyInfo {
                client_version: info.agent_version.clone(),
                protocol_version: info.protocol_version.clone(),
                supported_protocols: info.protocols.clone(),
                count_of_known_listen_addrs: info.listen_addrs.len(),
            };
            if network
                .set_peer_identify_info(&peer_id, identify_info)
                .is_err()
            {
                error!(
                    target: "network",
                    "can't find peer_id {:?} during process identify info",
                    peer_id
                )
            }
        }

        // add obserevd listened addr
        for original_address in network.original_listened_addresses.read().iter() {
            let transport = libp2p::tcp::TcpConfig::new();
            trace!(
                target: "network",
                "try get address use original_address {:?} and observed_address {:?}",
                original_address,
                observed_addr
            );
            // get an external addrs for our node
            if let Some(ext_addr) = transport.nat_traversal(original_address, &observed_addr) {
                debug!(target: "network", "get new external address {:?}", ext_addr);
                let mut listened_addresses = network.listened_addresses.write();
                if !listened_addresses.iter().any(|a| a == &ext_addr) {
                    listened_addresses.push(ext_addr.clone());
                }
            }
        }

        // update peer addrs in peerstore
        let _ = network
            .peer_store()
            .lock()
            .add_discovered_addresses(peer_id, info.listen_addrs.clone());
        Ok(())
    }
}

impl<T> ProtocolService<T> for IdentifyService
where
    T: AsyncRead + AsyncWrite + Send + 'static,
{
    type Output = IdentifyOutput<T>;
    fn convert_to_protocol(
        peer_id: Arc<PeerId>,
        addr: &Multiaddr,
        output: Self::Output,
    ) -> Protocol<T> {
        let peer_id = PeerId::clone(&peer_id);
        match output {
            IdentifyOutput::RemoteInfo {
                info,
                observed_addr,
            } => Protocol::IdentifyRequest(peer_id, info, observed_addr),
            IdentifyOutput::Sender { sender } => {
                Protocol::IdentifyResponse(peer_id, sender, addr.to_owned())
            }
        }
    }

    fn handle(
        &self,
        network: Arc<Network>,
        protocol: Protocol<T>,
    ) -> Box<Future<Item = (), Error = IoError> + Send> {
        match protocol {
            Protocol::IdentifyRequest(peer_id, info, ovserved_addr) => match self
                .process_identify_info(Arc::clone(&network), &peer_id, &info, &ovserved_addr)
            {
                Ok(_) => Box::new(future::ok(())),
                Err(err) => Box::new(future::err(err)),
            },
            Protocol::IdentifyResponse(_peer_id, sender, addr) => {
                sender.send(
                    IdentifyInfo {
                        public_key: network.local_public_key().clone(),
                        protocol_version: format!("ckb/{}", self.protocol_version).to_owned(),
                        agent_version: format!("ckb/{}", self.client_version).to_owned(),
                        listen_addrs: network.listened_addresses.read().clone(),
                        protocols: vec![], // TODO FIXME: report local protocols
                    },
                    &addr,
                )
            }
            _ => Box::new(future::ok(())) as Box<Future<Item = _, Error = _> + Send>,
        }
    }

    fn start_protocol<SwarmTran, Tran, TranOut>(
        &self,
        network: Arc<Network>,
        swarm_controller: SwarmController<
            SwarmTran,
            Box<Future<Item = (), Error = IoError> + Send>,
        >,
        transport: Tran,
    ) -> Box<Future<Item = (), Error = IoError> + Send>
    where
        SwarmTran: MuxedTransport<Output = Protocol<T>> + Clone + Send + 'static,
        SwarmTran::MultiaddrFuture: Send + 'static,
        SwarmTran::Dial: Send,
        SwarmTran::Listener: Send,
        SwarmTran::ListenerUpgrade: Send,
        SwarmTran::Incoming: Send,
        SwarmTran::IncomingUpgrade: Send,
        Tran: MuxedTransport<Output = TransportOutput<TranOut>> + Clone + Send + 'static,
        Tran::MultiaddrFuture: Send + 'static,
        Tran::Dial: Send,
        Tran::Listener: Send,
        Tran::ListenerUpgrade: Send,
        Tran::Incoming: Send,
        Tran::IncomingUpgrade: Send,
        TranOut: AsyncRead + AsyncWrite + Send + 'static,
    {
        let transport = transport.and_then(move |out, endpoint, client_addr| {
            let peer_id = out.peer_id;
            upgrade::apply(out.socket, IdentifyProtocolConfig, endpoint, client_addr).map(
                move |(output, addr)| {
                    let protocol = match output {
                        IdentifyOutput::RemoteInfo {
                            info,
                            observed_addr,
                        } => Protocol::IdentifyRequest(peer_id, info, observed_addr),
                        IdentifyOutput::Sender { .. } => {
                            panic!("should not reach here because we are dialer")
                        }
                    };
                    (protocol, addr)
                },
            )
        });

        let periodic_identify_future = Interval::new(
            Instant::now() + Duration::from_secs(5),
            self.identify_interval,
        )
        .map_err(|err| {
            debug!(target: "network", "identify periodic error {:?}", err);
            IoError::new(
                IoErrorKind::Other,
                format!("identify periodic error {:?}", err),
            )
        })
        .for_each({
            let transport = transport.clone();
            let _identify_timeout = self.identify_timeout;
            let network = Arc::clone(&network);
            move |_| {
                for peer_id in network.peers() {
                    if let Some(ref identify_info) = network.get_peer_identify_info(&peer_id) {
                        if identify_info.count_of_known_listen_addrs > 0 {
                            continue;
                        }
                    }
                    // TODO should we try all addresses?
                    if let Some(addr) = network.get_peer_addresses(&peer_id).get(0) {
                        trace!(
                        target: "network",
                        "request identify to peer {:?} {:?}",
                        peer_id,
                        addr
                        );
                        // dial identify
                        let _ = swarm_controller.dial(addr.clone(), transport.clone());
                    } else {
                        error!(
                        target: "network",
                        "error when prepare identify : can't find addresses for peer {:?}",
                        peer_id
                        );
                    }
                }
                Ok(())
            }
        })
        .then(|err| {
            warn!(target: "network", "Identify service stopped, reason: {:?}", err);
            err
        });
        Box::new(periodic_identify_future) as Box<Future<Item = _, Error = _> + Send>
    }
}
