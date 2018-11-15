#![cfg_attr(feature = "cargo-clippy", allow(needless_pass_by_value))]

use super::CKBProtocolHandler;
use super::Network;
use super::PeerId;
use ckb_protocol::CKBProtocolOutput;
use ckb_protocol_handler::DefaultCKBProtocolContext;
use futures::future::{self, Future};
use futures::Stream;
use libp2p::core::{Multiaddr, UniqueConnecState};
use libp2p::kad;
use peer_store::{Behaviour, Status};
use protocol::Protocol;
use protocol_service::ProtocolService;
use std::boxed::Box;
use std::io::{Error as IoError, ErrorKind as IoErrorKind};
use std::sync::Arc;
use tokio;

pub struct CKBService {
    // used to update kbuckets
    pub kad_system: Arc<kad::KadSystem>,
}

impl CKBService {
    fn handle_protocol_connection(
        network: Arc<Network>,
        peer_id: PeerId,
        protocol_output: CKBProtocolOutput<Arc<CKBProtocolHandler>>,
        kad_system: Arc<kad::KadSystem>,
        addr: Option<Multiaddr>,
    ) -> Box<Future<Item = (), Error = IoError> + Send> {
        let protocol_id = protocol_output.protocol_id;
        let protocol_handler = protocol_output.protocol_handler;
        let protocol_version = protocol_output.protocol_version;
        let endpoint = protocol_output.endpoint;
        let addresses = addr.map(|addr| vec![addr]);
        // get peer protocol_connection
        let protocol_connec =
            match network.ckb_protocol_connec(&peer_id, protocol_id, endpoint, addresses.clone()) {
                Ok(protocol_connec) => protocol_connec,
                Err(err) => {
                    return Box::new(future::err(IoError::new(IoErrorKind::Other, err)))
                        as Box<Future<Item = (), Error = IoError> + Send>
                }
            };
        if protocol_connec.state() == UniqueConnecState::Full {
            error!(
                target: "network",
                "we already connected peer {:?} with {:?}, stop handling",
                peer_id, protocol_id
            );
            return Box::new(future::ok(())) as Box<_>;
        }

        let peer_index = {
            let peers_registry = network.peers_registry().read();
            match peers_registry.get(&peer_id) {
                Some(peer) => peer.peer_index.unwrap(),
                None => {
                    return Box::new(future::err(IoError::new(
                        IoErrorKind::Other,
                        format!("can't find peer {:?}", peer_id),
                    )))
                }
            }
        };

        let protocol_future = {
            let handling_future = protocol_output.incoming_stream.for_each({
                let network = Arc::clone(&network);
                let protocol_handler = Arc::clone(&protocol_handler);
                let peer_id = peer_id.clone();
                let kad_system = Arc::clone(&kad_system);
                move |data| {
                    // update kad_system when we received data
                    kad_system.update_kbuckets(peer_id.clone());
                    let protocol_handler = Arc::clone(&protocol_handler);
                    let network = Arc::clone(&network);
                    let handle_received = future::lazy(move || {
                        protocol_handler.received(
                            Box::new(DefaultCKBProtocolContext::new(network, protocol_id)),
                            peer_index,
                            &data,
                        );
                        Ok(())
                    });
                    tokio::spawn(handle_received);
                    Ok(())
                }
            });
            protocol_connec
                .tie_or_stop(
                    (protocol_output.outgoing_msg_channel, protocol_version),
                    handling_future,
                ).then({
                    let network = Arc::clone(&network);
                    let peer_id = peer_id.clone();
                    let protocol_handler = Arc::clone(&protocol_handler);
                    let protocol_id = protocol_id;
                    move |val| {
                        info!(
                            target: "network",
                            "Disconnect! peer {:?} protocol_id {:?} reason {:?}",
                            peer_id, protocol_id, val
                        );
                        {
                            let mut peer_store = network.peer_store().write();
                            peer_store.report(&peer_id, Behaviour::UnexpectedDisconnect);
                            peer_store.report_status(&peer_id, Status::Disconnected);
                        }
                        protocol_handler.disconnected(
                            Box::new(DefaultCKBProtocolContext::new(
                                Arc::clone(&network),
                                protocol_id,
                            )),
                            peer_index,
                        );
                        let mut peers_registry = network.peers_registry().write();
                        peers_registry.drop_peer(&peer_id);
                        val
                    }
                })
        };

        info!(
            target: "network",
            "Connected to peer {:?} with protocol_id {:?} version {}",
            peer_id, protocol_id, protocol_version
        );
        {
            let mut peer_store = network.peer_store().write();
            peer_store.report(&peer_id, Behaviour::Connect);
            peer_store.report_status(&peer_id, Status::Connected);
        }
        protocol_handler.connected(
            Box::new(DefaultCKBProtocolContext::new(
                Arc::clone(&network),
                protocol_id,
            )),
            peer_index,
        );
        Box::new(protocol_future) as Box<_>
    }
}

impl<T: Send> ProtocolService<T> for CKBService {
    type Output = CKBProtocolOutput<Arc<CKBProtocolHandler>>;
    fn convert_to_protocol(
        peer_id: Arc<PeerId>,
        addr: &Multiaddr,
        output: Self::Output,
    ) -> Protocol<T> {
        Protocol::CKBProtocol(output, PeerId::clone(&peer_id), Some(addr.to_owned()))
    }
    fn handle(
        &self,
        network: Arc<Network>,
        protocol: Protocol<T>,
    ) -> Box<Future<Item = (), Error = IoError> + Send> {
        match protocol {
            Protocol::CKBProtocol(output, peer_id, addr) => {
                let handling_future = Self::handle_protocol_connection(
                    network,
                    peer_id,
                    output,
                    Arc::clone(&self.kad_system),
                    addr,
                );
                Box::new(handling_future) as Box<Future<Item = _, Error = _> + Send>
            }
            _ => Box::new(future::ok(())) as Box<Future<Item = _, Error = _> + Send>,
        }
    }
}
