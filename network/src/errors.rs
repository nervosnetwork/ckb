use crate::{PeerIndex, ProtocolId};
use p2p::secio::PeerId;
use std::error;
use std::fmt;
use std::fmt::Display;
use std::io::Error as IoError;

#[derive(Debug)]
pub enum Error {
    Peer(PeerError),
    Config(ConfigError),
    Protocol(ProtocolError),
    Io(IoError),
    Shutdown,
}

#[derive(Debug)]
pub enum ConfigError {
    BadAddress,
    InvalidKey,
}

#[derive(Debug)]
pub enum PeerError {
    IndexNotFound(PeerIndex),
    ProtocolNotFound(PeerId, ProtocolId),
    NotFound(PeerId),
    NonReserved(PeerId),
    Banned(PeerId),
    ReachMaxInboundLimit(PeerId),
    ReachMaxOutboundLimit(PeerId),
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

impl Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self)
    }
}

impl error::Error for Error {
    fn description(&self) -> &str {
        "ckb network error"
    }

    fn cause(&self) -> Option<&error::Error> {
        // Generic error, underlying cause isn't tracked.
        None
    }
}
