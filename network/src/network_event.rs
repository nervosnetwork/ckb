use crate::errors::{Error, ProtocolError};
use crate::peer_store::{sqlite::SqlitePeerStore, PeerStore, Status};
use crate::peers_registry::{ConnectionStatus, PeersRegistry};
use crate::protocols::{
    discovery::{DiscoveryEvent, DiscoveryProtocol},
    identify::IdentifyCallback,
};
use crate::protocols::{feeler::Feeler, BackgroundService, DefaultCKBProtocolContext};
use crate::MultiaddrList;
use crate::Peer;
use crate::{
    Behaviour, CKBProtocol, CKBProtocolContext, NetworkConfig, NetworkState, ProtocolId,
    ProtocolVersion, ServiceContext, ServiceControl, SessionId, SessionType,
};
use crate::{DISCOVERY_PROTOCOL_ID, FEELER_PROTOCOL_ID, IDENTIFY_PROTOCOL_ID, PING_PROTOCOL_ID};
use ckb_core::service::{Request, DEFAULT_CHANNEL_SIZE, SIGNAL_CHANNEL_SIZE};
use ckb_util::RwLock;
use crossbeam_channel::{self, select, Receiver, Sender, TryRecvError};
use fnv::{FnvHashMap, FnvHashSet};
use futures::sync::{
    mpsc::{self, channel, UnboundedSender},
    oneshot,
};
use futures::Future;
use futures::Stream;
use futures::{try_ready, Async, Poll};
use log::{debug, error, info, trace, warn};
use lru_cache::LruCache;
use p2p::{
    builder::{MetaBuilder, ServiceBuilder},
    error::Error as P2pError,
    multiaddr::{self, multihash::Multihash, Multiaddr},
    secio::PeerId,
    service::{DialProtocol, ProtocolEvent, ProtocolHandle, Service, ServiceError, ServiceEvent},
    traits::ServiceHandle,
    utils::extract_peer_id,
};
use p2p_identify::IdentifyProtocol;
use p2p_ping::{Event as PingEvent, PingHandler};
use secio;
use std::boxed::Box;
use std::cell::RefCell;
use std::cmp::max;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use std::usize;
use stop_handler::{SignalSender, StopHandler};
use tokio::runtime::Runtime;
use tokio::timer::Interval;

pub enum NetworkEvent {
    Protocol(ProtocolEvent),
    Event(ServiceEvent),
    Error(ServiceError),
}
pub struct EventHandler {
    sender: UnboundedSender<NetworkEvent>,
}

impl EventHandler {
    pub fn new(sender: UnboundedSender<NetworkEvent>) -> Self {
        EventHandler { sender }
    }
}

impl ServiceHandle for EventHandler {
    fn handle_error(&mut self, _context: &mut ServiceContext, error: ServiceError) {
        warn!(target: "network", "p2p service error: {:?}", error);
        match self.sender.unbounded_send(NetworkEvent::Error(error)) {
            Ok(_) => {
                trace!(target: "network", "send network error success");
            }
            Err(err) => error!(target: "network", "send network error failed: {:?}", err),
        }
    }

    fn handle_event(&mut self, context: &mut ServiceContext, event: ServiceEvent) {
        info!(target: "network", "p2p service event: {:?}", event);
        match self.sender.unbounded_send(NetworkEvent::Event(event)) {
            Ok(_) => {
                trace!(target: "network", "send network service event success");
            }
            Err(err) => error!(target: "network", "send network event failed: {:?}", err),
        }
    }

    fn handle_proto(&mut self, context: &mut ServiceContext, event: ProtocolEvent) {
        match self.sender.unbounded_send(NetworkEvent::Protocol(event)) {
            Ok(_) => {
                trace!(target: "network", "send network protocol event success");
            }
            Err(err) => error!(target: "network", "send network event failed: {:?}", err),
        }
    }
}
