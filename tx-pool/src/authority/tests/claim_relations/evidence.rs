//! Exact evidence-cut and dependency-publication reference relations.
//!
//! These relations describe facts read from one authority cut. They do not
//! retain production payloads or become another evidence/publication owner.

use super::dependency::{ClaimDependencyCut, ClaimDependencyKey};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) type ClaimKnownDependencies = BTreeSet<ClaimDependencyKey>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ClaimEvidenceView(pub(crate) u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ClaimRawTransaction(pub(crate) u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ClaimEvidenceIdentity {
    pub(crate) raw: ClaimRawTransaction,
    pub(crate) witness: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ClaimDependencyLevel {
    pub(crate) last_change: ClaimDependencyCut,
    pub(crate) last_definitive_loss: Option<ClaimDependencyCut>,
}

impl ClaimDependencyLevel {
    pub(crate) fn new(
        last_change: ClaimDependencyCut,
        last_definitive_loss: Option<ClaimDependencyCut>,
    ) -> Option<Self> {
        last_definitive_loss
            .is_none_or(|loss| loss <= last_change)
            .then_some(Self {
                last_change,
                last_definitive_loss,
            })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct ClaimUnindexedDependencyLevel {
    pub(crate) last_change: Option<ClaimDependencyCut>,
    pub(crate) last_definitive_loss: Option<ClaimDependencyCut>,
}

impl ClaimUnindexedDependencyLevel {
    pub(crate) fn new(
        last_change: Option<ClaimDependencyCut>,
        last_definitive_loss: Option<ClaimDependencyCut>,
    ) -> Option<Self> {
        match (last_change, last_definitive_loss) {
            (None, Some(_)) => None,
            (Some(change), Some(loss)) if loss > change => None,
            _ => Some(Self {
                last_change,
                last_definitive_loss,
            }),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct ClaimEvidenceFrontier {
    levels: BTreeMap<ClaimDependencyKey, ClaimDependencyLevel>,
    unindexed: ClaimUnindexedDependencyLevel,
}

impl ClaimEvidenceFrontier {
    pub(crate) fn new(
        levels: impl IntoIterator<Item = (ClaimDependencyKey, ClaimDependencyLevel)>,
        unindexed: ClaimUnindexedDependencyLevel,
    ) -> Option<Self> {
        let mut collected = BTreeMap::new();
        for (key, level) in levels {
            if collected.insert(key, level).is_some() {
                return None;
            }
        }
        Some(Self {
            levels: collected,
            unindexed,
        })
    }

    pub(crate) fn proof_is_current(
        &self,
        dependencies: &ClaimKnownDependencies,
        cut: ClaimDependencyCut,
    ) -> bool {
        dependencies.iter().all(|key| {
            self.levels
                .get(key)
                .and_then(|level| level.last_definitive_loss)
                .is_none_or(|loss| loss <= cut)
        })
    }

    pub(crate) fn owner_free_proof_is_current(
        &self,
        dependencies: &ClaimKnownDependencies,
        cut: ClaimDependencyCut,
    ) -> bool {
        self.proof_is_current(dependencies, cut)
            && (dependencies.is_empty()
                || self
                    .unindexed
                    .last_definitive_loss
                    .is_none_or(|loss| loss <= cut))
    }

    pub(crate) fn resolution_is_current(
        &self,
        baseline: &ClaimKnownDependencies,
        resolved: &ClaimKnownDependencies,
        cut: ClaimDependencyCut,
    ) -> bool {
        self.proof_is_current(resolved, cut)
            && (resolved.is_subset(baseline)
                || self
                    .unindexed
                    .last_definitive_loss
                    .is_none_or(|loss| loss <= cut))
    }

    pub(crate) fn missing_result_is_current(
        &self,
        baseline: &ClaimKnownDependencies,
        resolved: &ClaimKnownDependencies,
        missing: &ClaimKnownDependencies,
        cut: ClaimDependencyCut,
    ) -> bool {
        self.resolution_is_current(baseline, resolved, cut)
            && self.missing_observation_is_current(baseline, missing, cut)
    }

    pub(crate) fn missing_observation_is_current(
        &self,
        baseline: &ClaimKnownDependencies,
        missing: &ClaimKnownDependencies,
        cut: ClaimDependencyCut,
    ) -> bool {
        self.proof_is_current(baseline, cut)
            && missing.iter().all(|key| {
                self.levels.get(key).is_none_or(|level| {
                    level.last_change <= cut
                        && level.last_definitive_loss.is_none_or(|loss| loss <= cut)
                })
            })
            && (missing.is_subset(baseline)
                || self
                    .unindexed
                    .last_change
                    .is_none_or(|change| change <= cut))
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClaimEvidenceProof {
    pub(crate) view: ClaimEvidenceView,
    pub(crate) identity: ClaimEvidenceIdentity,
    pub(crate) dependencies: ClaimKnownDependencies,
    pub(crate) dependency_cut: ClaimDependencyCut,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClaimAdmissionReceipt {
    pub(crate) proof: ClaimEvidenceProof,
}

impl ClaimAdmissionReceipt {
    pub(crate) fn view(&self) -> ClaimEvidenceView {
        self.proof.view
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClaimEvidenceValidation {
    Current,
    StaleChain,
    StaleDependency,
    StructuralFault,
}

pub(crate) fn validate_final_acceptance(
    authority_view: ClaimEvidenceView,
    owner_identity: ClaimEvidenceIdentity,
    frontier: &ClaimEvidenceFrontier,
    receipt: &ClaimAdmissionReceipt,
) -> ClaimEvidenceValidation {
    if receipt.view() != authority_view {
        return ClaimEvidenceValidation::StaleChain;
    }
    if receipt.proof.identity != owner_identity {
        return ClaimEvidenceValidation::StructuralFault;
    }
    if !frontier.proof_is_current(&receipt.proof.dependencies, receipt.proof.dependency_cut) {
        return ClaimEvidenceValidation::StaleDependency;
    }
    ClaimEvidenceValidation::Current
}

pub(crate) fn validate_direct_acceptance(
    authority_view: ClaimEvidenceView,
    frontier: &ClaimEvidenceFrontier,
    receipt: &ClaimAdmissionReceipt,
) -> ClaimEvidenceValidation {
    if receipt.view() != authority_view {
        return ClaimEvidenceValidation::StaleChain;
    }
    if !frontier
        .owner_free_proof_is_current(&receipt.proof.dependencies, receipt.proof.dependency_cut)
    {
        return ClaimEvidenceValidation::StaleDependency;
    }
    ClaimEvidenceValidation::Current
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClaimReadyOwner {
    pub(crate) version: u8,
    pub(crate) ready: bool,
    pub(crate) dependencies: ClaimKnownDependencies,
    pub(crate) dependency_cut: ClaimDependencyCut,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ClaimFinalAdmissionSubject {
    pub(crate) view: ClaimEvidenceView,
    pub(crate) key: ClaimRawTransaction,
    pub(crate) version: u8,
    pub(crate) dependency_cut: ClaimDependencyCut,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClaimSubjectValidation {
    Current,
    StaleChain,
    Missing,
    StaleVersion,
    StalePhase,
    StaleDependency,
}

pub(crate) fn validate_final_subject(
    authority_view: ClaimEvidenceView,
    owners: &BTreeMap<ClaimRawTransaction, ClaimReadyOwner>,
    frontier: &ClaimEvidenceFrontier,
    subject: ClaimFinalAdmissionSubject,
) -> ClaimSubjectValidation {
    if subject.view != authority_view {
        return ClaimSubjectValidation::StaleChain;
    }
    let Some(owner) = owners.get(&subject.key) else {
        return ClaimSubjectValidation::Missing;
    };
    if owner.version != subject.version {
        return ClaimSubjectValidation::StaleVersion;
    }
    if !owner.ready {
        return ClaimSubjectValidation::StalePhase;
    }
    if owner.dependency_cut != subject.dependency_cut
        || !frontier.proof_is_current(&owner.dependencies, subject.dependency_cut)
    {
        return ClaimSubjectValidation::StaleDependency;
    }
    ClaimSubjectValidation::Current
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClaimPreAcceptedSource {
    Remote,
    Proposal,
    Recovery,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ClaimMissingFact {
    Cell {
        key: ClaimDependencyKey,
        parent_is_preaccepted: bool,
    },
    Header {
        key: ClaimDependencyKey,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClaimMissingDisposition {
    Wait,
    RejectUnknownCell(ClaimDependencyKey),
    RejectInvalidHeader(ClaimDependencyKey),
}

pub(crate) fn missing_resolution_disposition(
    source: ClaimPreAcceptedSource,
    missing: &BTreeSet<ClaimMissingFact>,
) -> ClaimMissingDisposition {
    if source == ClaimPreAcceptedSource::Remote {
        return ClaimMissingDisposition::Wait;
    }
    for fact in missing {
        match fact {
            ClaimMissingFact::Cell {
                key,
                parent_is_preaccepted: false,
            } => return ClaimMissingDisposition::RejectUnknownCell(*key),
            ClaimMissingFact::Header { key } => {
                return ClaimMissingDisposition::RejectInvalidHeader(*key);
            }
            ClaimMissingFact::Cell {
                parent_is_preaccepted: true,
                ..
            } => {}
        }
    }
    ClaimMissingDisposition::Wait
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClaimPoolParent {
    Removed,
    SurvivingAccepted { output_count: usize },
    Other,
}

/// A pool-output reference that a legal Accepted membership proof may carry.
/// Construction is the strict output-domain predicate used by resolution and
/// final membership validation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ClaimAcceptedPoolOutput {
    output_index: usize,
    output_count: usize,
}

impl ClaimAcceptedPoolOutput {
    pub(crate) const fn new(output_index: usize, output_count: usize) -> Option<Self> {
        if output_index < output_count {
            Some(Self {
                output_index,
                output_count,
            })
        } else {
            None
        }
    }
}

impl ClaimPoolParent {
    const fn preserves(self, output_index: usize) -> bool {
        match self {
            Self::SurvivingAccepted { output_count } => {
                ClaimAcceptedPoolOutput::new(output_index, output_count).is_some()
            }
            Self::Removed | Self::Other => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClaimReleasedInputCut {
    pub(crate) context: ClaimReleasedInputContext,
    pub(crate) current_spender: Option<ClaimRawTransaction>,
    pub(crate) removed: BTreeSet<ClaimRawTransaction>,
    pub(crate) chain_backed: bool,
    pub(crate) parent: ClaimPoolParent,
    pub(crate) output_index: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClaimReleasedInputContext {
    Replacement { candidate_uses_input: bool },
    Administrative { victim: ClaimRawTransaction },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClaimReleasedInputDisposition {
    Released,
    Retained,
    StructuralFault,
}

pub(crate) fn released_input_disposition(
    cut: &ClaimReleasedInputCut,
) -> ClaimReleasedInputDisposition {
    if matches!(
        cut.context,
        ClaimReleasedInputContext::Replacement {
            candidate_uses_input: true
        }
    ) {
        return ClaimReleasedInputDisposition::Retained;
    }
    let Some(spender) = cut.current_spender else {
        return ClaimReleasedInputDisposition::StructuralFault;
    };
    match cut.context {
        ClaimReleasedInputContext::Replacement { .. } => {
            if !cut.removed.contains(&spender) {
                return ClaimReleasedInputDisposition::Retained;
            }
        }
        ClaimReleasedInputContext::Administrative { victim } => {
            if spender != victim {
                return ClaimReleasedInputDisposition::StructuralFault;
            }
        }
    }
    if cut.chain_backed || cut.parent.preserves(cut.output_index) {
        ClaimReleasedInputDisposition::Released
    } else {
        ClaimReleasedInputDisposition::Retained
    }
}
