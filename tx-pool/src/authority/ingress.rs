//! Sealed construction of retained tx-pool admission evidence.
//!
//! Remote and Proposal callers provide only protocol facts. This module owns
//! non-contextual validation, compact payload materialization, the historical
//! Remote residency deadline, and construction of the private source value.
//! A caller therefore cannot stamp a deadline, trust class, lease, generation,
//! or dependency charge into authority state.

use super::{
    plan::{AuthorityFault, Backpressure, PlanError},
    rejection::{CommittedPublicReject, DirectTransactionRejection},
    runtime::AuthorityRuntime,
    state::{
        AdmissionValidationError, PreAcceptedSource, ProposalBase, RemoteBase, RemoteDeadline,
        RemoteResidencyLease, ValidatedAdmission,
    },
};
use crate::util::non_contextual_verify;
use ckb_chain_spec::consensus::{Consensus, MAX_BLOCK_INTERVAL};
use ckb_network::PeerIndex;
use ckb_types::core::{Cycle, TransactionView};
use std::sync::Arc;

const REMOTE_RESIDENCY_BLOCKS: u64 = 100;

/// Construction proof private to this module. `ValidatedAdmission` accepts
/// this capability instead of raw source fields, so sibling modules cannot
/// manufacture retained ingress evidence.
pub(super) struct RetainedIngressSeal(());

/// Move-only retained-ingress capability. Only this module can construct the
/// wrapper, so the authority planner cannot accidentally admit Recovery or a
/// caller-authored source through the external Remote/Proposal boundary.
#[derive(Debug)]
pub(super) struct RetainedIngress {
    kind: RetainedIngressKind,
    admission: ValidatedAdmission,
}

/// Non-contextually validated direct transaction. Local and TestAccept may
/// share the later owner-free computation, but only this ingress module can
/// construct the capability from caller-controlled bytes.
#[derive(Debug)]
pub(super) struct DirectTransaction {
    tx: Arc<TransactionView>,
    command: DirectCommand,
}

impl DirectTransaction {
    pub(super) fn into_parts(self) -> (Arc<TransactionView>, DirectCommand) {
        (self.tx, self.command)
    }
}

