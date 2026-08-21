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
        PreAcceptedSource, ProposalBase, RemoteBase, RemoteDeadline, RemoteResidencyLease,
        ValidatedAdmission,
    },
};
use crate::util::non_contextual_verify;
use ckb_chain_spec::consensus::{Consensus, MAX_BLOCK_INTERVAL};
use ckb_network::PeerIndex;
use ckb_types::core::{Cycle, TransactionView, tx_pool::TRANSACTION_SIZE_LIMIT};
use std::collections::VecDeque;
use std::sync::Arc;

const REMOTE_RESIDENCY_BLOCKS: u64 = 100;

/// Peer-declared script-cycle limit sealed against the paired consensus at
/// the tx-pool ingress boundary.  Downstream authority code can transport and
/// read the declaration, but cannot manufacture a declaration whose work
/// bound exceeds the node's consensus maximum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RemoteCycleLimit(Cycle);

impl RemoteCycleLimit {
    fn checked(declared: Cycle, consensus: &Consensus) -> Option<Self> {
        (declared <= consensus.max_block_cycles()).then_some(Self(declared))
    }

    pub(super) const fn declared(self) -> Cycle {
        self.0
    }
}

/// One externally supplied transaction after the authority-owned residency
/// boundary has proved the protocol byte limit and copied all molecule slices
/// into one bounded backing allocation. The type remains sealed across the
/// controller channel, so no downstream path can accidentally compact the
/// same payload a second time or admit an unproved raw view.
#[derive(Debug)]
pub(crate) struct BoundedTransaction {
    transaction: Arc<TransactionView>,
    payload_bytes: usize,
    encoded_edges: usize,
}

#[derive(Debug)]
pub(crate) enum BoundedTransactionError {
    TooLarge { actual: u64, maximum: u64 },
    Allocation,
}

impl BoundedTransaction {
    pub(crate) fn try_new(transaction: TransactionView) -> Result<Self, BoundedTransactionError> {
        let serialized_bytes = transaction.data().serialized_size_in_block();
        let serialized_bytes_u64 =
            u64::try_from(serialized_bytes).map_err(|_| BoundedTransactionError::Allocation)?;
        if serialized_bytes_u64 > TRANSACTION_SIZE_LIMIT {
            return Err(BoundedTransactionError::TooLarge {
                actual: serialized_bytes_u64,
                maximum: TRANSACTION_SIZE_LIMIT,
            });
        }
        let payload_bytes = transaction.data().total_size();
        let encoded_edges = transaction
            .inputs()
            .len()
            .checked_add(transaction.cell_deps().len())
            .and_then(|count| count.checked_add(transaction.header_deps().len()))
            .ok_or(BoundedTransactionError::Allocation)?;
        let transaction = transaction
            .try_into_compact()
            .map_err(|_| BoundedTransactionError::Allocation)?;
        Ok(Self {
            transaction: Arc::new(transaction),
            payload_bytes,
            encoded_edges,
        })
    }

    pub(crate) const fn payload_bytes(&self) -> usize {
        self.payload_bytes
    }

    pub(crate) fn into_transaction(self) -> Arc<TransactionView> {
        self.transaction
    }

    fn transaction(&self) -> &Arc<TransactionView> {
        &self.transaction
    }

    pub(super) fn into_admission_parts(self) -> (Arc<TransactionView>, usize, usize) {
        (self.transaction, self.payload_bytes, self.encoded_edges)
    }

    pub(super) fn into_direct(self) -> DirectIngressTransaction {
        DirectIngressTransaction(self.transaction)
    }
}

/// Sealed owner-free direct payload retained across exact stale-view retries.
/// The wrapper is borrowed while resolution clones only the fixed-size `Arc`
/// handle, so a retry neither recopies bytes nor permits a raw transaction to
/// bypass the one external residency constructor.
#[derive(Debug)]
pub(super) struct DirectIngressTransaction(Arc<TransactionView>);

impl DirectIngressTransaction {
    pub(super) fn from_retry(transaction: Arc<TransactionView>) -> Self {
        Self(transaction)
    }

