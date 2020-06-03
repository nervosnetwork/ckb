use crate::ProtocolId;
use p2p::{error::Error as P2PError, secio::PeerId, SessionId};
use std::fmt;
use std::fmt::Display;
use std::io::Error as IoError;

pub type Result<T> = ::std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    Peer(PeerError),
    Protocol(ProtocolError),
    Io(IoError),
    P2P(P2PError),
    Addr(AddrError),
    Dial(String),
    PeerStore(PeerStoreError),
    Shutdown,
}

#[derive(Debug)]
pub enum PeerStoreError {
    /// indicate the peer store is full
    EvictionFailed,
    Serde(serde_json::Error),
}

#[derive(Debug, Eq, PartialEq)]
pub enum PeerError {
    SessionExists(SessionId),
    PeerIdExists(PeerId),
    NotFound(PeerId),
    NonReserved,
    Banned,
    ReachMaxInboundLimit,
    ReachMaxOutboundLimit,
}

#[derive(Debug)]
pub enum ProtocolError {
    NotFound(ProtocolId),
    DisallowRegisterTimer,
    Duplicate(ProtocolId),
}

#[derive(Debug)]
pub enum AddrError {
    InvalidPeerId,
    MissingIP,
    MissingPort,
}

impl From<PeerStoreError> for Error {
    fn from(err: PeerStoreError) -> Error {
        Error::PeerStore(err)
    }
}

impl From<PeerError> for Error {
    fn from(err: PeerError) -> Error {
        Error::Peer(err)
    }
}

impl From<IoError> for Error {
    fn from(err: IoError) -> Error {
        Error::Io(err)
    }
}

impl From<ProtocolError> for Error {
    fn from(err: ProtocolError) -> Error {
        Error::Protocol(err)
    }
}

impl From<P2PError> for Error {
    fn from(err: P2PError) -> Error {
        Error::P2P(err)
    }
}

impl From<AddrError> for Error {
    fn from(err: AddrError) -> Error {
        Error::Addr(err)
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl Display for PeerError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}
