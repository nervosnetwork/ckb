// use crate::peer_store::Behaviour;
use crate::Network;
use futures::{sync::mpsc, sync::oneshot, Async, Future, Stream};
use log::{debug, warn};
use std::sync::Arc;

use p2p::{multiaddr::Multiaddr, secio::PeerId};

pub use p2p_identify::IdentifyProtocol;
use p2p_identify::{AddrManager, MisbehaveResult, Misbehavior};

#[derive(Clone)]
pub(crate) struct IdentifyAddressManager {
    event_sender: mpsc::UnboundedSender<IdentifyEvent>,
}

impl IdentifyAddressManager {
    pub(crate) fn new(
        event_sender: mpsc::UnboundedSender<IdentifyEvent>,
    ) -> IdentifyAddressManager {
        IdentifyAddressManager { event_sender }
    }
}

impl AddrManager for IdentifyAddressManager {
    fn add_listen_addrs(&mut self, peer_id: &PeerId, addrs: Vec<Multiaddr>) {
        let event = IdentifyEvent::AddListenAddrs {
            peer_id: peer_id.clone(),
            addrs,
        };
        if self.event_sender.unbounded_send(event).is_err() {
            warn!(target: "network", "receiver maybe dropped!");
        }
    }

    fn add_observed_addr(&mut self, peer_id: &PeerId, addr: Multiaddr) -> MisbehaveResult {
        let event = IdentifyEvent::AddObservedAddr {
            peer_id: peer_id.clone(),
            addr,
        };
        if self.event_sender.unbounded_send(event).is_err() {
            warn!(target: "network", "receiver maybe dropped!");
        }
        // NOTE: for future usage
        MisbehaveResult::Continue
    }

    fn misbehave(&mut self, peer_id: &PeerId, kind: Misbehavior) -> MisbehaveResult {
        let (sender, receiver) = oneshot::channel();
        let event = IdentifyEvent::Misbehave {
            peer_id: peer_id.clone(),
            kind,
            result: sender,
        };
        if self.event_sender.unbounded_send(event).is_err() {
            warn!(target: "network", "receiver maybe dropped!");
            MisbehaveResult::Disconnect
        } else {
            receiver.wait().unwrap_or(MisbehaveResult::Disconnect)
        }
    }
}

pub enum IdentifyEvent {
    AddListenAddrs {
        peer_id: PeerId,
        addrs: Vec<Multiaddr>,
    },
    AddObservedAddr {
        peer_id: PeerId,
        addr: Multiaddr,
    },
    Misbehave {
        peer_id: PeerId,
        kind: Misbehavior,
        result: oneshot::Sender<MisbehaveResult>,
    },
}

pub(crate) struct IdentifyService {
    event_receiver: mpsc::UnboundedReceiver<IdentifyEvent>,
    network: Arc<Network>,
}

impl IdentifyService {
    pub(crate) fn new(
        network: Arc<Network>,
        event_receiver: mpsc::UnboundedReceiver<IdentifyEvent>,
    ) -> IdentifyService {
        IdentifyService {
            event_receiver,
            network,
        }
    }
}

impl Stream for IdentifyService {
    type Item = ();
    type Error = ();
    fn poll(&mut self) -> Result<Async<Option<Self::Item>>, Self::Error> {
        match try_ready!(self.event_receiver.poll()) {
            Some(IdentifyEvent::AddListenAddrs { .. }) => {
                // TODO: how to transform those addresses
            }
            Some(IdentifyEvent::AddObservedAddr { .. }) => {
                // TODO: how to transform this address
            }
            Some(IdentifyEvent::Misbehave { result, .. }) => {
                // TODO: report misbehave
                if result.send(MisbehaveResult::Continue).is_err() {
                    return Err(());
                }
            }
            None => {
                debug!(target: "network", "identify service shutdown");
                return Ok(Async::Ready(None));
            }
        }
        Ok(Async::Ready(Some(())))
    }
}
