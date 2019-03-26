use crate::errors::{Error, PeerError, ProtocolError};
use crate::{Behaviour, Network, PeerIndex, ProtocolId, SessionInfo, TimerRegistry, TimerToken};
use bytes::Bytes;
use ckb_util::Mutex;
use log::debug;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Debug)]
pub enum Severity<'a> {
    Timeout,
    Useless(&'a str),
    Bad(&'a str),
}

pub trait CKBProtocolContext: Send {
    fn send(&self, peer_index: PeerIndex, data: Vec<u8>) -> Result<(), Error>;
    fn send_protocol(
        &self,
        peer_index: PeerIndex,
        protocol_id: ProtocolId,
        data: Vec<u8>,
    ) -> Result<(), Error>;
    // TODO combinate this interface with peer score
    fn report_peer(&self, peer_index: PeerIndex, behaviour: Behaviour) -> Result<(), Error>;
    fn ban_peer(&self, peer_index: PeerIndex, timeout: Duration);
    fn disconnect(&self, peer_index: PeerIndex);
    fn register_timer(&self, token: TimerToken, delay: Duration) -> Result<(), Error>;
    fn session_info(&self, peer_index: PeerIndex) -> Option<SessionInfo>;
    fn protocol_version(&self, peer_index: PeerIndex, protocol_id: ProtocolId) -> Option<u8>;
    fn protocol_id(&self) -> ProtocolId;
    fn sessions(&self, peer_indexes: &[PeerIndex]) -> Vec<(PeerIndex, SessionInfo)> {
        peer_indexes
            .iter()
            .filter_map(|peer_index| {
                self.session_info(*peer_index)
                    .and_then(|session| Some((*peer_index, session)))
            })
            .collect()
    }
    fn connected_peers(&self) -> Vec<PeerIndex>;
}

pub(crate) struct DefaultCKBProtocolContext {
    pub protocol_id: ProtocolId,
    pub network: Arc<Network>,
    pub timer_registry: TimerRegistry,
}

impl DefaultCKBProtocolContext {
    pub fn new(network: Arc<Network>, protocol_id: ProtocolId) -> Self {
        Self::with_timer_registry(network, protocol_id, Arc::new(Mutex::new(None)))
    }

    pub fn with_timer_registry(
        network: Arc<Network>,
        protocol_id: ProtocolId,
        timer_registry: TimerRegistry,
    ) -> Self {
        DefaultCKBProtocolContext {
            network,
            protocol_id,
            timer_registry,
        }
    }
}

impl CKBProtocolContext for DefaultCKBProtocolContext {
    fn send(&self, peer_index: PeerIndex, data: Vec<u8>) -> Result<(), Error> {
        self.send_protocol(peer_index, self.protocol_id, data)
    }
    fn send_protocol(
        &self,
        peer_index: PeerIndex,
        protocol_id: ProtocolId,
        data: Vec<u8>,
    ) -> Result<(), Error> {
        if let Some(peer_id) = self.network.get_peer_id(peer_index) {
            self.network.send(&peer_id, protocol_id, data.into())
        } else {
            Err(Error::Peer(PeerError::IndexNotFound(peer_index)))
        }
    }
    // report peer behaviour
    fn report_peer(&self, peer_index: PeerIndex, behaviour: Behaviour) -> Result<(), Error> {
        debug!(target: "network", "report peer {} behaviour: {:?}", peer_index, behaviour);
        if let Some(peer_id) = self.network.get_peer_id(peer_index) {
            if self
                .network
                .peer_store()
                .write()
                .report(&peer_id, behaviour)
                .is_banned()
            {
                self.disconnect(peer_index);
            }
            Ok(())
        } else {
            Err(Error::Peer(PeerError::IndexNotFound(peer_index)))
        }
    }

    // ban peer
    fn ban_peer(&self, peer_index: PeerIndex, timeout: Duration) {
        if let Some(peer_id) = self.network.get_peer_id(peer_index) {
            self.network.ban_peer(&peer_id, timeout)
        }
    }
    // disconnect from peer
    fn disconnect(&self, peer_index: PeerIndex) {
        debug!(target: "network", "disconnect peer {}", peer_index);
        if let Some(peer_id) = self.network.get_peer_id(peer_index) {
            self.network.drop_peer(&peer_id);
        }
    }
    fn register_timer(&self, token: TimerToken, duration: Duration) -> Result<(), Error> {
        let (_, handler) = self
            .network
            .find_protocol_without_version(self.protocol_id)
            .ok_or_else(|| ProtocolError::NotFound(self.protocol_id))?;

        match *self.timer_registry.lock() {
            Some(ref mut timer_registry) => {
                timer_registry.push((handler, self.protocol_id, token, duration))
            }
            None => return Err(ProtocolError::DisallowRegisterTimer.into()),
        }
        Ok(())
    }
    fn session_info(&self, peer_index: PeerIndex) -> Option<SessionInfo> {
        if let Some(session) = self
            .network
            .get_peer_id(peer_index)
            .map(|peer_id| self.network.session_info(&peer_id, self.protocol_id))
        {
            session
        } else {
            None
        }
    }

    fn protocol_version(&self, peer_index: PeerIndex, protocol_id: ProtocolId) -> Option<u8> {
        if let Some(protocol_version) = self
            .network
            .get_peer_id(peer_index)
            .map(|peer_id| self.network.peer_protocol_version(&peer_id, protocol_id))
        {
            protocol_version
        } else {
            None
        }
    }

    fn protocol_id(&self) -> ProtocolId {
        self.protocol_id
    }

    fn connected_peers(&self) -> Vec<PeerIndex> {
        self.network.peers_indexes()
    }
}

pub trait CKBProtocolHandler: Sync + Send {
    fn initialize(&self, _nc: Box<dyn CKBProtocolContext>);
    fn received(&self, _nc: Box<dyn CKBProtocolContext>, _peer: PeerIndex, _data: Bytes);
    fn connected(&self, _nc: Box<dyn CKBProtocolContext>, _peer: PeerIndex);
    fn disconnected(&self, _nc: Box<dyn CKBProtocolContext>, _peer: PeerIndex);
    fn timer_triggered(&self, _nc: Box<dyn CKBProtocolContext>, _timer: TimerToken) {}
}
