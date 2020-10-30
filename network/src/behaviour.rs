use crate::Score;

/// Peers behaviours
/// we maintain a score to each peer
/// report peer bahaviour will affects peer's score
///
/// Currently this feature is disabled, maybe someday we will add it back or totally remove it.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Behaviour {
    #[cfg(test)]
    TestGood,
    #[cfg(test)]
    TestBad,
}

impl Behaviour {
    /// TODO(doc): @driftluo
    pub fn score(self) -> Score {
        #[cfg(test)]
        match self {
            Behaviour::TestGood => 10,
            Behaviour::TestBad => -10,
        }
        #[cfg(not(test))]
        0
    }
}
