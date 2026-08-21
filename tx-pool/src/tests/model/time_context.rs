//! Minimal quotient of chain-relative transaction validity.
//!
//! Numeric `since`, epoch-fraction and cellbase-maturity decoding remains
//! owned by the consensus verifier.  The tx-pool model needs only the joined
//! lower bound produced by that verifier, the exact primitive facts which make
//! an accepted proof tip-sensitive, and the priority of rules changes over
//! ordinary reorg context changes.

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ModelTimePoint {
    pub(crate) block: u64,
    pub(crate) epoch_tick: u64,
    pub(crate) median_time: u64,
}

impl ModelTimePoint {
    pub(crate) const fn satisfies(self, requirement: ModelTimeRequirement) -> bool {
        self.block >= requirement.min_block
            && self.epoch_tick >= requirement.min_epoch_tick
            && self.median_time >= requirement.min_median_time
    }

    pub(crate) const fn extends(self, earlier: Self) -> bool {
        self.block >= earlier.block
            && self.epoch_tick >= earlier.epoch_tick
            && self.median_time >= earlier.median_time
    }
}

/// Coordinate-wise join of every absolute/relative time and maturity
/// obligation after consensus decoding.  This is a quotient, not a second
/// implementation of the CKB encoding rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ModelTimeRequirement {
    pub(crate) min_block: u64,
    pub(crate) min_epoch_tick: u64,
    pub(crate) min_median_time: u64,
}

impl ModelTimeRequirement {
    pub(crate) const fn join(self, other: Self) -> Self {
        Self {
            min_block: if self.min_block >= other.min_block {
                self.min_block
            } else {
                other.min_block
            },
            min_epoch_tick: if self.min_epoch_tick >= other.min_epoch_tick {
                self.min_epoch_tick
            } else {
                other.min_epoch_tick
            },
            min_median_time: if self.min_median_time >= other.min_median_time {
                self.min_median_time
            } else {
                other.min_median_time
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelContextSensitivity {
    Stable,
    TipContext,
}

impl ModelContextSensitivity {
    pub(crate) const fn requires_reorg_revalidation(self) -> bool {
        matches!(self, Self::TipContext)
    }
}

/// Exact quotient of the production classifier.  `has_nonzero_since` is
/// derived from transaction inputs; `has_chain_cellbase` is derived only from
/// roles consumed by `MaturityVerifier` (inputs and expanded cell deps).
pub(crate) const fn model_context_sensitivity(
    has_nonzero_since: bool,
    has_chain_cellbase: bool,
) -> ModelContextSensitivity {
    if has_nonzero_since || has_chain_cellbase {
        ModelContextSensitivity::TipContext
    } else {
        ModelContextSensitivity::Stable
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelAcceptedValidity {
    Preserved,
    ContextChanged,
    RulesChanged,
}

/// Production derives this sealed receipt in the same priority order.  A
/// rules change dominates a simultaneous detach because every accepted script
/// proof then needs rebuilding, not only the tip-sensitive subset.
pub(crate) const fn model_accepted_validity(
    rules_changed: bool,
    had_detached_chain: bool,
) -> ModelAcceptedValidity {
    if rules_changed {
        ModelAcceptedValidity::RulesChanged
    } else if had_detached_chain {
        ModelAcceptedValidity::ContextChanged
    } else {
        ModelAcceptedValidity::Preserved
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelPoolPhase {
    Pending,
    Gap,
    Proposed,
}

/// Validates the block-number observation produced by the tx-pool
/// `verification_environment` status quotient.
///
/// Production supplies the observed block. The model only checks the phase
/// relation; it is never the expected-value producer for a production test.
/// A Proposed receipt proves that the next block may commit, independently of
/// the exact retained occurrence. Gap lacks that precision and deliberately
/// uses the newest possible occurrence, which is a conservative bound.
pub(crate) fn model_phase_owned_environment_observation(
    phase: ModelPoolPhase,
    tip: u64,
    closest: u64,
    observed_block: u64,
) -> bool {
    let expected = match phase {
        ModelPoolPhase::Pending => tip
            .checked_add(1)
            .and_then(|candidate| candidate.checked_add(closest)),
        ModelPoolPhase::Gap => tip.checked_add(closest),
        ModelPoolPhase::Proposed => tip.checked_add(1),
    };
    expected == Some(observed_block)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelProposalTimeRelation {
    Exact,
    Conservative { excess_blocks: u64 },
    Premature { missing_blocks: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ModelProposalTimeObservation {
    pub(crate) environment_block: u64,
    pub(crate) occurrence_commit_block: u64,
    pub(crate) relation: ModelProposalTimeRelation,
}

/// Relates one concrete proposal occurrence to the status-only verification
/// environment. A legal production observation is exact or conservative;
/// Premature remains in the result algebra as the explicit falsifier.
///
/// Returning `None` means the supplied height cannot have the supplied status
/// under this exact tip/window. `Pending` has no retained occurrence and is
/// therefore outside this relation.
pub(crate) fn model_proposal_time_observation(
    phase: ModelPoolPhase,
    tip: u64,
    closest: u64,
    farthest: u64,
    proposal_height: u64,
    environment_block: u64,
) -> Option<ModelProposalTimeObservation> {
    if closest == 0 || closest > farthest || proposal_height > tip {
        return None;
    }
    let candidate = tip.checked_add(1)?;
    let actual_phase = if candidate <= closest {
        ModelPoolPhase::Gap
    } else {
        let start = candidate.saturating_sub(farthest);
        let end = candidate.checked_sub(closest)?;
        if (start..=end).contains(&proposal_height) {
            ModelPoolPhase::Proposed
        } else if proposal_height > end {
            ModelPoolPhase::Gap
        } else {
            ModelPoolPhase::Pending
        }
    };
    if phase != actual_phase || matches!(phase, ModelPoolPhase::Pending) {
        return None;
    }

    if !model_phase_owned_environment_observation(phase, tip, closest, environment_block) {
        return None;
    }
    let occurrence_commit_block = proposal_height.checked_add(closest)?;
    let relation = match environment_block.cmp(&occurrence_commit_block) {
        std::cmp::Ordering::Equal => ModelProposalTimeRelation::Exact,
        std::cmp::Ordering::Greater => ModelProposalTimeRelation::Conservative {
            excess_blocks: environment_block - occurrence_commit_block,
        },
        std::cmp::Ordering::Less => ModelProposalTimeRelation::Premature {
            missing_blocks: occurrence_commit_block - environment_block,
        },
    };
    Some(ModelProposalTimeObservation {
        environment_block,
        occurrence_commit_block,
        relation,
    })
}
