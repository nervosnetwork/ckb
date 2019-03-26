use crate::Score;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Behaviour {
    Connect,
    Ping,
    FailedToPing,
    Timeout,
    SyncUseless,
    UnexpectedMessage,
    UnexpectedDisconnect,
}

impl Behaviour {
    pub fn score(self) -> Score {
        use Behaviour::*;
        match self {
            Connect => 10,
            Ping => 10,
            FailedToPing => -20,
            Timeout => -20,
            SyncUseless => -50,
            UnexpectedMessage => -50,
            UnexpectedDisconnect => -10,
        }
    }
}
