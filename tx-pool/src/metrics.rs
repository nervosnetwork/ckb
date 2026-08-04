//! Low-cardinality operational projections of existing tx-pool authority.
//!
//! Metrics never participate in admission, scheduling, settlement or retry.
//! Callers copy already-maintained counters while holding their authority lock
//! and publish only after releasing it.

use crate::error::Reject;
use ckb_types::core::error::OutPointError;

fn gauge_value(value: usize) -> i64 {
    i64::try_from(value).map_or(i64::MAX, |converted| converted)
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct KernelUsage {
    pub(crate) total_entries: usize,
    pub(crate) total_bytes: usize,
    pub(crate) remote_entries: usize,
    pub(crate) remote_bytes: usize,
    pub(crate) conflict_entries: usize,
    pub(crate) conflict_bytes: usize,
    pub(crate) active_work: usize,
}

impl KernelUsage {
    pub(crate) fn publish(self) {
        let Some(metrics) = ckb_metrics::handle() else {
            return;
        };
        let residency = &metrics.ckb_tx_pool_pipeline_residency;
        residency.total_entries.set(gauge_value(self.total_entries));
        residency.total_bytes.set(gauge_value(self.total_bytes));
        residency
            .remote_entries
            .set(gauge_value(self.remote_entries));
        residency.remote_bytes.set(gauge_value(self.remote_bytes));
        residency
            .conflict_entries
            .set(gauge_value(self.conflict_entries));
        residency
            .conflict_bytes
            .set(gauge_value(self.conflict_bytes));
        residency.active_work.set(gauge_value(self.active_work));
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct EffectUsage {
    pub(crate) remote_batches: usize,
    pub(crate) remote_bytes: usize,
    pub(crate) ordinary_batches: usize,
    pub(crate) ordinary_bytes: usize,
    pub(crate) total_batches: usize,
    pub(crate) total_bytes: usize,
}

impl EffectUsage {
    pub(crate) fn publish(self) {
        let Some(metrics) = ckb_metrics::handle() else {
            return;
        };
        let usage = &metrics.ckb_tx_pool_effect_usage;
        usage.remote_batches.set(gauge_value(self.remote_batches));
        usage.remote_bytes.set(gauge_value(self.remote_bytes));
        usage
            .ordinary_batches
            .set(gauge_value(self.ordinary_batches));
        usage.ordinary_bytes.set(gauge_value(self.ordinary_bytes));
        usage.total_batches.set(gauge_value(self.total_batches));
        usage.total_bytes.set(gauge_value(self.total_bytes));
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct OperationalMetrics {
    pub(crate) kernel: KernelUsage,
    pub(crate) effects: EffectUsage,
}

impl OperationalMetrics {
    pub(crate) fn publish(self) {
        self.kernel.publish();
        self.effects.publish();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RejectionClass {
    Malformed,
    Policy,
    Capacity,
    Duplicate,
    Internal,
}

impl RejectionClass {
    pub(crate) fn from_reject(reject: &Reject) -> Self {
        match reject {
            Reject::Malformed(_, _)
            | Reject::DeclaredWrongCycles(_, _)
            | Reject::Resolve(OutPointError::OverMaxDepExpansionLimit) => Self::Malformed,
            Reject::Verification(_) if reject.is_malformed_tx() => Self::Malformed,
            Reject::Full(_) | Reject::ExceededMaximumAncestorsCount => Self::Capacity,
            Reject::Duplicated(_) => Self::Duplicate,
            Reject::Internal(_) => Self::Internal,
            Reject::LowFeeRate(_, _, _)
            | Reject::ExceededTransactionSizeLimit(_, _)
            | Reject::Resolve(_)
            | Reject::Verification(_)
            | Reject::Expiry(_)
            | Reject::RBFRejected(_)
            | Reject::Invalidated(_) => Self::Policy,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RejectionMetrics {
    malformed: usize,
    policy: usize,
    capacity: usize,
    duplicate: usize,
    internal: usize,
}

impl RejectionMetrics {
    pub(crate) fn record(&mut self, reject: &Reject) {
        let counter = match RejectionClass::from_reject(reject) {
            RejectionClass::Malformed => &mut self.malformed,
            RejectionClass::Policy => &mut self.policy,
            RejectionClass::Capacity => &mut self.capacity,
            RejectionClass::Duplicate => &mut self.duplicate,
            RejectionClass::Internal => &mut self.internal,
        };
        *counter = counter.saturating_add(1);
    }

    pub(crate) fn publish(self) {
        let Some(metrics) = ckb_metrics::handle() else {
            return;
        };
        let counters = &metrics.ckb_tx_pool_pipeline_rejections;
        let count = |value| u64::try_from(value).map_or(u64::MAX, |converted| converted);
        counters.malformed.inc_by(count(self.malformed));
        counters.policy.inc_by(count(self.policy));
        counters.capacity.inc_by(count(self.capacity));
        counters.duplicate.inc_by(count(self.duplicate));
        counters.internal.inc_by(count(self.internal));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FailureBoundary {
    TypedFault,
    WorkerExit,
    HandlerUnwind,
    EffectPublisher,
}

pub(crate) fn record_failure(boundary: FailureBoundary) {
    let Some(metrics) = ckb_metrics::handle() else {
        return;
    };
    let counters = &metrics.ckb_tx_pool_pipeline_failures;
    match boundary {
        FailureBoundary::TypedFault => counters.typed_fault.inc(),
        FailureBoundary::WorkerExit => counters.worker_exit.inc(),
        FailureBoundary::HandlerUnwind => counters.handler_unwind.inc(),
        FailureBoundary::EffectPublisher => counters.effect_publisher.inc(),
    }
}
