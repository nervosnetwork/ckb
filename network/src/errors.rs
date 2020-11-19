//! Error module
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

/// alias result on network module
pub type Result<T> = ::std::result::Result<T, Error>;

/// All error on network module
#[derive(Debug)]
pub enum Error {
    /// Peer error
    Peer(PeerError),
    /// Io error
    Io(IoError),
    /// error from tentacle
    P2P(P2PError),
    /// address error
    Addr(AddrError),
    /// Dail error
    Dial(String),
    /// Peer store error
    PeerStore(PeerStoreError),
}

/// error from tentacle
#[derive(Debug)]
pub enum P2PError {
    /// Not support transport or some other error
    Transport(TransportErrorKind),
    /// Handle panic or other error
    Protocol(ProtocolHandleErrorKind),
    /// Dail error
    Dail(DialerErrorKind),
    /// Listen error
    Listen(ListenErrorKind),
    /// Net shutdown or too many message block on
    Send(SendErrorKind),
}

/// Peer store error
#[derive(Debug)]
pub enum PeerStoreError {
    /// indicate the peer store is full
    EvictionFailed,
    /// file data is not json format
    Serde(serde_json::Error),
}

/// Peer error
#[derive(Debug, Eq, PartialEq)]
pub enum PeerError {
    /// session already exist
    SessionExists(SessionId),
    /// peer id exist
    PeerIdExists(PeerId),
    /// Non-reserved peers
    NonReserved,
    /// peer is banned
    Banned,
    /// reach max inbound limit
    ReachMaxInboundLimit,
    /// reach max outbound limit
    ReachMaxOutboundLimit,
}

/// Address error
#[derive(Debug)]
pub enum AddrError {
    /// missing ip
    MissingIP,
    /// missing port
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
