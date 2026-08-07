//! Independent executable reference model for the tx-pool proof contract.
//!
//! This module deliberately imports no production authority, planner, worker,
//! or protocol type. Shared names describe public semantics only; generated
//! contract checks bind those names to production separately.

mod adversarial;
mod adversarial_properties;
mod boundaries;
mod composition;
mod composition_properties;
mod handoff;
mod kernel;
mod permit;
mod properties;
mod protocol;
mod refinement;
mod resource;
mod state;
mod trace;

pub(crate) use refinement::{
    CellRole, EffectPressure, EvidenceOriginRole, FrontierObservation, FrontierTerminal,
    REFINEMENT_MAX_READY, ReadyOrderInput, SourceRole, accepted_capacity_observation,
    accepted_role_observation, candidate_graph_observation, candidate_role_observation,
    evidence_origin_observation, positioned_role_observation, ready_order_observation,
    shared_header_observation, source_observation, source_pressure_observation, stale_observation,
};
pub(crate) use trace::{
    TraceAcceptedProvenance, TraceAcceptedStatus, TraceAction, TraceCut, TraceDisposition,
    TraceEffect, TraceEffectClaim, TraceEffectClass, TraceEffectObservation, TraceLifecycleRoute,
    TraceObservation, TraceOwnerLocation, TraceOwnerObservation, TracePeerId, TraceResourceCounts,
    TraceRetainedPhase, TraceRetainedSource, TraceScenario, TraceTxId, TraceVerifyCapability,
    TraceVerifyClass, TraceWorkLocation, TraceWorkObservation, TraceWorkPermit, TraceWorkStage,
    replay_reference_trace,
};
