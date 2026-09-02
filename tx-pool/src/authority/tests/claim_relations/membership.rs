//! Minimal algebra for named membership, ordering, and eviction properties.

use std::cmp::max;

pub(crate) const REFINEMENT_MAX_READY: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CellRole {
    None,
    Input,
    Read,
    Output,
}

impl CellRole {
    pub(crate) const ALL: [Self; 4] = [Self::None, Self::Input, Self::Read, Self::Output];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FrontierTerminal {
    Complete,
    Coupled,
    Stale,
    DuplicateOutputIdentity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SourceRole {
    Trusted,
    Remote,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EffectPressure {
    RemoteFull,
    OrdinaryFull,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EvidenceOriginRole {
    ChainInput,
    ChainRead,
    PoolInput,
    PoolRead,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FrontierObservation {
    pub(crate) prefix_len: usize,
    pub(crate) terminal: FrontierTerminal,
}

pub(crate) fn candidate_role_observation(left: CellRole, right: CellRole) -> FrontierObservation {
    positioned_role_observation(2, 0, left, 1, right)
}

pub(crate) fn positioned_role_observation(
    owner_count: usize,
    left_index: usize,
    left: CellRole,
    right_index: usize,
    right: CellRole,
) -> FrontierObservation {
    if left_index >= owner_count || right_index >= owner_count || left_index == right_index {
        return FrontierObservation {
            prefix_len: 0,
            terminal: FrontierTerminal::Coupled,
        };
    }
    if left == CellRole::Output && right == CellRole::Output {
        return FrontierObservation {
            prefix_len: 0,
            terminal: FrontierTerminal::DuplicateOutputIdentity,
        };
    }
    if independent_roles(left, right) {
        FrontierObservation {
            prefix_len: owner_count,
            terminal: FrontierTerminal::Complete,
        }
    } else {
        FrontierObservation {
            prefix_len: left_index.max(right_index),
            terminal: FrontierTerminal::Coupled,
        }
    }
}

pub(crate) fn accepted_role_observation(
    candidate: CellRole,
    accepted: CellRole,
) -> FrontierObservation {
    if candidate == CellRole::Output && accepted == CellRole::Output {
        return FrontierObservation {
            prefix_len: 0,
            terminal: FrontierTerminal::DuplicateOutputIdentity,
        };
    }
    let complete = independent_roles(candidate, accepted);
    FrontierObservation {
        prefix_len: usize::from(complete),
        terminal: if complete {
            FrontierTerminal::Complete
        } else {
            FrontierTerminal::Coupled
        },
    }
}

const fn independent_roles(left: CellRole, right: CellRole) -> bool {
    matches!(left, CellRole::None)
        || matches!(right, CellRole::None)
        || matches!((left, right), (CellRole::Read, CellRole::Read))
}

pub(crate) fn candidate_graph_observation(edge_mask: u8) -> FrontierObservation {
    let mut first_coupled_position = None;
    let mut bit = 0u8;
    for left in 0..4 {
        for right in (left + 1)..4 {
            if edge_mask & (1u8 << bit) != 0 {
                first_coupled_position =
                    Some(first_coupled_position.map_or(right, |current: usize| current.min(right)));
            }
            bit += 1;
        }
    }
    first_coupled_position.map_or(
        FrontierObservation {
            prefix_len: 4,
            terminal: FrontierTerminal::Complete,
        },
        |prefix_len| FrontierObservation {
            prefix_len,
            terminal: FrontierTerminal::Coupled,
        },
    )
}

pub(crate) fn source_observation(sources: &[SourceRole]) -> FrontierObservation {
    let trusted = sources
        .iter()
        .filter(|source| **source == SourceRole::Trusted)
        .count();
    let mixed = trusted != 0 && trusted != sources.len();
    FrontierObservation {
        // Ready ordering places every trusted proposal ahead of remote work.
        // A mixed batch therefore reaches the effect-class boundary only
        // after the complete trusted partition, independent of input order.
        prefix_len: if mixed { trusted } else { sources.len() },
        terminal: if mixed {
            FrontierTerminal::Coupled
        } else {
            FrontierTerminal::Complete
        },
    }
}

pub(crate) fn accepted_capacity_observation(
    candidate_count: usize,
    accepted_entries: u16,
) -> FrontierObservation {
    let prefix_len = candidate_count.min(usize::from(accepted_entries));
    FrontierObservation {
        prefix_len,
        terminal: if prefix_len == candidate_count {
            FrontierTerminal::Complete
        } else {
            FrontierTerminal::Coupled
        },
    }
}

pub(crate) fn source_pressure_observation(
    source: SourceRole,
    pressure: EffectPressure,
) -> FrontierObservation {
    let blocked = source == SourceRole::Remote || pressure == EffectPressure::OrdinaryFull;
    FrontierObservation {
        prefix_len: usize::from(!blocked),
        terminal: if blocked {
            FrontierTerminal::Coupled
        } else {
            FrontierTerminal::Complete
        },
    }
}

pub(crate) const fn stale_observation() -> FrontierObservation {
    FrontierObservation {
        prefix_len: 0,
        terminal: FrontierTerminal::Stale,
    }
}

pub(crate) const fn shared_header_observation(owner_count: usize) -> FrontierObservation {
    FrontierObservation {
        prefix_len: owner_count,
        terminal: FrontierTerminal::Complete,
    }
}

pub(crate) const fn evidence_origin_observation(origin: EvidenceOriginRole) -> FrontierObservation {
    match origin {
        EvidenceOriginRole::ChainInput | EvidenceOriginRole::ChainRead => FrontierObservation {
            prefix_len: 1,
            terminal: FrontierTerminal::Complete,
        },
        EvidenceOriginRole::PoolInput | EvidenceOriginRole::PoolRead => FrontierObservation {
            prefix_len: 0,
            terminal: FrontierTerminal::Coupled,
        },
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ClaimFeeRate(u64);

impl ClaimFeeRate {
    pub(crate) const fn from_u64(value: u64) -> Self {
        Self(value)
    }

    pub(crate) const fn as_u64(self) -> u64 {
        self.0
    }

    pub(crate) const fn fee(self, weight: u64) -> u64 {
        self.0.saturating_mul(weight) / 1_000
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClaimMinimumFeeObservation {
    Accepted { actual: u64, required: u64 },
    Rejected { actual: u64, required: u64 },
}

pub(crate) const fn minimum_fee_observation(
    actual: u64,
    weight: u64,
    minimum_rate: ClaimFeeRate,
) -> ClaimMinimumFeeObservation {
    let required = minimum_rate.fee(weight);
    if actual < required {
        ClaimMinimumFeeObservation::Rejected { actual, required }
    } else {
        ClaimMinimumFeeObservation::Accepted { actual, required }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ClaimTransactionCost {
    payload_bytes: u32,
    fee: u64,
    cycles: u64,
}

impl ClaimTransactionCost {
    pub(crate) const fn new(payload_bytes: u32, fee: u64, cycles: u64) -> Option<Self> {
        if payload_bytes.checked_add(4).is_none() {
            return None;
        }
        Some(Self {
            payload_bytes,
            fee,
            cycles,
        })
    }

    pub(crate) const fn serialized_bytes(self) -> u32 {
        self.payload_bytes + 4
    }

    pub(crate) const fn fee(self) -> u64 {
        self.fee
    }

    pub(crate) const fn cycles(self) -> u64 {
        self.cycles
    }
}

pub(crate) fn ready_order_observation(items: &[ClaimTransactionCost]) -> Vec<usize> {
    let mut order = (0..items.len()).collect::<Vec<_>>();
    order.sort_unstable_by(|left, right| {
        let left_cost = items[*left];
        let right_cost = items[*right];
        let left_weight = transaction_weight(EvictionRefinementMetrics::new(
            left_cost.fee,
            u64::from(left_cost.serialized_bytes()),
            left_cost.cycles,
        ));
        let right_weight = transaction_weight(EvictionRefinementMetrics::new(
            right_cost.fee,
            u64::from(right_cost.serialized_bytes()),
            right_cost.cycles,
        ));
        (u128::from(right_cost.fee) * u128::from(left_weight))
            .cmp(&(u128::from(left_cost.fee) * u128::from(right_weight)))
            .then_with(|| left.cmp(right))
    });
    order
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum EvictionRefinementStatus {
    Pending,
    Gap,
    Proposed,
}

pub(crate) const fn eviction_status_witness(
    status: EvictionRefinementStatus,
) -> EvictionRefinementStatus {
    status
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
    pub(crate) status: EvictionRefinementStatus,
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
    const CKB_BYTES_PER_CYCLE: f64 = 0.000_170_571_4_f64;
    max(
        metrics.serialized_bytes,
        (metrics.cycles as f64 * CKB_BYTES_PER_CYCLE) as u64,
    )
}

fn fee_rate(metrics: EvictionRefinementMetrics) -> u64 {
    metrics
        .fee
        .saturating_mul(1_000)
        .checked_div(transaction_weight(metrics))
        .unwrap_or(0)
}

pub(crate) fn eviction_observation(
    input: EvictionRefinementInput,
) -> EvictionRefinementObservation {
    EvictionRefinementObservation {
        status: input.status,
        fee_rate: max(fee_rate(input.own), fee_rate(input.descendants)),
        descendants_count: input.descendants_count,
        arrival: input.arrival,
        identity: input.identity,
    }
}
