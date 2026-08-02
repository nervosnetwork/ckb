//! Sealed construction of retained tx-pool admission evidence.
//!
//! Remote and Proposal callers provide only protocol facts. This module owns
//! non-contextual validation, compact payload materialization, the historical
//! Remote residency deadline, and construction of the private source value.
//! A caller therefore cannot stamp a deadline, trust class, lease, generation,
//! or dependency charge into authority state.

use super::{
    plan::PlanError,
    rejection::CommittedPublicReject,
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

impl RetainedIngress {
    pub(super) fn into_parts(self) -> (RetainedIngressKind, ValidatedAdmission) {
        (self.kind, self.admission)
    }

    #[cfg(test)]
    pub(super) fn admission_for_foundation(&self) -> &ValidatedAdmission {
        &self.admission
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

    #[cfg(test)]
    pub(super) fn reason_for_foundation(&self) -> &CommittedPublicReject {
        &self.reason
    }
}

#[derive(Debug)]
pub(super) enum RetainedIngressBoundaryError {
    Admission(AdmissionValidationError),
    Plan(PlanError),
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
        consensus: &Consensus,
    ) -> Result<RetainedIngressCommit, RetainedIngressBoundaryError> {
        match remote(tx, declared_cycles, peer, consensus) {
            Ok(ingress) => self
                .commit_retained_ingress(ingress)
                .map_err(RetainedIngressBoundaryError::Plan),
            Err(RetainedIngressError::Rejected(rejection)) => self
                .commit_retained_ingress_rejection(rejection)
                .map_err(RetainedIngressBoundaryError::Plan),
            Err(RetainedIngressError::Admission(error)) => {
                Err(RetainedIngressBoundaryError::Admission(error))
            }
        }
    }

    pub(super) fn submit_proposal_ingress(
        &self,
        tx: TransactionView,
        consensus: &Consensus,
    ) -> Result<RetainedIngressCommit, RetainedIngressBoundaryError> {
        match proposal(tx, consensus) {
            Ok(ingress) => self
                .commit_retained_ingress(ingress)
                .map_err(RetainedIngressBoundaryError::Plan),
            Err(RetainedIngressError::Rejected(rejection)) => self
                .commit_retained_ingress_rejection(rejection)
                .map_err(RetainedIngressBoundaryError::Plan),
            Err(RetainedIngressError::Admission(error)) => {
                Err(RetainedIngressBoundaryError::Admission(error))
            }
        }
    }
}

#[cfg(test)]
pub(super) fn remote_at_for_foundation(
    tx: TransactionView,
    declared_cycles: Cycle,
    peer: PeerIndex,
    admitted_at_secs: u64,
    consensus: &Consensus,
) -> Result<RetainedIngress, RetainedIngressError> {
    remote_at(tx, declared_cycles, peer, admitted_at_secs, consensus)
}
