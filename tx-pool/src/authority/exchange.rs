//! Ephemeral retained-compute topology evidence.
//!
//! These types identify bounded worker slots and their execution capability.
//! They never own a transaction or policy decision; the authority scheduler is
//! the only component that may turn an idle slot into checked-out work.

use super::state::{VerifyCapability, WorkPermit};
use std::sync::Arc;
use tokio::sync::{Notify, OwnedSemaphorePermit};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum ComputeWorkerSlotId {
    OrderedResolve,
    Verifier(usize),
}

/// Stable identity and role of one retained-compute worker within an authority
/// generation. Construction is sealed to the authority topology, so callers
/// cannot invent a role which the spawned worker cannot execute.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ComputeWorkerSlot {
    OrderedResolve,
    Verifier(ComputeVerifierSlot),
}

/// A verifier slot carries both the stable topology identity and the maximum
/// cycle class its worker can execute. Keeping this separate makes it
/// impossible for the verifier worker variant to contain an ordered-resolver
/// slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ComputeVerifierSlot {
    worker_id: usize,
    capability: VerifyCapability,
}

/// One fair token from the shared tx-pool execution gate. It carries no
/// transaction identity or membership right. A checkout can be committed
/// only while this move-only value is already owned; dropping it returns the
/// count to Direct and retained contenders through Tokio's fair semaphore.
#[derive(Debug)]
#[must_use = "a transient compute permit must guard one complete execution"]
pub(in crate::authority) struct AuthorityComputeExecutionPermit {
    permit: Option<OwnedSemaphorePermit>,
    released: Arc<Notify>,
}

impl AuthorityComputeExecutionPermit {
    pub(in crate::authority) fn new(permit: OwnedSemaphorePermit, released: Arc<Notify>) -> Self {
        Self {
            permit: Some(permit),
            released,
        }
    }
}

impl Drop for AuthorityComputeExecutionPermit {
    fn drop(&mut self) {
        // Release the fair semaphore count before publishing its derived
        // level. `notify_one` coalesces releases and stores one wake when the
        // coordinator has not yet polled, so a failed immediate acquisition
        // cannot miss the transition to available capacity.
        drop(self.permit.take());
        self.released.notify_one();
    }
}

/// Linear permission to assign one stable worker slot in an atomic exchange.
/// Slot identity alone is deliberately insufficient: a completion may make a
/// worker idle while Direct already owns the newly released fair count.
#[derive(Debug)]
#[must_use = "a worker grant must be assigned or returned to the fair gate"]
pub(in crate::authority) struct ComputeWorkerGrant {
    slot: ComputeWorkerSlot,
    execution: AuthorityComputeExecutionPermit,
}

impl ComputeWorkerGrant {
    pub(in crate::authority) fn new(
        slot: ComputeWorkerSlot,
        execution: AuthorityComputeExecutionPermit,
    ) -> Self {
        Self { slot, execution }
    }

    pub(in crate::authority) fn slot(&self) -> ComputeWorkerSlot {
        self.slot
    }

    pub(in crate::authority) fn into_parts(
        self,
    ) -> (ComputeWorkerSlot, AuthorityComputeExecutionPermit) {
        (self.slot, self.execution)
    }
}

impl ComputeVerifierSlot {
    pub(super) const fn new(worker_id: usize, capability: VerifyCapability) -> Self {
        Self {
            worker_id,
            capability,
        }
    }

    pub(super) const fn worker_id(self) -> usize {
        self.worker_id
    }
}

impl ComputeWorkerSlot {
    pub(super) const fn ordered_resolve() -> Self {
        Self::OrderedResolve
    }

    pub(super) const fn id(self) -> ComputeWorkerSlotId {
        match self {
            Self::OrderedResolve => ComputeWorkerSlotId::OrderedResolve,
            Self::Verifier(slot) => ComputeWorkerSlotId::Verifier(slot.worker_id),
        }
    }

    /// Stable fair-permit probe order. The ordered resolver remains first so
    /// a saturated Verify backlog cannot repeatedly reclaim the only released
    /// permit and starve dependency progress. Work selection inside an atomic
    /// exchange has its own verifier-primary phases and does not reuse this
    /// permit-acquisition order as policy.
    pub(super) const fn canonical_key(self) -> (u8, usize) {
        match self {
            Self::OrderedResolve => (0, 0),
            Self::Verifier(ComputeVerifierSlot {
                worker_id,
                capability: VerifyCapability::SmallCycleOnly,
            }) => (1, worker_id),
            Self::Verifier(ComputeVerifierSlot {
                worker_id,
                capability: VerifyCapability::Any,
            }) => (2, worker_id),
        }
    }

    /// Deterministic work-selection order after fair permits are already
    /// owned. Every verifier primary is considered before Resolve fallback;
    /// the ordered resolver remains the final independent progress lane.
    pub(super) const fn work_selection_key(self) -> (u8, usize) {
        match self {
            Self::Verifier(ComputeVerifierSlot {
                worker_id,
                capability: VerifyCapability::SmallCycleOnly,
            }) => (0, worker_id),
            Self::Verifier(ComputeVerifierSlot {
                worker_id,
                capability: VerifyCapability::Any,
            }) => (1, worker_id),
            Self::OrderedResolve => (2, 0),
        }
    }

    pub(super) const fn primary_permit(self) -> WorkPermit {
        match self {
            Self::OrderedResolve => WorkPermit::ResolveOnly,
            Self::Verifier(slot) => WorkPermit::VerifyOnly(slot.capability),
        }
    }

    pub(super) const fn fallback_permit(self) -> Option<WorkPermit> {
        match self {
            Self::OrderedResolve => None,
            Self::Verifier(slot) => Some(WorkPermit::ResolveThenVerify(slot.capability)),
        }
    }
}

impl From<ComputeVerifierSlot> for ComputeWorkerSlot {
    fn from(slot: ComputeVerifierSlot) -> Self {
        Self::Verifier(slot)
    }
}
