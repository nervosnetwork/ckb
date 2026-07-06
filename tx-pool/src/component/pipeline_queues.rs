use crate::component::ordered_resolve_queue::OrderedResolveQueue;
use crate::component::pre_check_queue::PreCheckQueue;
use crate::component::verify_queue::VerifyQueue;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Bundles the three pipeline queues so that [`TxPoolService`] does not expose
/// them as individual fields.
///
/// The pre-check, ordered-resolve and verify queues are always passed together
/// and share the same lifecycle, so grouping them reduces field count and makes
/// "queue-related" state easier to identify.
#[derive(Clone)]
pub(crate) struct PipelineQueues {
    pub ordered_resolve_queue: Arc<RwLock<OrderedResolveQueue>>,
    pub verify_queue: Arc<RwLock<VerifyQueue>>,
    pub pre_check_queue: Arc<PreCheckQueue>,
}
