use crate::ckb_protocol::CKBProtocolOutput;
use crate::CKBProtocolHandler;
use futures::Future;
use p2p::{multiaddr::Multiaddr, PeerId};
use libp2p::identify::{IdentifyInfo, IdentifySender};
use libp2p::ping;
use std::io::Error as IoError;
use std::sync::Arc;

pub enum Protocol<T> {
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
        Multiaddr,
    ),
}
