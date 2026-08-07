use super::state::{
    AcceptedStatus, Omega, OwnerLocation, RetainedOwner, RetainedPhase, RulesId, Source, TxId,
    WitnessId, WorkPermit, WorkStage,
};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct TemplateSources {
    pub(super) replacement: u16,
    pub(super) proposals: u16,
    pub(super) transactions: u16,
    pub(super) uncles: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TemplateLane {
    Full,
    Reset,
    Proposals,
    Transactions,
    Uncles,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TemplateReceipt {
    pub(super) lane: TemplateLane,
    pub(super) sources: TemplateSources,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TemplateDisposition {
    Captured(TemplateReceipt),
    ReplacementBusy,
    FullPreemptedReset(TemplateReceipt),
    Published(TemplateLane),
    Stale(TemplateLane),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct TemplateProtocol {
    pub(super) current: TemplateSources,
    pub(super) published: TemplateSources,
    replacement_claim: Option<TemplateLane>,
}

impl TemplateProtocol {
    pub(super) fn advance(&mut self, lane: TemplateLane) -> bool {
        let source = match lane {
            TemplateLane::Full | TemplateLane::Reset => &mut self.current.replacement,
            TemplateLane::Proposals => &mut self.current.proposals,
            TemplateLane::Transactions => &mut self.current.transactions,
            TemplateLane::Uncles => &mut self.current.uncles,
        };
        let Some(next) = source.checked_add(1) else {
            return false;
        };
        *source = next;
        true
    }

    pub(super) fn capture(&mut self, lane: TemplateLane) -> TemplateDisposition {
        match lane {
            TemplateLane::Full | TemplateLane::Reset => match self.replacement_claim {
                None => {
                    if !self.bump_replacement() {
                        return TemplateDisposition::ReplacementBusy;
                    }
                    self.replacement_claim = Some(lane);
                    TemplateDisposition::Captured(TemplateReceipt {
                        lane,
                        sources: self.current,
                    })
                }
                Some(TemplateLane::Reset) if lane == TemplateLane::Full => {
                    if !self.bump_replacement() {
                        return TemplateDisposition::ReplacementBusy;
                    }
                    self.replacement_claim = Some(TemplateLane::Full);
                    TemplateDisposition::FullPreemptedReset(TemplateReceipt {
                        lane,
                        sources: self.current,
                    })
                }
                Some(_) => TemplateDisposition::ReplacementBusy,
            },
            TemplateLane::Proposals | TemplateLane::Transactions | TemplateLane::Uncles => {
                TemplateDisposition::Captured(TemplateReceipt {
                    lane,
                    sources: self.current,
                })
            }
        }
    }

    pub(super) fn publish(&mut self, receipt: TemplateReceipt) -> TemplateDisposition {
        let current = match receipt.lane {
            TemplateLane::Full | TemplateLane::Reset => {
                self.replacement_claim == Some(receipt.lane) && receipt.sources == self.current
            }
            TemplateLane::Proposals => {
                receipt.sources.replacement == self.current.replacement
                    && receipt.sources.proposals == self.current.proposals
            }
            TemplateLane::Transactions => {
                receipt.sources.replacement == self.current.replacement
                    && receipt.sources.transactions == self.current.transactions
            }
            TemplateLane::Uncles => {
                receipt.sources.replacement == self.current.replacement
                    && receipt.sources.uncles == self.current.uncles
            }
        };
        if !current {
            if matches!(receipt.lane, TemplateLane::Full | TemplateLane::Reset)
                && self.replacement_claim == Some(receipt.lane)
            {
                self.replacement_claim = None;
            }
            return TemplateDisposition::Stale(receipt.lane);
        }
        match receipt.lane {
            TemplateLane::Full | TemplateLane::Reset => {
                self.published = receipt.sources;
                self.replacement_claim = None;
            }
            TemplateLane::Proposals => self.published.proposals = receipt.sources.proposals,
            TemplateLane::Transactions => {
                self.published.transactions = receipt.sources.transactions;
            }
            TemplateLane::Uncles => self.published.uncles = receipt.sources.uncles,
        }
        TemplateDisposition::Published(receipt.lane)
    }

    fn bump_replacement(&mut self) -> bool {
        let Some(next) = self.current.replacement.checked_add(1) else {
            return false;
        };
        self.current.replacement = next;
        true
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CandidateUncle {
    pub(super) id: u8,
    pub(super) proposals: BTreeSet<TxId>,
}

pub(super) fn filter_uncles_conflicting_with_proposals(
    uncles: impl IntoIterator<Item = CandidateUncle>,
    proposals: &BTreeSet<TxId>,
) -> Vec<CandidateUncle> {
    uncles
        .into_iter()
        .filter(|uncle| uncle.proposals.is_disjoint(proposals))
        .collect()
}

pub(super) fn persistence_projection(omega: &Omega) -> Vec<TxId> {
    omega
        .authority
        .owners
        .iter()
        .filter_map(|(id, owner)| {
            (matches!(owner.location, OwnerLocation::Accepted { .. })
                || matches!(owner.retained_source(), Some(Source::Recovery(_))))
            .then_some(*id)
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct VerificationKey {
    pub(super) witness: WitnessId,
    pub(super) rules: RulesId,
}

impl VerificationKey {
    pub(super) const fn new(witness: WitnessId, rules: RulesId) -> Self {
        Self { witness, rules }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum QueryStatus {
    Pending,
    Proposed,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum QuerySubject {
    Accepted(AcceptedStatus),
    PreAcceptedPending,
    PreAcceptedProposalAware(AcceptedStatus),
    Hidden,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct QueryProjection {
    pub(super) status: QueryStatus,
    pub(super) minimum_replacement_fee: Option<u64>,
}

pub(super) fn query_projection(
    subject: QuerySubject,
    descendant_fee: u64,
    minimum_rate: u64,
    transaction_size: u64,
) -> QueryProjection {
    let status = match subject {
        QuerySubject::Accepted(AcceptedStatus::Proposed)
        | QuerySubject::PreAcceptedProposalAware(AcceptedStatus::Proposed) => QueryStatus::Proposed,
        QuerySubject::Accepted(AcceptedStatus::Pending | AcceptedStatus::Gap)
        | QuerySubject::PreAcceptedPending
        | QuerySubject::PreAcceptedProposalAware(AcceptedStatus::Pending | AcceptedStatus::Gap) => {
            QueryStatus::Pending
        }
        QuerySubject::Hidden => QueryStatus::Unknown,
    };
    let minimum_replacement_fee = matches!(
        subject,
        QuerySubject::Accepted(AcceptedStatus::Pending | AcceptedStatus::Gap)
    )
    .then(|| {
        minimum_rate
            .checked_mul(transaction_size)
            .and_then(|increment| descendant_fee.checked_add(increment))
    })
    .flatten();
    QueryProjection {
        status,
        minimum_replacement_fee,
    }
}

pub(super) fn query_subject(
    omega: &Omega,
    transaction: TxId,
    proposal_window_status: AcceptedStatus,
) -> QuerySubject {
    match omega
        .authority
        .owners
        .get(&transaction)
        .map(|owner| &owner.location)
    {
        Some(OwnerLocation::Accepted { status, .. }) => QuerySubject::Accepted(*status),
        Some(OwnerLocation::Retained(RetainedOwner {
            phase:
                RetainedPhase::Queued(WorkStage::Verify(_))
                | RetainedPhase::Computing(WorkPermit::VerifyOnly(_))
                | RetainedPhase::Ready(_),
            ..
        })) => QuerySubject::PreAcceptedProposalAware(proposal_window_status),
        Some(OwnerLocation::Retained(_)) => QuerySubject::PreAcceptedPending,
        Some(OwnerLocation::ReplacementHistory { .. }) | None => QuerySubject::Hidden,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CallbackAccess {
    AuthorityMutation,
    CoherentRead,
    NonblockingDerivedControl,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CallbackDisposition {
    Allowed,
    ReentrantMutationRejected,
}

pub(super) fn callback_disposition(
    callback_active: bool,
    access: CallbackAccess,
) -> CallbackDisposition {
    if callback_active && access == CallbackAccess::AuthorityMutation {
        CallbackDisposition::ReentrantMutationRejected
    } else {
        CallbackDisposition::Allowed
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum VerificationControl {
    Running,
    Suspended,
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ActiveVerificationAction {
    Continue,
    ReturnCapability,
}

impl VerificationControl {
    pub(super) fn suspend(self) -> Self {
        match self {
            Self::Running => Self::Suspended,
            state => state,
        }
    }

    pub(super) fn resume(self) -> Self {
        match self {
            Self::Suspended => Self::Running,
            state => state,
        }
    }

    pub(super) const fn stop(self) -> Self {
        Self::Stopped
    }

    pub(super) const fn checkout_allowed(self) -> bool {
        matches!(self, Self::Running)
    }

    pub(super) const fn active_action(self) -> ActiveVerificationAction {
        match self {
            Self::Running | Self::Suspended => ActiveVerificationAction::Continue,
            Self::Stopped => ActiveVerificationAction::ReturnCapability,
        }
    }
}
