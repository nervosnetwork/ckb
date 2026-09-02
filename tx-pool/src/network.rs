//! Network abstraction used by the tx-pool.
//!
//! The tx-pool only needs a tiny subset of the full network API (currently just
//! the ability to ban misbehaving peers).  This trait lets tests and benchmarks
//! inject a lightweight mock instead of spinning up a real
//! [`ckb_network::NetworkService`].

use ckb_network::{NetworkController, PeerIndex};
use std::sync::Arc;
use std::time::Duration;

/// Minimal network interface required by the transaction pool.
pub trait TxPoolNetwork: Send + Sync + 'static {
    /// Ban a peer for the specified duration.
    fn ban_peer(&self, peer: PeerIndex, duration: Duration, reason: String);
}

impl TxPoolNetwork for NetworkController {
    fn ban_peer(&self, peer: PeerIndex, duration: Duration, reason: String) {
        self.ban_peer(peer, duration, reason);
    }
}

impl<T: TxPoolNetwork + ?Sized> TxPoolNetwork for Arc<T> {
    fn ban_peer(&self, peer: PeerIndex, duration: Duration, reason: String) {
        (**self).ban_peer(peer, duration, reason);
    }
}

/// No-op network implementation for tests and benchmarks.
#[cfg(any(test, feature = "internal"))]
#[derive(Debug, Default, Clone)]
pub struct DummyTxPoolNetwork;

#[cfg(any(test, feature = "internal"))]
impl TxPoolNetwork for DummyTxPoolNetwork {
    fn ban_peer(&self, _peer: PeerIndex, _duration: Duration, _reason: String) {}
}

/// Type-erased handle to a [`TxPoolNetwork`] implementation.
pub type TxPoolNetworkHandle = Arc<dyn TxPoolNetwork>;
