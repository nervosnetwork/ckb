use crate::peer_store::{Behaviour, Status};
use crate::protocol_handler::DefaultCKBProtocolContext;
use crate::{CKBEvent, CKBProtocol, CKBProtocolHandler, Network, PeerId};
use faketime::unix_time_as_millis;
use futures::{
    future::{self, Future},
    sync::mpsc::Receiver,
    Async, Stream,
};
use log::{debug, error, info, trace, warn};
use p2p::{context::ServiceControl, multiaddr::Multiaddr, ProtocolId, SessionType};
use std::boxed::Box;
use std::io::{Error as IoError, ErrorKind as IoErrorKind};
use std::sync::Arc;
use tokio;

pub struct CKBService {
    pub event_receiver: Receiver<CKBEvent>,
    pub network: Arc<Network>,
}

impl CKBService {
    fn find_handler(&self, protocol_id: ProtocolId) -> Option<Arc<dyn CKBProtocolHandler>> {
        self.network
            .find_protocol(protocol_id)
            .map(|(_, handler)| handler)
    }
}

impl Stream for CKBService {
    type Item = ();
    type Error = ();
    fn poll(&mut self) -> Result<Async<Option<Self::Item>>, Self::Error> {
        use crate::CKBEvent::*;

        let network = Arc::clone(&self.network);
        match try_ready!(self.event_receiver.poll()) {
            Some(Connected(peer_id, addr, protocol_id, session_type, version)) => {
                let connect_result = match session_type {
                    SessionType::Client => {
                        network.new_outbound_connection(peer_id.clone(), addr.clone())
                    }
                    SessionType::Server => {
                        network.new_inbound_connection(peer_id.clone(), addr.clone())
                    }
                };

                match connect_result {
                    Ok(peer_index) => {
                        // update status in peer_store
                        {
                            let mut peer_store = network.peer_store().write();
                            peer_store.report(&peer_id, Behaviour::Connect);
                            peer_store.update_status(&peer_id, Status::Connected);
                            peer_store.add_discovered_address(&peer_id, addr);
                        }
                        // call handler
                        match self.find_handler(protocol_id) {
                            Some(handler) => handler.connected(
                                Box::new(DefaultCKBProtocolContext::new(
                                    Arc::clone(&network),
                                    protocol_id,
                                )),
                                peer_index,
                            ),
                            None => {
                                error!(target: "network", "can't find protocol handler for {}", protocol_id)
                            }
                        }
                    }
                    Err(err) => {
                        info!(target: "network", "reject connection from {} {}, because {}", peer_id.to_base58(), addr, err)
                    }
                }
            }
            Some(ConnectedError(addr)) => {
                error!(target: "network", "ckb protocol connected error, addr: {}", addr);
            }
            Some(Disconnected(peer_id, protocol_id)) => {
                // update disconnect in peer_store
                {
                    let mut peer_store = network.peer_store().write();
                    peer_store.report(&peer_id, Behaviour::UnexpectedDisconnect);
                    peer_store.update_status(&peer_id, Status::Disconnected);
                }
                if let Some(peer_index) = network.get_peer_index(&peer_id) {
                    // call handler
                    match self.find_handler(protocol_id) {
                        Some(handler) => handler.disconnected(
                            Box::new(DefaultCKBProtocolContext::new(
                                Arc::clone(&network),
                                protocol_id,
                            )),
                            peer_index,
                        ),
                        None => {
                            error!(target: "network", "can't find protocol handler for {}", protocol_id)
                        }
                    }
                }
                // disconnect
                network.drop_peer(&peer_id);
            }
            Some(Received(peer_id, protocol_id, data)) => {
                network.modify_peer(&peer_id, |peer| {
                    peer.last_message_time = Some(unix_time_as_millis())
                });
                let peer_index = network.get_peer_index(&peer_id).expect("peer_index");
                match self.find_handler(protocol_id) {
                    Some(handler) => handler.received(
                        Box::new(DefaultCKBProtocolContext::new(network, protocol_id)),
                        peer_index,
                        data,
                    ),
                    None => {
                        error!(target: "network", "can't find protocol handler for {}", protocol_id)
                    }
                }
            }
            Some(Notify(protocol_id, token)) => {
                debug!(target: "network", "receive ckb timer notify, protocol_id: {} token: {}", protocol_id, token);
            }
            None => {
                error!(target: "network", "ckb service should not stop");
            }
        }
        Ok(Async::Ready(Some(())))
    }
}
