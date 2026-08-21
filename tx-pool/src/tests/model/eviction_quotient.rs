//! Pure eviction-policy quotient shared by the executable kernel and the
//! independently constructed production refinement adapter.
//!
//! The decimal is intentionally encoded independently from
//! `ckb_types::core::tx_pool::get_transaction_weight`. A production change to
//! the CKB weight or integer fee-rate policy must therefore fail refinement
//! instead of changing both sides through one helper.

use super::{
    proposal::{ProposalContext, ProposalStatusReceipt},
    state::{AcceptedStatus, ProposalId},
};
use std::cmp::max;
use std::collections::BTreeSet;

const CKB_BYTES_PER_CYCLE: f64 = 0.000_170_571_4_f64;
const KILOWEIGHT: u64 = 1_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum EvictionRefinementStatus {
    Pending,
    Gap,
    Proposed,
}

/// Model witness that a quotient status is realizable by primitive proposal
/// history. Production refinement may call this only through its checked
/// `ProposalContextReceipt` bridge; the default-production history provenance
/// remains independently enforced.
pub(crate) fn eviction_status_witness(status: EvictionRefinementStatus) -> ProposalStatusReceipt {
    let proposal = ProposalId(0);
    let context = match status {
        EvictionRefinementStatus::Pending => ProposalContext::empty(),
        EvictionRefinementStatus::Gap => {
            ProposalContext::status_witness(BTreeSet::new(), BTreeSet::from([proposal]))
                .expect("the fixed gap witness is valid")
        }
        EvictionRefinementStatus::Proposed => {
            ProposalContext::status_witness(BTreeSet::from([proposal]), BTreeSet::new())
                .expect("the fixed proposed witness is valid")
        }
    };
    context.status(proposal)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EvictionRefinementMetrics {
    pub(crate) fee: u64,
    pub(crate) serialized_bytes: u64,
    pub(crate) cycles: u64,
}

impl EvictionRefinementMetrics {
    pub(crate) const fn new(fee: u64, serialized_bytes: u64, cycles: u64) -> Self {
        Self {
            fee,
            serialized_bytes,
            cycles,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EvictionRefinementInput {
    pub(crate) status: ProposalStatusReceipt,
    pub(crate) own: EvictionRefinementMetrics,
    pub(crate) descendants: EvictionRefinementMetrics,
    pub(crate) descendants_count: usize,
    pub(crate) arrival: u128,
    pub(crate) identity: [u8; 32],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct EvictionRefinementObservation {
    pub(crate) status: EvictionRefinementStatus,
    pub(crate) fee_rate: u64,
    pub(crate) descendants_count: usize,
    pub(crate) arrival: u128,
    pub(crate) identity: [u8; 32],
}

pub(crate) fn transaction_weight(metrics: EvictionRefinementMetrics) -> u64 {
    max(
        metrics.serialized_bytes,
        (metrics.cycles as f64 * CKB_BYTES_PER_CYCLE) as u64,
    )
}

fn fee_rate(metrics: EvictionRefinementMetrics) -> u64 {
    let weight = transaction_weight(metrics);
    metrics
        .fee
        .saturating_mul(KILOWEIGHT)
        .checked_div(weight)
        .unwrap_or(0)
}

pub(crate) fn eviction_observation(
    input: EvictionRefinementInput,
) -> EvictionRefinementObservation {
    EvictionRefinementObservation {
        status: match input.status.value() {
            AcceptedStatus::Pending => EvictionRefinementStatus::Pending,
            AcceptedStatus::Gap => EvictionRefinementStatus::Gap,
            AcceptedStatus::Proposed => EvictionRefinementStatus::Proposed,
        },
        fee_rate: max(fee_rate(input.own), fee_rate(input.descendants)),
        descendants_count: input.descendants_count,
        arrival: input.arrival,
        identity: input.identity,
    }
}
