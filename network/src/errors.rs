use p2p::{
    error::{
        DialerErrorKind, ListenErrorKind, ProtocolHandleErrorKind, SendErrorKind,
        TransportErrorKind,
    },
    secio::PeerId,
    SessionId,
};
use std::fmt;
use std::fmt::Display;
use std::io::Error as IoError;

pub type Result<T> = ::std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    Peer(PeerError),
    Io(IoError),
    P2P(P2PError),
    Addr(AddrError),
    Dial(String),
    PeerStore(PeerStoreError),
}

#[derive(Debug)]
pub enum P2PError {
    Transport(TransportErrorKind),
    Protocol(ProtocolHandleErrorKind),
    Dail(DialerErrorKind),
    Listen(ListenErrorKind),
    Send(SendErrorKind),
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
    NonReserved,
    Banned,
    ReachMaxInboundLimit,
    ReachMaxOutboundLimit,
}

#[derive(Debug)]
pub enum AddrError {
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

impl From<TransportErrorKind> for Error {
    fn from(err: TransportErrorKind) -> Error {
        Error::P2P(P2PError::Transport(err))
    }
}

impl From<ProtocolHandleErrorKind> for Error {
    fn from(err: ProtocolHandleErrorKind) -> Error {
        Error::P2P(P2PError::Protocol(err))
    }
}

impl From<DialerErrorKind> for Error {
    fn from(err: DialerErrorKind) -> Error {
        Error::P2P(P2PError::Dail(err))
    }
}

impl From<ListenErrorKind> for Error {
    fn from(err: ListenErrorKind) -> Error {
        Error::P2P(P2PError::Listen(err))
    }
}

impl From<SendErrorKind> for Error {
    fn from(err: SendErrorKind) -> Error {
        Error::P2P(P2PError::Send(err))
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

impl Display for P2PError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}
