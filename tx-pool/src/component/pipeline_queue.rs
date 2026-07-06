//! Common interface shared by the verify queue and the ordered resolve queue.
//!
//! Both queues cache `total_tx_size`, track in-flight outputs via
//! [`FlightTracker`], and expose a [`Notify`] subscription.  This trait captures
//! those shared behaviours so the concrete types only implement what is
//! queue-specific.

use crate::component::flight_tracker::FlightTracker;
use crate::component::saturating_counter::SaturatingCounter;
use crate::constants::DEFAULT_MAX_PIPELINE_QUEUE_TX_SIZE;
use crate::error::Reject;
use ckb_network::PeerIndex;
use ckb_types::core::TransactionView;
use ckb_types::packed::ProposalShortId;
use std::sync::Arc;
use tokio::sync::Notify;

pub(crate) trait PipelineQueue {
    /// Transaction-like item stored in the queue.
    type Tx;

    /// Cached total serialized size of all live items in the queue.
    fn total_tx_size(&self) -> &SaturatingCounter<usize>;

    /// In-flight output tracker.
    fn flight(&self) -> &FlightTracker;

    /// Notification handle used by workers to wait for new items.
    fn ready_rx(&self) -> &Arc<Notify>;

    /// Returns `true` if adding `add_tx_size` would reach or exceed the queue
    /// size limit.
    fn is_full(&self, add_tx_size: usize) -> bool {
        self.total_tx_size().get().saturating_add(add_tx_size) >= DEFAULT_MAX_PIPELINE_QUEUE_TX_SIZE
    }

    /// Returns `true` if `tx` spends an output produced by another tx that is
    /// currently in this queue.
    fn depends_on(&self, tx: &TransactionView) -> bool {
        self.flight().depends_on(tx)
    }

    /// Returns a clone of the readiness notification handle.
    fn subscribe(&self) -> Arc<Notify> {
        Arc::clone(self.ready_rx())
    }

    /// Returns `true` if the queue contains no live txs.
    fn is_empty(&self) -> bool;

    /// Returns `true` if the queue contains a tx with the specified id.
    fn contains_key(&self, id: &ProposalShortId) -> bool;

    /// Remove a tx from the queue by its short id.
    fn remove_tx(&mut self, id: &ProposalShortId) -> Option<Self::Tx>;

    /// Remove all txs submitted by the given peer.
    fn remove_txs_by_peer(&mut self, peer: &PeerIndex) -> Vec<ProposalShortId>;

    /// Add a tx to the queue.
    ///
    /// Returns `Ok(true)` if the tx was newly added, `Ok(false)` if it was a
    /// duplicate, and `Err(Reject::Full)` if the queue is full.
    fn add_tx(&mut self, tx: Self::Tx) -> Result<bool, Reject>;

    /// Clears the queue, removing all elements.
    fn clear(&mut self);
}
