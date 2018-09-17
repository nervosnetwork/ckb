pub mod many_messages;
pub mod many_nodes;
pub mod simple;

use network::SessionInfo;
use network::{PeerId, TimerToken};

#[derive(Debug)]
pub enum ProtocolEvent {
    Initialize,
    Read(PeerId, Box<SessionInfo>, usize),
    Connected(PeerId, Box<SessionInfo>),
    Disconnected(PeerId, Box<SessionInfo>),
    Timeout(TimerToken),
}
