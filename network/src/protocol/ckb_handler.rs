use crate::errors::{Error, PeerError};
use crate::{Behaviour, NetworkState, PeerIndex, ProtocolId, ServiceControl, SessionInfo};
use bytes::Bytes;
use log::{debug, error};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Debug)]
pub enum Severity<'a> {
    Timeout,
    Useless(&'a str),
    Bad(&'a str),
}

pub trait CKBProtocolContext: Send {
    fn send(&mut self, peer_index: PeerIndex, data: Vec<u8>) -> Result<(), Error>;
    fn send_protocol(
        &mut self,
        peer_index: PeerIndex,
        protocol_id: ProtocolId,
        data: Vec<u8>,
    ) -> Result<(), Error>;
    // TODO combinate this interface with peer score
    fn report_peer(&self, peer_index: PeerIndex, behaviour: Behaviour) -> Result<(), Error>;
    fn ban_peer(&self, peer_index: PeerIndex, timeout: Duration);
    fn disconnect(&self, peer_index: PeerIndex);
    fn register_timer(&self, interval: Duration, token: u64);
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
    pub network_state: Arc<NetworkState>,
    pub p2p_control: ServiceControl,
}

impl DefaultCKBProtocolContext {
    pub fn new(
        protocol_id: ProtocolId,
        network_state: Arc<NetworkState>,
        p2p_control: ServiceControl,
    ) -> Self {
        DefaultCKBProtocolContext {
            protocol_id,
            network_state,
            p2p_control,
        }
    }
}

impl CKBProtocolContext for DefaultCKBProtocolContext {
    fn send(&mut self, peer_index: PeerIndex, data: Vec<u8>) -> Result<(), Error> {
        self.send_protocol(peer_index, self.protocol_id, data)
    }
    fn send_protocol(
        &mut self,
        peer_index: PeerIndex,
        protocol_id: ProtocolId,
        data: Vec<u8>,
    ) -> Result<(), Error> {
        let peer_id = self
            .network_state
            .get_peer_id(peer_index)
            .ok_or_else(|| PeerError::IndexNotFound(peer_index))?;
        let session_id = self
            .network_state
            .peers_registry
            .read()
            .get(&peer_id)
            .ok_or_else(|| PeerError::NotFound(peer_id.to_owned()))
            .and_then(|peer| {
                peer.protocol_version(protocol_id)
                    .ok_or_else(|| PeerError::ProtocolNotFound(peer_id.to_owned(), protocol_id))
                    .map(|_| peer.session_id)
            })?;

        self.p2p_control
            .send_message(session_id, protocol_id, data.to_vec())
            .map_err(|_| {
                Error::P2P(format!(
                    "error send to peer {:?} protocol {}",
                    peer_id, protocol_id
                ))
            })
    }
    // report peer behaviour
    fn report_peer(&self, peer_index: PeerIndex, behaviour: Behaviour) -> Result<(), Error> {
        debug!(target: "network", "report peer {} behaviour: {:?}", peer_index, behaviour);
        if let Some(peer_id) = self.network_state.get_peer_id(peer_index) {
            if self
                .network_state
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
        if let Some(peer_id) = self.network_state.get_peer_id(peer_index) {
            self.network_state.ban_peer(&peer_id, timeout)
        }
    }
    // disconnect from peer
    fn disconnect(&self, peer_index: PeerIndex) {
        debug!(target: "network", "disconnect peer {}", peer_index);
        if let Some(peer_id) = self.network_state.get_peer_id(peer_index) {
            self.network_state.drop_peer(&peer_id);
        }
    }

    fn register_timer(&self, interval: Duration, token: u64) {
        // TODO: handle channel is full
        if let Err(err) =
            self.p2p_control
                .clone()
                .set_service_notify(self.protocol_id, interval, token)
        {
            error!(target: "network", "register timer error: {:?}", err);
        }
    }

    fn session_info(&self, peer_index: PeerIndex) -> Option<SessionInfo> {
        if let Some(session) = self
            .network_state
            .get_peer_id(peer_index)
            .map(|peer_id| self.network_state.session_info(&peer_id, self.protocol_id))
        {
            session
        } else {
            None
        }
    }

    fn protocol_version(&self, peer_index: PeerIndex, protocol_id: ProtocolId) -> Option<u8> {
        if let Some(protocol_version) = self.network_state.get_peer_id(peer_index).map(|peer_id| {
            self.network_state
                .peer_protocol_version(&peer_id, protocol_id)
        }) {
            protocol_version
        } else {
            None
        }
    }

    fn protocol_id(&self) -> ProtocolId {
        self.protocol_id
    }

    fn connected_peers(&self) -> Vec<PeerIndex> {
        self.network_state.peers_indexes()
    }
}

pub trait CKBProtocolHandler: Sync + Send {
    // TODO: Remove (_service: &mut ServiceContext) argument later
    fn initialize(&self, _nc: Box<dyn CKBProtocolContext>);
    fn received(&self, _nc: Box<dyn CKBProtocolContext>, _peer: PeerIndex, _data: Bytes);
    fn connected(&self, _nc: Box<dyn CKBProtocolContext>, _peer: PeerIndex);
    fn disconnected(&self, _nc: Box<dyn CKBProtocolContext>, _peer: PeerIndex);
    fn timer_triggered(&self, _nc: Box<dyn CKBProtocolContext>, _timer: u64) {}
}