    fn clone_transaction(&self) -> Arc<TransactionView> {
        Arc::clone(&self.0)
    }
}

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
    tx: Arc<TransactionView>,
    peer: PeerIndex,
    pressure: RemoteIngressPressure,
) -> RetainedIngressRejection {
    RetainedIngressRejection {
        kind: RetainedIngressKind::Remote(peer),
        tx,
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
    tx: BoundedTransaction,
    declared_cycles: Cycle,
    peer: PeerIndex,
    consensus: &Consensus,
) -> RetainedIngressAttempt {
    remote_at(
        tx,
        declared_cycles,
        peer,
        ckb_systemtime::unix_time().as_secs(),
        consensus,
    )
}

fn remote_at(
    tx: BoundedTransaction,
    declared_cycles: Cycle,
    peer: PeerIndex,
    admitted_at_secs: u64,
    consensus: &Consensus,
) -> RetainedIngressAttempt {
    let Some(declared_limit) = RemoteCycleLimit::checked(declared_cycles, consensus) else {
        return RetainedIngressAttempt::Rejected(RetainedIngressRejection {
            kind: RetainedIngressKind::Remote(peer),
            tx: tx.into_transaction(),
            reason: CommittedPublicReject::new(crate::error::Reject::Malformed(
                "remote declared cycles".to_owned(),
                format!(
                    "declared cycles {declared_cycles} exceed consensus maximum {}",
                    consensus.max_block_cycles()
                ),
            )),
        });
    };
    let tx = match validate_non_contextual(tx, RetainedIngressKind::Remote(peer), consensus) {
        Ok(tx) => tx,
        Err(rejection) => return RetainedIngressAttempt::Rejected(rejection),
    };
    let expires_at =
        admitted_at_secs.saturating_add(REMOTE_RESIDENCY_BLOCKS.saturating_mul(MAX_BLOCK_INTERVAL));
    match ValidatedAdmission::from_retained_ingress(
        RetainedIngressSeal(()),
        tx,
        PreAcceptedSource::Remote(RemoteBase::ingress(
            RemoteResidencyLease::new(peer, RemoteDeadline(expires_at)),
            declared_limit,
        )),
    ) {
        Ok(admission) => RetainedIngressAttempt::Validated(RetainedIngress {
            kind: RetainedIngressKind::Remote(peer),
            admission,
        }),
        Err(failure) => RetainedIngressAttempt::Rejected(remote_pressure_rejection(
            failure.into_transaction(),
            peer,
            RemoteIngressPressure::Allocation,
        )),
    }
}

/// Validate and seal one trusted Proposal admission. Proposal-window
/// placement is derived later from the paired snapshot; there is no caller-
/// supplied context or lease token to retain here.
pub(super) fn proposal(tx: BoundedTransaction, consensus: &Consensus) -> RetainedIngressAttempt {
    let tx = match validate_non_contextual(tx, RetainedIngressKind::Proposal, consensus) {
        Ok(tx) => tx,
        Err(rejection) => return RetainedIngressAttempt::Rejected(rejection),
    };
    match ValidatedAdmission::from_retained_ingress(
        RetainedIngressSeal(()),
        tx,
        PreAcceptedSource::Proposal {
            base: ProposalBase::Trusted,
        },
    ) {
        Ok(admission) => RetainedIngressAttempt::Validated(RetainedIngress {
            kind: RetainedIngressKind::Proposal,
            admission,
        }),
        Err(_) => RetainedIngressAttempt::ProposalUnavailable,
    }
}

/// Validate and compact a synchronous Local/TestAccept transaction before it
/// can enter owner-free resolution. Rejection retains the exact transaction
/// for Local publication without creating an authority owner.
#[expect(
    clippy::result_large_err,
    reason = "the rejection retains exact source and sparse Accepted-read evidence inline; boxing would allocate on hostile direct ingress"
)]
pub(super) fn direct(
    tx: &DirectIngressTransaction,
    consensus: &Consensus,
    command: DirectCommand,
) -> Result<DirectTransaction, DirectTransactionRejection> {
    let tx = tx.clone_transaction();
    non_contextual_verify(consensus, &tx)
        .map_err(|reason| DirectTransactionRejection::stable(Arc::clone(&tx), command, reason))?;
    Ok(DirectTransaction { tx, command })
}

fn validate_non_contextual(
    tx: BoundedTransaction,
    kind: RetainedIngressKind,
    consensus: &Consensus,
) -> Result<BoundedTransaction, RetainedIngressRejection> {
    match non_contextual_verify(consensus, tx.transaction()) {
        Ok(()) => Ok(tx),
        Err(reason) => Err(RetainedIngressRejection {
            kind,
            tx: tx.into_transaction(),
            reason: CommittedPublicReject::new(reason),
        }),
    }
}

#[cfg(test)]
#[path = "tests/support/ingress.rs"]
pub(in crate::authority) mod test_support;
