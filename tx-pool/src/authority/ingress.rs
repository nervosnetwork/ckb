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
    state::{
        AdmissionValidationError, PreAcceptedSource, ProposalBase, RemoteBase, RemoteDeadline,
        RemoteResidencyLease, ValidatedAdmission,
    },
};
use crate::util::non_contextual_verify;
use ckb_chain_spec::consensus::{Consensus, MAX_BLOCK_INTERVAL};
use ckb_network::PeerIndex;
use ckb_types::core::{Cycle, TransactionView};
use std::collections::VecDeque;
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

impl RetainedIngress {
    pub(super) const fn kind(&self) -> RetainedIngressKind {
        self.kind
    }

    pub(super) fn admission(&self) -> &ValidatedAdmission {
        &self.admission
    }
}

#[derive(Debug)]
pub(super) enum RetainedIngressError {
    Rejected(RetainedIngressRejection),
    Admission(AdmissionValidationError),
}

#[derive(Clone, Debug)]
pub(super) struct RetainedIngressRejection {
    kind: RetainedIngressKind,
    tx: Arc<TransactionView>,
    reason: CommittedPublicReject,
}

impl RetainedIngressRejection {
    pub(super) const fn kind(&self) -> RetainedIngressKind {
        self.kind
    }

    pub(super) fn is_malformed(&self) -> bool {
        self.reason.is_malformed()
    }

    pub(super) fn transaction(&self) -> &Arc<TransactionView> {
        &self.tx
    }

    pub(super) fn reason(&self) -> &CommittedPublicReject {
        &self.reason
    }

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

/// One non-contextually classified item in a retained-ingress microbatch.
/// Every variant is a terminal result of lock-external validation; authority
/// Plan may retain it, publish its rejection, or record the existing no-owner
/// pressure outcome without re-reading caller-controlled bytes.
#[derive(Debug)]
pub(super) enum RetainedIngressAttempt {
    Validated(RetainedIngress),
    Rejected(RetainedIngressRejection),
    ProposalUnavailable,
}

impl RetainedIngressAttempt {
    pub(super) const fn kind(&self) -> RetainedIngressKind {
        match self {
            Self::Validated(ingress) => ingress.kind(),
            Self::Rejected(rejection) => rejection.kind(),
            Self::ProposalUnavailable => RetainedIngressKind::Proposal,
        }
    }

    pub(super) fn is_malformed_remote(&self) -> bool {
        matches!(
            self,
            Self::Rejected(rejection)
                if matches!(rejection.kind(), RetainedIngressKind::Remote(_))
                    && rejection.is_malformed()
        )
    }
}

/// Non-empty homogeneous retained-ingress capability consumed in canonical
/// controller order. Keeping `head` separate makes an empty authority batch
/// unrepresentable; the private constructor prevents Remote peers or Proposal
/// trust classes from being mixed under one resource/effect policy.
#[derive(Debug)]
pub(super) struct RetainedAdmissionBatch {
    kind: RetainedIngressKind,
    head: RetainedIngressAttempt,
    tail: VecDeque<RetainedIngressAttempt>,
}

impl RetainedAdmissionBatch {
    pub(super) fn new(
        head: RetainedIngressAttempt,
        tail: VecDeque<RetainedIngressAttempt>,
    ) -> Result<Self, RetainedIngressBoundaryError> {
        let Some(item_count) = tail.len().checked_add(1) else {
            return Err(RetainedIngressBoundaryError::ResourceUnavailable);
        };
        if item_count > crate::constants::MAX_POOL_MUTATION_CANDIDATES {
            return Err(RetainedIngressBoundaryError::ResourceUnavailable);
        }
        let kind = head.kind();
        let homogeneous = tail.iter().all(|attempt| match (kind, attempt.kind()) {
            (RetainedIngressKind::Remote(expected), RetainedIngressKind::Remote(actual)) => {
                expected == actual
            }
            (RetainedIngressKind::Proposal, RetainedIngressKind::Proposal) => true,
            (RetainedIngressKind::Remote(_), RetainedIngressKind::Proposal)
            | (RetainedIngressKind::Proposal, RetainedIngressKind::Remote(_)) => false,
        });
        if !homogeneous {
            return Err(RetainedIngressBoundaryError::InvalidEvidence);
        }
        Ok(Self { kind, head, tail })
    }

    pub(super) const fn kind(&self) -> RetainedIngressKind {
        self.kind
    }

    pub(super) fn len(&self) -> usize {
        self.tail.len().saturating_add(1)
    }

    pub(super) fn attempts(&self) -> impl Iterator<Item = &RetainedIngressAttempt> {
        std::iter::once(&self.head).chain(&self.tail)
    }

    pub(super) fn into_attempts(self) -> VecDeque<RetainedIngressAttempt> {
        let mut attempts = self.tail;
        attempts.push_front(self.head);
        attempts
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

impl RemoteIngressPressure {
    pub(super) fn reason(self) -> &'static str {
        match self {
            Self::TotalResources => "tx-pool total residency limit reached",
            Self::RemoteResources => "tx-pool remote residency limit reached",
            Self::PeerResources => "tx-pool per-peer residency limit reached",
            Self::ComputeResources => "tx-pool transient compute limit reached",
            Self::ProposalCollision => "tx-pool proposal short-id collision",
            Self::Allocation => "tx-pool resource allocation unavailable",
        }
    }
}

pub(super) fn remote_pressure_rejection(
    tx: TransactionView,
    peer: PeerIndex,
    pressure: RemoteIngressPressure,
) -> RetainedIngressRejection {
    RetainedIngressRejection {
        kind: RetainedIngressKind::Remote(peer),
        tx: Arc::new(tx.into_compact()),
        reason: CommittedPublicReject::new(crate::error::Reject::Full(
            pressure.reason().to_owned(),
        )),
    }
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

#[cfg(test)]
#[path = "tests/support/ingress.rs"]
pub(in crate::authority) mod test_support;
