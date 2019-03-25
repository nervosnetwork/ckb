use crate::Score;
pub type Behaviour = (Score, &'static str);

pub const CONNECT: Behaviour = (10, "peer connected");
pub const UNEXPECTED_DISCONNECT: Behaviour = (-10, "peer unexpected disconnected");
pub const PING: Behaviour = (10, "peer ping");
pub const FAILED_TO_PING: Behaviour = (-20, "failed to ping");
pub const SYNC_USELESS: Behaviour = (-50, "sync useless");
pub const UNEXPECTED_NETWORK_MESSAGE: Behaviour = (-50, "unexpected network message");
pub const NETWORK_TIMEOUT: Behaviour = (-20, "network timeout");
