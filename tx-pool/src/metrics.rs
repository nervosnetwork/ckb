//! Low-cardinality operational projections of existing tx-pool authority.
//!
//! Metrics never participate in admission, scheduling, settlement or retry.
//! Authority callers copy existing counters under their locks and publish after
//! releasing them. The independent relay mailbox records its own queue directly.

use crate::error::Reject;

fn gauge_value(value: usize) -> i64 {
    i64::try_from(value).map_or(i64::MAX, |converted| converted)
}

/// The relay mailbox records its own projection under its private lock, so an
/// older publisher cannot overwrite the receiver's drained observation.
pub(crate) fn relay_queue(items: usize, capacity: usize) {
    if let Some(metrics) = ckb_metrics::handle() {
        metrics
            .ckb_relay_tx_verify_result_queue_size
            .set(gauge_value(items));
        metrics
            .ckb_relay_tx_verify_result_queue_capacity
            .set(gauge_value(capacity));
    }
}

pub(crate) struct EntryUsage {
    pub(crate) pending: usize,
    pub(crate) gap: usize,
    pub(crate) proposed: usize,
}

impl EntryUsage {
    pub(crate) fn publish(self) {
        let Some(metrics) = ckb_metrics::handle() else {
            return;
        };
        let entries = &metrics.ckb_tx_pool_entry;
        entries.pending.set(gauge_value(self.pending));
        entries.gap.set(gauge_value(self.gap));
        entries.proposed.set(gauge_value(self.proposed));
    }
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RejectionClass {
    Malformed,
    Policy,
    Capacity,
    Duplicate,
}

impl RejectionClass {
    pub(crate) fn from_reject(reject: &Reject) -> Self {
        if reject.is_malformed_tx() {
            return Self::Malformed;
        }
        // Classify the remaining, non-malformed rejections without duplicating
        // the canonical validity rule (including its script-error exceptions).
        match reject {
            Reject::Full(_)
            | Reject::ExceededMaximumAncestorsCount
            | Reject::ExcessiveVerifyTime => Self::Capacity,
            Reject::Duplicated(_) => Self::Duplicate,
            Reject::Malformed(_, _)
            | Reject::DeclaredWrongCycles(_, _)
            | Reject::LowFeeRate(_, _, _)
            | Reject::ExceededTransactionSizeLimit(_, _)
            | Reject::Resolve(_)
            | Reject::Verification(_)
            | Reject::Expiry(_)
            | Reject::RBFRejected(_)
            | Reject::Invalidated(_) => Self::Policy,
        }
    }
}

impl RejectionClass {
    pub(crate) fn record(self) {
        let Some(metrics) = ckb_metrics::handle() else {
            return;
        };
        let counters = &metrics.ckb_tx_pool_pipeline_rejections;
        match self {
            Self::Malformed => counters.malformed.inc(),
            Self::Policy => counters.policy.inc(),
            Self::Capacity => counters.capacity.inc(),
            Self::Duplicate => counters.duplicate.inc(),
        }
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

/// Publish single-writer authority lock timings and descendant-index cardinality
/// strictly after releasing `Store::state.write()`.
pub(crate) fn record_authority_write(
    wait: std::time::Duration,
    hold: std::time::Duration,
    descendant_index_entries: usize,
) {
    let Some(metrics) = ckb_metrics::handle() else {
        return;
    };
    metrics
        .ckb_tx_pool_authority_write_wait_seconds
        .observe(wait.as_secs_f64());
    metrics
        .ckb_tx_pool_authority_write_hold_seconds
        .observe(hold.as_secs_f64());
    metrics
        .ckb_tx_pool_authority_runtime
        .descendant_index_entries
        .set(gauge_value(descendant_index_entries));
}

/// Record an ordinary commit or preview yielding to an active `ReorgGate`.
pub(crate) fn record_reorg_gate_wait() {
    if let Some(metrics) = ckb_metrics::handle() {
        metrics.ckb_tx_pool_reorg_gate_waits.inc();
    }
}

/// Publish direct-gate in-flight counts outside all authority locks.
pub(crate) fn record_direct_gate_usage(direct_inflight: usize, callback_read_inflight: usize) {
    let Some(metrics) = ckb_metrics::handle() else {
        return;
    };
    let runtime = &metrics.ckb_tx_pool_authority_runtime;
    runtime.direct_inflight.set(gauge_value(direct_inflight));
    runtime
        .callback_read_inflight
        .set(gauge_value(callback_read_inflight));
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_error::{ErrorKind, OtherError};
    use ckb_types::core::error::{ARGV_TOO_LONG_TEXT, OutPointError};

    #[test]
    fn rejection_metrics_preserve_validity_exceptions_and_local_refusals() {
        for (reject, expected) in [
            (
                Reject::Verification(ErrorKind::Script.because(OtherError::new("script failure"))),
                RejectionClass::Malformed,
            ),
            (
                Reject::Verification(
                    ErrorKind::Script.because(OtherError::new(ARGV_TOO_LONG_TEXT)),
                ),
                RejectionClass::Policy,
            ),
            (
                Reject::Resolve(OutPointError::OverMaxDepExpansionLimit),
                RejectionClass::Malformed,
            ),
            (Reject::Full("pipeline".into()), RejectionClass::Capacity),
            (
                Reject::ExceededMaximumAncestorsCount,
                RejectionClass::Capacity,
            ),
            (
                Reject::ExceededTransactionSizeLimit(2, 1),
                RejectionClass::Policy,
            ),
            (
                Reject::Duplicated(Default::default()),
                RejectionClass::Duplicate,
            ),
        ] {
            assert_eq!(RejectionClass::from_reject(&reject), expected, "{reject:?}");
        }
    }

    #[test]
    fn authority_runtime_metrics_record_when_enabled() {
        let _ = ckb_metrics::METRICS_SERVICE_ENABLED.set(true);
        let metrics = ckb_metrics::handle().expect("metrics enabled in isolated nextest process");
        let before_waits = metrics.ckb_tx_pool_reorg_gate_waits.get();
        record_reorg_gate_wait();
        assert_eq!(metrics.ckb_tx_pool_reorg_gate_waits.get(), before_waits + 1);

        record_authority_write(
            std::time::Duration::from_micros(25),
            std::time::Duration::from_micros(120),
            42,
        );
        assert!(
            metrics
                .ckb_tx_pool_authority_write_wait_seconds
                .get_sample_count()
                >= 1
        );
        assert!(
            metrics
                .ckb_tx_pool_authority_write_hold_seconds
                .get_sample_count()
                >= 1
        );
        assert_eq!(
            metrics
                .ckb_tx_pool_authority_runtime
                .descendant_index_entries
                .get(),
            42
        );

        record_direct_gate_usage(7, 2);
        assert_eq!(
            metrics.ckb_tx_pool_authority_runtime.direct_inflight.get(),
            7
        );
        assert_eq!(
            metrics
                .ckb_tx_pool_authority_runtime
                .callback_read_inflight
                .get(),
            2
        );
    }
}
