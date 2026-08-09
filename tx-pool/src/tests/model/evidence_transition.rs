//! Exact evidence-cut and dependency-publication reference relations.
//!
//! These relations describe facts read from one authority cut. They do not
//! retain production payloads or become another evidence/publication owner.

use super::dependency_progress::{ModelDependencyCut, ModelDependencyKey};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) type ModelKnownDependencies = BTreeSet<ModelDependencyKey>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ModelEvidenceView(pub(crate) u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ModelRawTransaction(pub(crate) u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ModelEvidenceIdentity {
    pub(crate) raw: ModelRawTransaction,
    pub(crate) witness: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ModelDependencyLevel {
    pub(crate) last_change: ModelDependencyCut,
    pub(crate) last_definitive_loss: Option<ModelDependencyCut>,
}

impl ModelDependencyLevel {
    pub(crate) fn new(
        last_change: ModelDependencyCut,
        last_definitive_loss: Option<ModelDependencyCut>,
    ) -> Option<Self> {
        last_definitive_loss
            .is_none_or(|loss| loss <= last_change)
            .then_some(Self {
                last_change,
                last_definitive_loss,
            })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ModelUnindexedDependencyLevel {
    pub(crate) last_change: Option<ModelDependencyCut>,
    pub(crate) last_definitive_loss: Option<ModelDependencyCut>,
}

impl ModelUnindexedDependencyLevel {
    pub(crate) fn new(
        last_change: Option<ModelDependencyCut>,
        last_definitive_loss: Option<ModelDependencyCut>,
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ModelEvidenceFrontier {
    levels: BTreeMap<ModelDependencyKey, ModelDependencyLevel>,
    unindexed: ModelUnindexedDependencyLevel,
}

impl ModelEvidenceFrontier {
    pub(crate) fn new(
        levels: impl IntoIterator<Item = (ModelDependencyKey, ModelDependencyLevel)>,
        unindexed: ModelUnindexedDependencyLevel,
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
        dependencies: &ModelKnownDependencies,
        cut: ModelDependencyCut,
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
        dependencies: &ModelKnownDependencies,
        cut: ModelDependencyCut,
    ) -> bool {
        self.proof_is_current(dependencies, cut)
            && self
                .unindexed
                .last_definitive_loss
                .is_none_or(|loss| loss <= cut)
    }

    pub(crate) fn resolution_is_current(
        &self,
        baseline: &ModelKnownDependencies,
        resolved: &ModelKnownDependencies,
        cut: ModelDependencyCut,
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
        baseline: &ModelKnownDependencies,
        resolved: &ModelKnownDependencies,
        missing: &ModelKnownDependencies,
        cut: ModelDependencyCut,
    ) -> bool {
        self.resolution_is_current(baseline, resolved, cut)
            && self.missing_observation_is_current(baseline, missing, cut)
    }

    pub(crate) fn missing_observation_is_current(
        &self,
        baseline: &ModelKnownDependencies,
        missing: &ModelKnownDependencies,
        cut: ModelDependencyCut,
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
pub(crate) struct ModelEvidenceProof {
    pub(crate) view: ModelEvidenceView,
    pub(crate) identity: ModelEvidenceIdentity,
    pub(crate) dependencies: ModelKnownDependencies,
    pub(crate) dependency_cut: ModelDependencyCut,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModelAdmissionReceipt {
    pub(crate) view: ModelEvidenceView,
    pub(crate) key: ModelRawTransaction,
    pub(crate) proof: ModelEvidenceProof,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelEvidenceValidation {
    Current,
    StaleChain,
    StaleDependency,
    StructuralFault,
}

pub(crate) fn validate_final_acceptance(
    authority_view: ModelEvidenceView,
    owner_identity: ModelEvidenceIdentity,
    frontier: &ModelEvidenceFrontier,
    receipt: &ModelAdmissionReceipt,
) -> ModelEvidenceValidation {
    if receipt.view != authority_view {
        return ModelEvidenceValidation::StaleChain;
    }
    if receipt.key != owner_identity.raw
        || receipt.proof.identity != owner_identity
        || receipt.proof.view != authority_view
    {
        return ModelEvidenceValidation::StructuralFault;
    }
    if !frontier.proof_is_current(&receipt.proof.dependencies, receipt.proof.dependency_cut) {
        return ModelEvidenceValidation::StaleDependency;
    }
    ModelEvidenceValidation::Current
}

pub(crate) fn validate_direct_acceptance(
    authority_view: ModelEvidenceView,
    frontier: &ModelEvidenceFrontier,
    receipt: &ModelAdmissionReceipt,
) -> ModelEvidenceValidation {
    if receipt.view != authority_view {
        return ModelEvidenceValidation::StaleChain;
    }
    if receipt.key != receipt.proof.identity.raw || receipt.proof.view != authority_view {
        return ModelEvidenceValidation::StructuralFault;
    }
    if !frontier
        .owner_free_proof_is_current(&receipt.proof.dependencies, receipt.proof.dependency_cut)
    {
        return ModelEvidenceValidation::StaleDependency;
    }
    ModelEvidenceValidation::Current
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModelReadyOwner {
    pub(crate) version: u8,
    pub(crate) ready: bool,
    pub(crate) dependencies: ModelKnownDependencies,
    pub(crate) dependency_cut: ModelDependencyCut,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ModelFinalAdmissionSubject {
    pub(crate) view: ModelEvidenceView,
    pub(crate) key: ModelRawTransaction,
    pub(crate) version: u8,
    pub(crate) dependency_cut: ModelDependencyCut,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelSubjectValidation {
    Current,
    StaleChain,
    Missing,
    StaleVersion,
    StalePhase,
    StaleDependency,
}

pub(crate) fn validate_final_subject(
    authority_view: ModelEvidenceView,
    owners: &BTreeMap<ModelRawTransaction, ModelReadyOwner>,
    frontier: &ModelEvidenceFrontier,
    subject: ModelFinalAdmissionSubject,
) -> ModelSubjectValidation {
    if subject.view != authority_view {
        return ModelSubjectValidation::StaleChain;
    }
    let Some(owner) = owners.get(&subject.key) else {
        return ModelSubjectValidation::Missing;
    };
    if owner.version != subject.version {
        return ModelSubjectValidation::StaleVersion;
    }
    if !owner.ready {
        return ModelSubjectValidation::StalePhase;
    }
    if owner.dependency_cut != subject.dependency_cut
        || !frontier.proof_is_current(&owner.dependencies, subject.dependency_cut)
    {
        return ModelSubjectValidation::StaleDependency;
    }
    ModelSubjectValidation::Current
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelDirectRejectionValidity {
    Stable,
    AcceptedCut {
        view: ModelEvidenceView,
        accepted_source: u8,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelDirectRejectionObservation {
    Current,
    StaleChain,
    StaleSource,
}

pub(crate) fn validate_direct_rejection(
    authority_view: ModelEvidenceView,
    accepted_source: u8,
    validity: ModelDirectRejectionValidity,
) -> ModelDirectRejectionObservation {
    match validity {
        ModelDirectRejectionValidity::Stable => ModelDirectRejectionObservation::Current,
        ModelDirectRejectionValidity::AcceptedCut {
            view,
            accepted_source: observed,
        } => {
            if view != authority_view {
                ModelDirectRejectionObservation::StaleChain
            } else if observed != accepted_source {
                ModelDirectRejectionObservation::StaleSource
            } else {
                ModelDirectRejectionObservation::Current
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelPreAcceptedSource {
    Remote,
    Proposal,
    Recovery,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ModelMissingFact {
    Cell {
        key: ModelDependencyKey,
        parent_is_preaccepted: bool,
    },
    Header {
        key: ModelDependencyKey,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelMissingDisposition {
    Wait,
    RejectUnknownCell(ModelDependencyKey),
    RejectInvalidHeader(ModelDependencyKey),
}

pub(crate) fn missing_resolution_disposition(
    source: ModelPreAcceptedSource,
    missing: &BTreeSet<ModelMissingFact>,
) -> ModelMissingDisposition {
    if source == ModelPreAcceptedSource::Remote {
        return ModelMissingDisposition::Wait;
    }
    for fact in missing {
        match fact {
            ModelMissingFact::Cell {
                key,
                parent_is_preaccepted: false,
            } => return ModelMissingDisposition::RejectUnknownCell(*key),
            ModelMissingFact::Header { key } => {
                return ModelMissingDisposition::RejectInvalidHeader(*key);
            }
            ModelMissingFact::Cell {
                parent_is_preaccepted: true,
                ..
            } => {}
        }
    }
    ModelMissingDisposition::Wait
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelPoolParent {
    Removed,
    SurvivingAccepted { output_count: usize },
    Other,
}

impl ModelPoolParent {
    const fn preserves(self, output_index: usize) -> bool {
        matches!(
            self,
            Self::SurvivingAccepted { output_count } if output_index < output_count
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModelReleasedInputCut {
    pub(crate) context: ModelReleasedInputContext,
    pub(crate) current_spender: Option<ModelRawTransaction>,
    pub(crate) removed: BTreeSet<ModelRawTransaction>,
    pub(crate) chain_backed: bool,
    pub(crate) parent: ModelPoolParent,
    pub(crate) output_index: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelReleasedInputContext {
    Replacement { candidate_uses_input: bool },
    Administrative { victim: ModelRawTransaction },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelReleasedInputDisposition {
    Released,
    Retained,
    StructuralFault,
}

pub(crate) fn released_input_disposition(
    cut: &ModelReleasedInputCut,
) -> ModelReleasedInputDisposition {
    if matches!(
        cut.context,
        ModelReleasedInputContext::Replacement {
            candidate_uses_input: true
        }
    ) {
        return ModelReleasedInputDisposition::Retained;
    }
    let Some(spender) = cut.current_spender else {
        return ModelReleasedInputDisposition::StructuralFault;
    };
    match cut.context {
        ModelReleasedInputContext::Replacement { .. } => {
            if !cut.removed.contains(&spender) {
                return ModelReleasedInputDisposition::Retained;
            }
        }
        ModelReleasedInputContext::Administrative { victim } => {
            if spender != victim {
                return ModelReleasedInputDisposition::StructuralFault;
            }
        }
    }
    if cut.chain_backed || cut.parent.preserves(cut.output_index) {
        ModelReleasedInputDisposition::Released
    } else {
        ModelReleasedInputDisposition::Retained
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelReplacementReference {
    Input { candidate_uses_input: bool },
    CellDependency,
}

pub(crate) const fn replacement_history_trigger(
    reference: ModelReplacementReference,
    producer_removed: bool,
    chain_backed: bool,
) -> bool {
    match reference {
        ModelReplacementReference::Input {
            candidate_uses_input,
        } => candidate_uses_input || (producer_removed && !chain_backed),
        ModelReplacementReference::CellDependency => producer_removed && !chain_backed,
    }
}
