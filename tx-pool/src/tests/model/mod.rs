//! Independent executable reference model for the tx-pool proof contract.
//!
//! This module deliberately imports no production authority, planner, worker,
//! or protocol type. Shared names describe public semantics only; generated
//! contract checks bind those names to production separately.

mod adversarial;
mod adversarial_properties;
mod boundaries;
mod boundary_trace;
mod composition;
mod composition_properties;
mod handoff;
mod kernel;
mod permit;
mod properties;
mod protocol;
mod refinement;
mod resource;
mod scheduler_properties;
mod scheduler_quotient;
mod state;
mod trace;

pub(crate) use boundary_trace::{
    BoundaryCheckpoint, BoundaryControllerState, BoundaryEffectState, BoundaryEnqueueFailure,
    BoundaryKey, BoundaryLifecycleState, BoundaryPeerId, BoundaryRelaySettlement,
    BoundaryRequestId, BoundarySource, BoundaryTxId, BoundaryWitnessId,
    reference_controller_success_boundary_trace, reference_failed_enqueue_boundary_trace,
    reference_remote_rejection_boundary_trace_with_relay,
};
pub(crate) use refinement::{
    CellRole, EffectPressure, EvidenceOriginRole, FrontierObservation, FrontierTerminal,
    REFINEMENT_MAX_READY, ReadyOrderInput, SourceRole, accepted_capacity_observation,
    accepted_role_observation, candidate_graph_observation, candidate_role_observation,
    evidence_origin_observation, positioned_role_observation, ready_order_observation,
    shared_header_observation, source_observation, source_pressure_observation, stale_observation,
};
pub(crate) use scheduler_quotient::{
    SchedulerRefinementAssignment, SchedulerRefinementCapability, SchedulerRefinementCursors,
    SchedulerRefinementEntry, SchedulerRefinementObservation, SchedulerRefinementOwner,
    SchedulerRefinementPermit, SchedulerRefinementSource, SchedulerRefinementStage,
    SchedulerRefinementVerifyClass, SchedulerRefinementVerifyOrder, SchedulerRefinementWorker,
    SchedulerRefinementWorkerRole, scheduler_wave_observation,
};
pub(crate) use trace::{
    TraceAcceptedProvenance, TraceAcceptedStatus, TraceAction, TraceCut, TraceDisposition,
    TraceEffect, TraceEffectClaim, TraceEffectClass, TraceEffectObservation, TraceLifecycleRoute,
    TraceObservation, TraceOwnerLocation, TraceOwnerObservation, TracePeerId, TraceResourceCounts,
    TraceRetainedPhase, TraceRetainedSource, TraceScenario, TraceTxId, TraceVerifyCapability,
    TraceVerifyClass, TraceWorkLocation, TraceWorkObservation, TraceWorkPermit, TraceWorkStage,
    replay_reference_trace,
};