/// Closed synchronous command semantics carried by owner-free validation.
/// It is not resident pool state: it only prevents a later caller from
/// choosing mutation after computation has already started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DirectCommand {
    Local,
    TestAccept,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RetainedIngressKind {
    Remote(PeerIndex),
    Proposal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RetainedIngressCommit {
    Retained,
    AcceptedDuplicate,
    RemoteReleased,
    ProposalUnchanged,
    Rejected,
}

/// Exact proof returned only after a retained/no-owner rejection and its
/// public effect commit in one Apply. Narrow callers cannot observe unrelated
/// ingress dispositions or manufacture an impossible service mismatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct IngressRejectionCommit;

impl RetainedIngress {
    pub(super) fn into_parts(self) -> (RetainedIngressKind, ValidatedAdmission) {
        (self.kind, self.admission)
    }
}

#[derive(Debug)]
pub(super) enum RetainedIngressError {
    Rejected(RetainedIngressRejection),
    Admission(AdmissionValidationError),
}

#[derive(Debug)]
pub(super) struct RetainedIngressRejection {
    kind: RetainedIngressKind,
    tx: Arc<TransactionView>,
    reason: CommittedPublicReject,
}

impl RetainedIngressRejection {
    pub(super) fn into_parts(
        self,
    ) -> (
        RetainedIngressKind,
        Arc<TransactionView>,
        CommittedPublicReject,
    ) {
        (self.kind, self.tx, self.reason)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RetainedIngressBackpressure {
    TotalResources,
    RemoteResources,
    PeerResources,
    ComputeResources,
    EffectCapacity,
    ProposalCollision,
}

/// Terminal no-owner pressure at the Remote service boundary. This closed
/// domain is compiled here because only ingress owns the peer audience and
/// transaction payload needed for an exact relay release. Effect-capacity
/// pressure is deliberately absent: it waits and replans instead of being
/// misreported as pool pressure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RemoteIngressPressure {
    TotalResources,
    RemoteResources,
    PeerResources,
    ComputeResources,
    ProposalCollision,
    Allocation,
}

/// Closed service-boundary result for retained Remote and Proposal ingress.
///
/// The open planner error family is intentionally consumed here. Legal peer
/// policy, bounded capacity and allocator pressure remain local outcomes;
/// only a contradiction in the already-sealed authority projection is a
/// structural fault. This prevents the production boundary from inventing a
/// fail-stop policy for a later `PlanError` variant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum RetainedIngressBoundaryError {
    InvalidEvidence,
    ResourceUnavailable,
    Backpressure(RetainedIngressBackpressure),
    LifecycleClosed,
    Fault(AuthorityFault),
}

impl RetainedIngressBoundaryError {
    pub(super) fn from_admission(error: AdmissionValidationError) -> Self {
        match error {
            AdmissionValidationError::ResourceAllocation => Self::ResourceUnavailable,
            AdmissionValidationError::EmptyTransaction
            | AdmissionValidationError::ResourceArithmetic => Self::InvalidEvidence,
        }
    }

    pub(super) fn from_plan(error: PlanError) -> Self {
        match error {
            PlanError::Backpressure(Backpressure::TotalResources) => {
                Self::Backpressure(RetainedIngressBackpressure::TotalResources)
            }
            PlanError::Backpressure(Backpressure::RemoteResources) => {
                Self::Backpressure(RetainedIngressBackpressure::RemoteResources)
            }
            PlanError::Backpressure(Backpressure::PeerResources) => {
                Self::Backpressure(RetainedIngressBackpressure::PeerResources)
            }
            PlanError::Backpressure(Backpressure::ComputeResources) => {
                Self::Backpressure(RetainedIngressBackpressure::ComputeResources)
            }
            PlanError::Backpressure(Backpressure::EffectCapacity) => {
                Self::Backpressure(RetainedIngressBackpressure::EffectCapacity)
            }
            PlanError::Backpressure(Backpressure::ProposalCollision) => {
                Self::Backpressure(RetainedIngressBackpressure::ProposalCollision)
            }
            PlanError::Backpressure(Backpressure::Allocation) => Self::ResourceUnavailable,
            PlanError::EffectClosed => Self::LifecycleClosed,
            PlanError::Fault(fault) => Self::Fault(fault),
            PlanError::Backpressure(Backpressure::AcceptedResources) => {
                Self::Fault(AuthorityFault::ResourceProjection)
            }
            PlanError::Backpressure(Backpressure::GenerationReplacement) => {
                Self::Fault(AuthorityFault::SchedulerProjection)
            }
            PlanError::Duplicate
            | PlanError::PayloadVariant
            | PlanError::Membership(_)
            | PlanError::Stale(_) => Self::Fault(AuthorityFault::MembershipProjection),
        }
    }
}

/// Validate and seal one Remote admission using the production wall-clock
/// policy. Time is sampled here, not supplied by the network or dispatcher.
pub(super) fn remote(
    tx: TransactionView,
    declared_cycles: Cycle,
    peer: PeerIndex,
    consensus: &Consensus,
) -> Result<RetainedIngress, RetainedIngressError> {
    remote_at(
        tx,
        declared_cycles,
        peer,
        ckb_systemtime::unix_time().as_secs(),
        consensus,
    )
}

fn remote_at(
    tx: TransactionView,
    declared_cycles: Cycle,
    peer: PeerIndex,
    admitted_at_secs: u64,
    consensus: &Consensus,
) -> Result<RetainedIngress, RetainedIngressError> {
    let tx = Arc::new(tx.into_compact());
    validate_non_contextual(&tx, RetainedIngressKind::Remote(peer), consensus)?;
    let expires_at =
        admitted_at_secs.saturating_add(REMOTE_RESIDENCY_BLOCKS.saturating_mul(MAX_BLOCK_INTERVAL));
    ValidatedAdmission::from_retained_ingress(
        RetainedIngressSeal(()),
        Arc::unwrap_or_clone(tx),
        PreAcceptedSource::Remote(RemoteBase::ingress(
            RemoteResidencyLease::new(peer, RemoteDeadline(expires_at)),
            declared_cycles,
        )),
    )
    .map(|admission| RetainedIngress {
        kind: RetainedIngressKind::Remote(peer),
        admission,
    })
    .map_err(RetainedIngressError::Admission)
}

/// Validate and seal one trusted Proposal admission. Proposal-window
/// placement is derived later from the paired snapshot; there is no caller-
/// supplied context or lease token to retain here.
pub(super) fn proposal(
    tx: TransactionView,
    consensus: &Consensus,
) -> Result<RetainedIngress, RetainedIngressError> {
    let tx = Arc::new(tx.into_compact());
    validate_non_contextual(&tx, RetainedIngressKind::Proposal, consensus)?;
    ValidatedAdmission::from_retained_ingress(
        RetainedIngressSeal(()),
        Arc::unwrap_or_clone(tx),
        PreAcceptedSource::Proposal {
            base: ProposalBase::Trusted,
        },
    )
    .map(|admission| RetainedIngress {
        kind: RetainedIngressKind::Proposal,
        admission,
    })
    .map_err(RetainedIngressError::Admission)
}

/// Validate and compact a synchronous Local/TestAccept transaction before it
/// can enter owner-free resolution. Rejection retains the exact transaction
/// for Local publication without creating an authority owner.
pub(super) fn direct(
    tx: &TransactionView,
    consensus: &Consensus,
    command: DirectCommand,
) -> Result<DirectTransaction, DirectTransactionRejection> {
    let tx = Arc::new(tx.clone().into_compact());
    non_contextual_verify(consensus, &tx)
        .map_err(|reason| DirectTransactionRejection::stable(Arc::clone(&tx), command, reason))?;
    Ok(DirectTransaction { tx, command })
}

fn validate_non_contextual(
    tx: &Arc<TransactionView>,
    kind: RetainedIngressKind,
    consensus: &Consensus,
) -> Result<(), RetainedIngressError> {
    non_contextual_verify(consensus, tx)
        .map_err(CommittedPublicReject::new)
        .map_err(|reason| {
            RetainedIngressError::Rejected(RetainedIngressRejection {
                kind,
                tx: Arc::clone(tx),
                reason,
            })
        })
}

impl AuthorityRuntime {
    pub(super) fn submit_remote_ingress(
        &self,
        tx: TransactionView,
        declared_cycles: Cycle,
        peer: PeerIndex,
    ) -> Result<RetainedIngressCommit, RetainedIngressBoundaryError> {
        let consensus = self.paired_consensus();
        match remote(tx, declared_cycles, peer, &consensus) {
            Ok(ingress) => self
                .commit_retained_ingress(ingress)
                .map_err(RetainedIngressBoundaryError::from_plan),
            Err(RetainedIngressError::Rejected(rejection)) => self
                .commit_retained_ingress_rejection(rejection)
                .map(|_| RetainedIngressCommit::Rejected)
                .map_err(RetainedIngressBoundaryError::from_plan),
            Err(RetainedIngressError::Admission(error)) => {
                Err(RetainedIngressBoundaryError::from_admission(error))
            }
        }
    }

    pub(super) fn submit_proposal_ingress(
        &self,
        tx: TransactionView,
    ) -> Result<RetainedIngressCommit, RetainedIngressBoundaryError> {
        let consensus = self.paired_consensus();
        match proposal(tx, &consensus) {
            Ok(ingress) => self
                .commit_retained_ingress(ingress)
                .map_err(RetainedIngressBoundaryError::from_plan),
            Err(RetainedIngressError::Rejected(rejection)) => self
                .commit_retained_ingress_rejection(rejection)
                .map(|_| RetainedIngressCommit::Rejected)
                .map_err(RetainedIngressBoundaryError::from_plan),
            Err(RetainedIngressError::Admission(error)) => {
                Err(RetainedIngressBoundaryError::from_admission(error))
            }
        }
    }

    /// Publish a terminal Remote no-owner disposition through the same
    /// committed effect authority as every other ingress result. The caller
    /// may retry only effect capacity; it cannot choose a nearby public reason
    /// or bypass the relay/recent-reject policy compiler.
    pub(super) fn reject_remote_ingress_pressure(
        &self,
        tx: TransactionView,
        peer: PeerIndex,
        pressure: RemoteIngressPressure,
    ) -> Result<IngressRejectionCommit, RetainedIngressBoundaryError> {
        let reason = match pressure {
            RemoteIngressPressure::TotalResources => "tx-pool total residency limit reached",
            RemoteIngressPressure::RemoteResources => "tx-pool remote residency limit reached",
            RemoteIngressPressure::PeerResources => "tx-pool per-peer residency limit reached",
            RemoteIngressPressure::ComputeResources => "tx-pool transient compute limit reached",
            RemoteIngressPressure::ProposalCollision => "tx-pool proposal short-id collision",
            RemoteIngressPressure::Allocation => "tx-pool resource allocation unavailable",
        };
        let rejection = RetainedIngressRejection {
            kind: RetainedIngressKind::Remote(peer),
            tx: Arc::new(tx.into_compact()),
            reason: CommittedPublicReject::new(crate::error::Reject::Full(reason.to_owned())),
        };
        self.commit_retained_ingress_rejection(rejection)
            .map_err(RetainedIngressBoundaryError::from_plan)
    }
}

#[cfg(test)]
#[path = "tests/support/ingress.rs"]
pub(in crate::authority) mod test_support;
