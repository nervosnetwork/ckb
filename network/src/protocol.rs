use crate::ckb_protocol::CKBProtocolOutput;
use crate::CKBProtocolHandler;
use futures::{Future, Stream};
use libp2p::core::{Multiaddr, PeerId};
use libp2p::identify::{IdentifyInfo, IdentifySender};
use libp2p::{kad, ping};
use std::io::Error as IoError;
use std::sync::Arc;

pub enum Protocol<T> {
    Kad(
        kad::KadConnecController,
        Box<Stream<Item = kad::KadIncomingRequest, Error = IoError> + Send>,
        PeerId,
        Multiaddr,
    ),
    Pong(Box<Future<Item = (), Error = IoError> + Send>, PeerId),
    Ping(
        ping::Pinger,
        Box<Future<Item = (), Error = IoError> + Send>,
        PeerId,
    ),
    IdentifyRequest(PeerId, IdentifyInfo, Multiaddr),
    IdentifyResponse(PeerId, IdentifySender<T>, Multiaddr),
    CKBProtocol(
        CKBProtocolOutput<Arc<CKBProtocolHandler>>,
        PeerId,
        Option<Multiaddr>,
    ),
}
