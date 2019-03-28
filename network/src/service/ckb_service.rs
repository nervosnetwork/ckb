use crate::peer_store::{Behaviour, Status};
use crate::protocol::ckb_handler::DefaultCKBProtocolContext;
use crate::{peers_registry::RegisterResult, CKBEvent, CKBProtocolHandler, Network, PeerId};
use futures::{sync::mpsc::Receiver, Async, Stream};
use log::{debug, error, info};
use p2p::ProtocolId;
use std::boxed::Box;
use std::sync::Arc;
use std::time::Instant;

pub struct CKBService {
    pub event_receiver: Receiver<CKBEvent>,
    pub network: Arc<Network>,
}

impl CKBService {
    fn find_handler(
        &self,
        peer_id: &PeerId,
        protocol_id: ProtocolId,
    ) -> Option<Arc<dyn CKBProtocolHandler>> {
        self.network
            .peer_protocol_version(peer_id, protocol_id)
            .and_then(|version| {
                self.network
                    .find_protocol(protocol_id, version)
                    .map(|(_, handler)| handler)
            })
    }
}

impl Stream for CKBService {
    type Item = ();
    type Error = ();
    fn poll(&mut self) -> Result<Async<Option<Self::Item>>, Self::Error> {
        use crate::CKBEvent::*;

        let network = Arc::clone(&self.network);
        match try_ready!(self.event_receiver.poll()) {
            Some(Connected(
                peer_id,
                addr,
                session_id,
                session_type,
                protocol_id,
                protocol_version,
            )) => {
                match network.accept_connection(
                    peer_id.clone(),
                    addr.clone(),
                    session_id,
                    session_type,
                    protocol_id,
                    protocol_version,
                ) {
                    Ok(register_result) => {
                        // update status in peer_store
                        if let RegisterResult::New(_) = register_result {
                            let mut peer_store = network.peer_store().write();
                            peer_store.report(&peer_id, Behaviour::Connect);
                            peer_store.update_status(&peer_id, Status::Connected);
                            let _ = peer_store.add_discovered_addr(&peer_id, addr);
                        }
                        // call handler
                        match self.find_handler(&peer_id, protocol_id) {
                            Some(handler) => handler.connected(
                                Box::new(DefaultCKBProtocolContext::new(
                                    Arc::clone(&network),
                                    protocol_id,
                                )),
                                register_result.peer_index(),
                            ),
                            None => {
                                network.drop_peer(&peer_id);
                                error!(target: "network", "can't find protocol handler for {} {:?} {}",session_id, session_type, protocol_id)
                            }
                        }
                    }
                    Err(err) => {
                        network.drop_peer(&peer_id);
                        info!(target: "network", "reject connection from {} {}, because {}", peer_id.to_base58(), addr, err)
                    }
                }
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
                    match self.find_handler(&peer_id, protocol_id) {
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
                let now = Instant::now();
                network.modify_peer(&peer_id, |peer| {
                    peer.last_message_time = Some(now);
                });
                let peer_index = network.get_peer_index(&peer_id).expect("peer_index");
                match self.find_handler(&peer_id, protocol_id) {
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
                debug!(target: "network", "ckb service shutdown");
                return Ok(Async::Ready(None));
            }
        }
        Ok(Async::Ready(Some(())))
    }
}
