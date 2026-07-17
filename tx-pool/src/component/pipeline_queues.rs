use crate::component::ordered_resolve_queue::OrderedResolveQueue;
use crate::component::pre_check_queue::PreCheckQueue;
use crate::component::rbf_candidates::RbfCandidates;
use crate::component::verify_queue::VerifyQueue;
use tokio::sync::RwLock;

/// Bundles the pipeline queues and the in-flight RBF gate so that
/// [`TxPoolService`] does not expose them as individual fields.
///
/// The pre-check, ordered-resolve and verify queues share the same lifecycle
/// and are always accessed as a unit. `rbf_candidates` lives here as well
/// because it participates in the same lock hierarchy
/// (`ordered_resolve_queue → rbf_candidates → verify_queue`), so keeping the
/// four together makes the ordering visible in the type structure.
///
/// The whole bundle is shared as one `Arc<PipelineQueues>` between the
/// service and its workers, which removes the per-queue `Arc`s that used to
/// be cloned independently into every worker.
pub(crate) struct PipelineQueues {
    pub ordered_resolve_queue: RwLock<OrderedResolveQueue>,
    pub verify_queue: RwLock<VerifyQueue>,
    pub pre_check_queue: PreCheckQueue,
    pub rbf_candidates: RwLock<RbfCandidates>,
}
