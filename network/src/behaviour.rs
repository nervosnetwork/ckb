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
    pub fn score(self) -> Score {
        #[allow(unreachable_patterns)]
        match self {
            #[cfg(test)]
            Behaviour::TestGood => 10,
            #[cfg(test)]
            Behaviour::TestBad => -10,
            _ => 0,
        }
    }
}
