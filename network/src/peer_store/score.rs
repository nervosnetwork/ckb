use fnv::FnvHashMap;
use std::time::Duration;
/// Peer Behaviours
#[allow(dead_code)]
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub enum Behaviour {
    FailedToConnect,
    FailedToPing,
    Ping,
    Connect,
    UnexpectedDisconnect,
}

pub type Score = i32;

pub struct ScoringSchema {
    schema: FnvHashMap<Behaviour, Score>,
    peer_init_score: Score,
    ban_score: Score,
    default_ban_timeout: Duration,
}

impl ScoringSchema {
    pub fn new_default() -> Self {
        let schema = [
            (Behaviour::FailedToConnect, -20),
            (Behaviour::FailedToPing, -10),
            (Behaviour::Ping, 5),
            (Behaviour::Connect, 10),
            (Behaviour::UnexpectedDisconnect, -20),
        ]
        .iter()
        .cloned()
        .collect();
        ScoringSchema {
            schema,
            peer_init_score: 100,
            ban_score: 40,
            default_ban_timeout: Duration::from_secs(24 * 3600),
        }
    }

    pub fn peer_init_score(&self) -> Score {
        self.peer_init_score
    }

    pub fn ban_score(&self) -> Score {
        self.ban_score
    }

    pub fn get_score(&self, behaviour: Behaviour) -> Option<Score> {
        self.schema.get(&behaviour).cloned()
    }

    pub fn default_ban_timeout(&self) -> Duration {
        self.default_ban_timeout
    }
}

impl Default for ScoringSchema {
    fn default() -> Self {
        ScoringSchema::new_default()
    }
}
