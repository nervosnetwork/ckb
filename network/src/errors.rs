use crate::{peer_store::sqlite::DBError, ProtocolId};
use p2p::{error::Error as P2PError, secio::PeerId, SessionId};
use std::fmt;
use std::fmt::Display;
use std::io::Error as IoError;

#[derive(Debug)]
pub enum Error {
    Peer(PeerError),
    Config(ConfigError),
    Protocol(ProtocolError),
    Io(IoError),
    P2P(P2PError),
    DB(DBError),
    Shutdown,
}

#[derive(Debug)]
pub enum ConfigError {
    BadAddress,
    InvalidKey,
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

impl From<ConfigError> for Error {
    fn from(err: ConfigError) -> Error {
        Error::Config(err)
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

impl From<DBError> for Error {
    fn from(err: DBError) -> Error {
        Error::DB(err)
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl Display for ConfigError {
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
