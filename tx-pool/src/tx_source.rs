//! Origin of a transaction entering the tx-pool pipeline.
//!
//! Distinguishing the source explicitly avoids overloading `Option<(Cycle,
//! PeerIndex)>` with multiple meanings (remote peer, local submission, or block
//! proposal notification).

use ckb_network::PeerIndex;
use ckb_types::core::Cycle;

/// The origin of a transaction entering the tx-pool pipeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TxSource {
    /// Submitted by a remote peer. Carries the peer's declared verification
    /// cycles and the peer index.
    Remote {
        /// Peer-declared cycles.
        cycles: Cycle,
        /// Remote peer index.
        peer: PeerIndex,
    },
    /// A local submission (including test-only paths).
    Local,
    /// Received as a block proposal notification.
    Proposal,
}

impl TxSource {
    /// Create a remote source with the given declared cycles and peer.
    pub(crate) fn remote(cycles: Cycle, peer: PeerIndex) -> Self {
        Self::Remote { cycles, peer }
    }

    /// Shorthand for `TxSource::Local`.
    pub(crate) fn local() -> Self {
        Self::Local
    }

    /// Returns the declared cycles if this is a remote submission.
    pub(crate) fn cycles(&self) -> Option<Cycle> {
        match *self {
            Self::Remote { cycles, .. } => Some(cycles),
            _ => None,
        }
    }

    /// Returns the peer index if this is a remote submission.
    pub(crate) fn peer(&self) -> Option<PeerIndex> {
        match *self {
            Self::Remote { peer, .. } => Some(peer),
            _ => None,
        }
    }
}
