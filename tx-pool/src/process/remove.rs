//! Wake-up helper for the ordered resolver.
//!
//! Cross-structure removal and the orphan flight-check heuristics live in
//! `service::pipeline_ops`; this module only keeps the resolver wake-up
//! used by those paths.

use crate::component::pipeline_queue::PipelineQueue;

impl super::TxPoolService {
    /// Notify the ordered resolver if there are jobs waiting.
    ///
    /// Must be called after a transaction is removed from the verify queue or
    /// the in-pool set: the removed tx may have had descendants waiting in
    /// the ordered resolve queue, and waking the resolver lets them be retried
    /// (and rejected if the parent is gone) promptly.
    pub(crate) async fn wake_ordered_resolver_if_needed(&self) {
        let ordered = self.pipeline.queues.ordered_resolve_queue.read().await;
        if !ordered.is_empty() {
            ordered.subscribe().notify_one();
        }
    }
}
