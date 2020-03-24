/// CKB evicts inactive peers in `sync` protocol; but due to P2P connection design,
/// a malicious peer may choose not to open `sync` protocol, to sneak from the eviction mechanism;
/// this service periodically check peers opened sub-protocols, to make sure no malicious connection.
///
/// Currently, 2 sub-protocols types are valid:
///
/// 1. fully-opened: all sub-protocols(except feeler) are opened.
/// 2. feeler: only open feeler protocol is open.
///
/// Other protocols will be closed after a timeout.
use crate::{
    network::{disconnect_with_message, FEELER_PROTOCOL_ID},
    NetworkState, Peer, ProtocolId,
};
use ckb_logger::{debug, warn};
use futures::{Future, Stream};
use p2p::service::ServiceControl;
use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::{Duration, Instant},
};
use tokio::time::Interval;

const TIMEOUT: Duration = Duration::from_secs(60);
const CHECK_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Debug)]
enum ProtocolType {
    FullyOpen,
    Feeler,
}

impl std::fmt::Display for ProtocolType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::result::Result<(), std::fmt::Error> {
        use ProtocolType::*;
        match self {
            FullyOpen => write!(f, "fully-open")?,
            Feeler => write!(f, "feeler")?,
        }
        Ok(())
    }
}

#[derive(Debug)]
enum ProtocolTypeError {
    Incomplete,
}

impl std::fmt::Display for ProtocolTypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::result::Result<(), std::fmt::Error> {
        use ProtocolTypeError::*;
        match self {
            Incomplete => write!(f, "incomplete open protocols")?,
        }
        Ok(())
    }
}

pub struct ProtocolTypeCheckerService {
    network_state: Arc<NetworkState>,
    p2p_control: ServiceControl,
    interval: Option<Interval>,
    fully_open_required_protocol_ids: Vec<ProtocolId>,
}

impl ProtocolTypeCheckerService {
    pub fn new(
        network_state: Arc<NetworkState>,
        p2p_control: ServiceControl,
        fully_open_required_protocol_ids: Vec<ProtocolId>,
    ) -> Self {
        ProtocolTypeCheckerService {
            network_state,
            p2p_control,
            interval: None,
            fully_open_required_protocol_ids,
        }
    }

    fn check_protocol_type(&self) {
        self.network_state.with_peer_registry(|reg| {
            let now = Instant::now();
            for (session_id, peer) in reg.peers() {
                // skip just connected peers
                if now.duration_since(peer.connected_time) < TIMEOUT {
                    continue;
                }

                // check open protocol type
                if let Err(err) = self.opened_procotol_type(peer) {
                    debug!(
                        "close peer {:?} due to open protocols error: {}",
                        peer.peer_id, err
                    );
                    if let Err(err) = disconnect_with_message(
                        &self.p2p_control,
                        *session_id,
                        &format!("open protocols error: {}", err),
                    ) {
                        debug!("Disconnect failed {:?}, error: {:?}", session_id, err);
                    }
                }
            }
        });
    }

    fn opened_procotol_type(&self, peer: &Peer) -> Result<ProtocolType, ProtocolTypeError> {
        if peer
            .protocols
            .contains_key(&ProtocolId::new(FEELER_PROTOCOL_ID))
        {
            Ok(ProtocolType::Feeler)
        } else if self
            .fully_open_required_protocol_ids
            .iter()
            .all(|p_id| peer.protocols.contains_key(p_id))
        {
            Ok(ProtocolType::FullyOpen)
        } else {
            Err(ProtocolTypeError::Incomplete)
        }
    }
}

impl Future for ProtocolTypeCheckerService {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.interval.is_none() {
            self.interval = Some(tokio::time::interval(CHECK_INTERVAL));
        }
        let mut interval = self.interval.take().unwrap();
        loop {
            match Pin::new(&mut interval).as_mut().poll_next(cx) {
                Poll::Ready(Some(_tick)) => self.check_protocol_type(),
                Poll::Ready(None) => {
                    warn!("ckb protocol checker service stopped");
                    return Poll::Ready(());
                }
                Poll::Pending => {
                    self.interval = Some(interval);
                    return Poll::Pending;
                }
            }
        }
    }
}
