//! Independent executable reference model for the tx-pool proof contract.
//!
//! This module deliberately imports no production authority, planner, worker,
//! or protocol type. Shared names describe public semantics only; generated
//! contract checks bind those names to production separately.

mod adversarial;
mod adversarial_properties;
mod atomic_transition;
mod atomic_transition_properties;
mod boundaries;
mod boundary_trace;
mod composition;
mod composition_properties;
mod contract_observation;
mod contract_observation_properties;
mod dependency_progress;
mod dependency_progress_properties;
mod develop_refinement;
mod eviction_properties;
mod eviction_quotient;
mod evidence_transition;
mod evidence_transition_properties;
mod handoff;
mod kernel;
mod permit;
mod progress;
mod progress_properties;
mod properties;
mod proposal;
mod protocol;
mod refinement;
mod resource;
mod resource_transition_properties;
mod scheduler_properties;
mod scheduler_quotient;
mod scheduler_transition;
mod scheduler_transition_properties;
mod settlement_transition;
mod settlement_transition_properties;
mod state;
mod time_context;
mod time_context_properties;
mod topology;
mod topology_properties;
mod trace;
pub(crate) mod two_phase;

pub(crate) use atomic_transition::{
    ClockBranchDecision, ClockCommit, ClockCommitError, ClockDemand, ClockPlan,
    ModelAuthorityClocks,
};

pub(crate) use boundaries::{ModelMinimumFeeObservation, minimum_fee_observation};
pub(crate) use boundary_trace::{
    BoundaryCheckpoint, BoundaryControllerState, BoundaryEffectState, BoundaryEnqueueFailure,
    BoundaryKey, BoundaryLifecycleState, BoundaryPeerId, BoundaryRelaySettlement,
    BoundaryRequestId, BoundarySource, BoundaryTxId, BoundaryWitnessId,
    reference_controller_success_boundary_trace, reference_failed_enqueue_boundary_trace,
    reference_remote_rejection_boundary_trace_with_relay,
};
pub(crate) use contract_observation::{
    bounded_prefix_len, cumulative_effect_region_projection, exact_operational_projection,
    reservation_capacity_is_sufficient,
};
pub(crate) use dependency_progress::{ModelDependencyCut, ModelDependencyKey};
pub(crate) use eviction_quotient::{
    EvictionRefinementInput, EvictionRefinementMetrics, EvictionRefinementObservation,
    EvictionRefinementStatus, eviction_observation, eviction_status_witness,
};
pub(crate) use evidence_transition::{
    ModelAdmissionReceipt, ModelCellLocation, ModelDependencyLevel, ModelEvidenceFrontier,
    ModelEvidenceIdentity, ModelEvidenceProof, ModelEvidenceValidation, ModelEvidenceView,
    ModelFinalAdmissionSubject, ModelKnownDependencies, ModelLocationDependentMetrics,
    ModelMissingDisposition, ModelMissingFact, ModelPoolParent, ModelPreAcceptedSource,
    ModelRawTransaction, ModelReadyOwner, ModelReadyPayloadRelation, ModelReleasedInputContext,
    ModelReleasedInputCut, ModelReleasedInputDisposition, ModelSubjectValidation,
    ModelUnindexedDependencyLevel, location_refresh_observation, missing_resolution_disposition,
    released_input_disposition, validate_direct_acceptance, validate_final_acceptance,
    validate_final_subject, validated_location_transition,
};
pub(crate) use progress::{
    AuthorityProgressCut, EffectHead, EffectLogCut, EffectPublicationObservation,
    EffectReceiptSource, EffectUsageCut, ProgressVersion, SchedulerProgressCut, WakeObservation,
};
pub(crate) use proposal::ProposalStatusReceipt;
pub(crate) use refinement::{
    CellRole, EffectPressure, EvidenceOriginRole, FrontierObservation, FrontierTerminal,
    REFINEMENT_MAX_READY, SourceRole, accepted_capacity_observation, accepted_role_observation,
    candidate_graph_observation, candidate_role_observation, evidence_origin_observation,
    positioned_role_observation, ready_order_observation, shared_header_observation,
    source_observation, source_pressure_observation, stale_observation,
};
pub(crate) use resource::{
    ContinuousAcceptedResources, ContinuousChargeRecord, ContinuousComputeLimits,
    ContinuousResourceChange, ContinuousResourceConfigError, ContinuousResourceLedger,
    ContinuousResourceLimits, ContinuousResourceUsage, ContinuousResourceVector, ModelComputeGrant,
};
pub(crate) use scheduler_quotient::{
    SchedulerRefinementAssignment, SchedulerRefinementCapability, SchedulerRefinementCursors,
    SchedulerRefinementEntry, SchedulerRefinementObservation, SchedulerRefinementOwner,
    SchedulerRefinementPermit, SchedulerRefinementSource, SchedulerRefinementStage,
    SchedulerRefinementVerifyClass, SchedulerRefinementVerifyOrder, SchedulerRefinementWorker,
    SchedulerRefinementWorkerRole, scheduler_wave_observation,
};
pub(crate) use scheduler_transition::{
    SchedulerOwnerPopulation, SchedulerOwnerRing, SchedulerProjectionChange, SchedulerSetProjection,
};
pub(crate) use settlement_transition::{
    ModelMissingSettlement, ModelPayloadPolicy, ModelPayloadPolicyEvolution, ModelSettlementCut,
    ModelSettlementEvidence, ModelSettlementFault, ModelSettlementNext, ModelSettlementObservation,
    ModelSettlementOrigin, ModelSettlementRejection,
};
pub(crate) use state::{
    AcceptedStatus, ModelFeeRate, ModelTransactionCost, VerifyCycleClass as ModelVerifyCycleClass,
};
pub(crate) use time_context::{
    ModelAcceptedValidity, ModelPoolPhase, ModelProposalTimeRelation, model_accepted_validity,
    model_phase_owned_environment_observation, model_proposal_time_observation,
};
pub(crate) use trace::{
    TraceAcceptedProvenance, TraceAcceptedStatus, TraceAction, TraceCut, TraceDisposition,
    TraceEffect, TraceEffectClaim, TraceEffectClass, TraceEffectObservation, TraceLifecycleRoute,
    TraceObservation, TraceOwnerLocation, TraceOwnerObservation, TracePeerId, TraceResourceCounts,
    TraceRetainedPhase, TraceRetainedSource, TraceScenario, TraceTxId, TraceVerifyCapability,
    TraceVerifyClass, TraceWorkLocation, TraceWorkObservation, TraceWorkPermit, TraceWorkStage,
    replay_reference_trace,
};
