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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
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

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
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

    fn retire_level(&mut self, level: ModelDependencyLevel) {
        self.unindexed.last_change = Some(
            self.unindexed
                .last_change
                .map_or(level.last_change, |current| current.max(level.last_change)),
        );
        if let Some(loss) = level.last_definitive_loss {
            self.unindexed.last_definitive_loss = Some(
                self.unindexed
                    .last_definitive_loss
                    .map_or(loss, |current| current.max(loss)),
            );
        }
    }

    /// Apply one primitive availability/loss cut using the exact current
    /// consumer projection.  Indexed keys retain their precise level;
    /// unindexed keys collapse into the same two conservative maxima used by
    /// production.  Thus late dep-group discovery cannot smuggle an event
    /// past checkout merely because the member was not yet known.
    pub(crate) fn apply_events(
        &mut self,
        available: &BTreeSet<ModelDependencyKey>,
        lost: &BTreeSet<ModelDependencyKey>,
        indexed: &BTreeSet<ModelDependencyKey>,
        cut: ModelDependencyCut,
    ) -> bool {
        if available.iter().any(|key| lost.contains(key)) {
            return false;
        }
        for (key, definitive_loss) in available
            .iter()
            .copied()
            .map(|key| (key, false))
            .chain(lost.iter().copied().map(|key| (key, true)))
        {
            let previous = self.levels.get(&key).copied();
            if previous.is_some_and(|level| level.last_change >= cut) {
                return false;
            }
            let level = ModelDependencyLevel {
                last_change: cut,
                last_definitive_loss: if definitive_loss {
                    Some(cut)
                } else {
                    previous.and_then(|level| level.last_definitive_loss)
                },
            };
            if indexed.contains(&key) {
                self.levels.insert(key, level);
            } else {
                self.retire_level(level);
                if let Some(previous) = self.levels.remove(&key) {
                    self.retire_level(previous);
                }
            }
        }
        true
    }

    /// Dependency levels have no independent lifetime.  When the last owner
    /// edge disappears, fold the level into the bounded unindexed summary so
    /// a later owner observes the same conservative production cut.
    pub(crate) fn prune_to(&mut self, indexed: &BTreeSet<ModelDependencyKey>) {
        let retired = self
            .levels
            .keys()
            .filter(|key| !indexed.contains(key))
            .copied()
            .collect::<Vec<_>>();
        for key in retired {
            if let Some(level) = self.levels.remove(&key) {
                self.retire_level(level);
            }
        }
    }

    pub(crate) fn dependency_cuts(&self) -> BTreeSet<ModelDependencyCut> {
        self.levels
            .values()
            .flat_map(|level| std::iter::once(level.last_change).chain(level.last_definitive_loss))
            .chain(self.unindexed.last_change)
            .chain(self.unindexed.last_definitive_loss)
            .collect()
    }

    pub(crate) fn remap_dependency_cuts(
        &mut self,
        mapping: &BTreeMap<ModelDependencyCut, ModelDependencyCut>,
    ) -> bool {
        fn remap(
            cut: &mut ModelDependencyCut,
            mapping: &BTreeMap<ModelDependencyCut, ModelDependencyCut>,
        ) -> bool {
            if cut.0 == 0 {
                return true;
            }
            let Some(mapped) = mapping.get(cut).copied() else {
                return false;
            };
            *cut = mapped;
            true
        }

        let mut next = self.clone();
        for level in next.levels.values_mut() {
            if !remap(&mut level.last_change, mapping)
                || level
                    .last_definitive_loss
                    .as_mut()
                    .is_some_and(|loss| !remap(loss, mapping))
                || level
                    .last_definitive_loss
                    .is_some_and(|loss| loss > level.last_change)
            {
                return false;
            }
        }
        let unindexed_is_valid = match (
            next.unindexed.last_change,
            next.unindexed.last_definitive_loss,
        ) {
            (None, Some(_)) => false,
            (Some(change), Some(loss)) => loss <= change,
            (None | Some(_), None) => true,
        };
        if next
            .unindexed
            .last_change
            .as_mut()
            .is_some_and(|change| !remap(change, mapping))
            || next
                .unindexed
                .last_definitive_loss
                .as_mut()
                .is_some_and(|loss| !remap(loss, mapping))
            || !unindexed_is_valid
        {
            return false;
        }
        *self = next;
        true
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

    fn all_observed_dependencies_available(
        &self,
        observed: &ModelKnownDependencies,
        cut: ModelDependencyCut,
    ) -> bool {
        observed.iter().all(|key| {
            self.levels.get(key).is_some_and(|level| {
                cut < level.last_change
                    && level
                        .last_definitive_loss
                        .is_none_or(|loss| loss < level.last_change)
            })
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelDependencyMaintenanceScope {
    ExistingWaiters,
    AllConsumers,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ModelPreAcceptedMaintenancePhase {
    QueuedResolve,
    QueuedVerify {
        dependency_cut: ModelDependencyCut,
    },
    Computing,
    Waiting {
        observed: ModelKnownDependencies,
        dependency_cut: ModelDependencyCut,
    },
    Ready {
        dependency_cut: ModelDependencyCut,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ModelDependencyMaintenanceLocation {
    PreAccepted(ModelPreAcceptedMaintenancePhase),
    Accepted {
        dependency_cut: ModelDependencyCut,
    },
    ReplacementHistory {
        observed: ModelKnownDependencies,
        dependency_cut: ModelDependencyCut,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModelDependencyMaintenanceOwner {
    pub(crate) identity_matches: bool,
    pub(crate) dependencies: ModelKnownDependencies,
    pub(crate) location: ModelDependencyMaintenanceLocation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ModelDependencyMaintenanceTicket {
    pub(crate) key: ModelDependencyKey,
    pub(crate) has_owner_edge: bool,
    pub(crate) target: ModelDependencyCut,
    pub(crate) scope: ModelDependencyMaintenanceScope,
    pub(crate) last_definitive_loss: Option<ModelDependencyCut>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelDependencyMaintenanceAction {
    Advance,
    Requeue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelDependencyMaintenanceError {
    Projection,
    SurvivingAcceptedConsumer,
}

/// Total owner decision for one indexed dependency-maintenance ticket.
///
/// The finite-rank relation owns which edge is selected and how its successor
/// decreases. This relation owns the independent semantic question that must
/// be answered before that successor is compiled: whether the selected owner
/// has already observed the ticket's level or must return to Resolve.
pub(crate) fn dependency_maintenance_action(
    frontier: &ModelEvidenceFrontier,
    ticket: ModelDependencyMaintenanceTicket,
    owner: Option<&ModelDependencyMaintenanceOwner>,
) -> Result<ModelDependencyMaintenanceAction, ModelDependencyMaintenanceError> {
    if !ticket.has_owner_edge {
        return Ok(ModelDependencyMaintenanceAction::Advance);
    }
    let owner = owner.ok_or(ModelDependencyMaintenanceError::Projection)?;
    if !owner.identity_matches {
        return Err(ModelDependencyMaintenanceError::Projection);
    }
    match &owner.location {
        ModelDependencyMaintenanceLocation::Accepted { dependency_cut } => {
            if ticket.scope == ModelDependencyMaintenanceScope::AllConsumers
                && ticket
                    .last_definitive_loss
                    .is_some_and(|loss| *dependency_cut < loss)
            {
                return Err(ModelDependencyMaintenanceError::SurvivingAcceptedConsumer);
            }
            Ok(ModelDependencyMaintenanceAction::Advance)
        }
        ModelDependencyMaintenanceLocation::ReplacementHistory {
            observed,
            dependency_cut,
        } => {
            if !owner.dependencies.contains(&ticket.key) {
                return Err(ModelDependencyMaintenanceError::Projection);
            }
            Ok(
                if observed.contains(&ticket.key)
                    && frontier.all_observed_dependencies_available(observed, *dependency_cut)
                {
                    ModelDependencyMaintenanceAction::Requeue
                } else {
                    ModelDependencyMaintenanceAction::Advance
                },
            )
        }
        ModelDependencyMaintenanceLocation::PreAccepted(phase) => {
            if !owner.dependencies.contains(&ticket.key) {
                return Err(ModelDependencyMaintenanceError::Projection);
            }
            let stale = match ticket.scope {
                ModelDependencyMaintenanceScope::ExistingWaiters => match phase {
                    ModelPreAcceptedMaintenancePhase::Waiting {
                        observed,
                        dependency_cut,
                    } => observed.contains(&ticket.key) && *dependency_cut < ticket.target,
                    ModelPreAcceptedMaintenancePhase::QueuedResolve
                    | ModelPreAcceptedMaintenancePhase::QueuedVerify { .. }
                    | ModelPreAcceptedMaintenancePhase::Computing
                    | ModelPreAcceptedMaintenancePhase::Ready { .. } => false,
                },
                ModelDependencyMaintenanceScope::AllConsumers => {
                    let loss = ticket
                        .last_definitive_loss
                        .ok_or(ModelDependencyMaintenanceError::Projection)?;
                    match phase {
                        ModelPreAcceptedMaintenancePhase::QueuedResolve
                        | ModelPreAcceptedMaintenancePhase::Computing => false,
                        ModelPreAcceptedMaintenancePhase::QueuedVerify { dependency_cut }
                        | ModelPreAcceptedMaintenancePhase::Ready { dependency_cut } => {
                            *dependency_cut < loss
                        }
                        ModelPreAcceptedMaintenancePhase::Waiting { dependency_cut, .. } => {
                            *dependency_cut < ticket.target
                        }
                    }
                }
            };
            Ok(if stale {
                ModelDependencyMaintenanceAction::Requeue
            } else {
                ModelDependencyMaintenanceAction::Advance
            })
        }
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
    pub(crate) proof: ModelEvidenceProof,
}

impl ModelAdmissionReceipt {
    pub(crate) fn view(&self) -> ModelEvidenceView {
        self.proof.view
    }

    pub(crate) fn key(&self) -> ModelRawTransaction {
        self.proof.identity.raw
    }
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
    if receipt.view() != authority_view {
        return ModelEvidenceValidation::StaleChain;
    }
    if receipt.proof.identity != owner_identity {
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
    if receipt.view() != authority_view {
        return ModelEvidenceValidation::StaleChain;
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

/// A pool-output reference that a legal Accepted membership proof may carry.
/// Construction is the strict output-domain predicate used by resolution and
/// final membership validation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ModelAcceptedPoolOutput {
    output_index: usize,
    output_count: usize,
}

impl ModelAcceptedPoolOutput {
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

    pub(crate) const fn output_index(self) -> usize {
        self.output_index
    }

    pub(crate) const fn output_count(self) -> usize {
        self.output_count
    }
}

impl ModelPoolParent {
    const fn preserves(self, output_index: usize) -> bool {
        match self {
            Self::SurvivingAccepted { output_count } => {
                ModelAcceptedPoolOutput::new(output_index, output_count).is_some()
            }
            Self::Removed | Self::Other => false,
        }
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

/// The externally relevant location carried by one resolved cell. The chain
/// token is deliberately value-bearing: changing between two chain locations
/// is as observable as changing between chain and pool origin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelCellLocation {
    Pool,
    Chain(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelReadyPayloadRelation {
    Shared,
    LocationRefreshed,
}

/// Location-dependent accounting observed at final admission. These are
/// values, not evidence constructors: the model keeps the current production
/// carry-over observation distinct from a value independently recomputed for
/// the refreshed cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ModelLocationDependentMetrics {
    pub(crate) fee: u64,
    pub(crate) accepted_resident_bytes: usize,
}

/// One pointwise final-validation transition. Payload metadata is consumed by
/// block construction while context provenance is consumed by policy; both
/// must observe the same authoritative location cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ModelValidatedLocationTransition {
    pub(crate) payload_location: ModelCellLocation,
    pub(crate) context_location: ModelCellLocation,
    pub(crate) relation: ModelReadyPayloadRelation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ModelLocationRefreshObservation {
    pub(crate) transition: ModelValidatedLocationTransition,
    pub(crate) previous_metrics: ModelLocationDependentMetrics,
    pub(crate) committed_metrics: ModelLocationDependentMetrics,
    pub(crate) recomputed_metrics: ModelLocationDependentMetrics,
}

impl ModelLocationRefreshObservation {
    /// A location change is one producer cut: the committed fee and resident
    /// charge must equal an independent recomputation from the refreshed
    /// resolved transaction.
    pub(crate) const fn is_atomically_resealed(self) -> bool {
        self.committed_metrics.fee == self.recomputed_metrics.fee
            && self.committed_metrics.accepted_resident_bytes
                == self.recomputed_metrics.accepted_resident_bytes
    }
}

pub(crate) fn validated_location_transition(
    previous: ModelCellLocation,
    authoritative: ModelCellLocation,
) -> ModelValidatedLocationTransition {
    ModelValidatedLocationTransition {
        payload_location: authoritative,
        context_location: authoritative,
        relation: if previous == authoritative {
            ModelReadyPayloadRelation::Shared
        } else {
            ModelReadyPayloadRelation::LocationRefreshed
        },
    }
}

/// Exact refinement observation for the one location-refresh producer cut.
/// Keeping previous, committed and independently recomputed metrics distinct
/// makes stale carry-over an executable falsifier rather than an implicit
/// assumption.
pub(crate) fn location_refresh_observation(
    previous: ModelCellLocation,
    authoritative: ModelCellLocation,
    previous_metrics: ModelLocationDependentMetrics,
    committed_metrics: ModelLocationDependentMetrics,
    recomputed_metrics: ModelLocationDependentMetrics,
) -> ModelLocationRefreshObservation {
    ModelLocationRefreshObservation {
        transition: validated_location_transition(previous, authoritative),
        previous_metrics,
        committed_metrics,
        recomputed_metrics,
    }
}
