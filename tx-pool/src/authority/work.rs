use super::state::{
    ChainEpoch, ComputeLeaseId, ComputedOutcome, EntryVersion, ObservedDependencies, QueuedWork,
    RawTxHash, RejectionKind, ResolvedFacts, ResolvedPayload, TxIdentity, VerifiedFacts,
    WaitCondition, WorkPermit,
};
use ckb_types::core::TransactionView;
use std::sync::Arc;

#[derive(Debug)]
pub(super) struct LeaseToken {
    pub(super) hash: RawTxHash,
    pub(super) version: EntryVersion,
    pub(super) lease: ComputeLeaseId,
    pub(super) chain_epoch: ChainEpoch,
    pub(super) permit: WorkPermit,
}

#[derive(Debug)]
pub(super) struct ResolveWork {
    token: LeaseToken,
    tx: Arc<TransactionView>,
}

#[derive(Debug)]
pub(super) struct ContinuousResolveWork {
    token: LeaseToken,
    tx: Arc<TransactionView>,
}

#[derive(Debug)]
pub(super) struct VerifyWork {
    token: LeaseToken,
    tx: Arc<TransactionView>,
    resolved: ResolvedFacts,
}

#[derive(Debug)]
pub(super) struct ContinuousVerifyWork {
    token: LeaseToken,
    tx: Arc<TransactionView>,
    resolved: ResolvedFacts,
}

#[derive(Debug)]
#[must_use = "checked-out work owns the only live compute capability"]
pub(super) enum CheckedOutWork {
    Resolve(ResolveWork),
    ContinuousResolve(ContinuousResolveWork),
    Verify(VerifyWork),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WorkPermitMismatch {
    Incompatible,
}

#[derive(Debug)]
pub(super) enum SettlementNext {
    QueuedVerify(ResolvedFacts),
    Waiting(WaitCondition),
    Computed(ComputedOutcome),
}

#[derive(Debug)]
#[must_use = "a settlement must be planned and applied or explicitly discarded as stale"]
pub(super) struct ComputeSettlement {
    pub(super) token: LeaseToken,
    pub(super) next: SettlementNext,
}

fn verified(identity: &TxIdentity, resolved: ResolvedFacts) -> ComputedOutcome {
    ComputedOutcome::Verified(VerifiedFacts {
        witness: identity.witness.clone(),
        chain_epoch: resolved.chain_epoch,
        payload: resolved.payload,
    })
}

impl ResolveWork {
    pub(super) fn transaction(&self) -> &TransactionView {
        &self.tx
    }

    pub(super) fn missing(self, dependencies: ObservedDependencies) -> ComputeSettlement {
        ComputeSettlement {
            token: self.token,
            next: SettlementNext::Waiting(WaitCondition::Missing(dependencies)),
        }
    }

    pub(super) fn yield_verify(self, payload: ResolvedPayload) -> ComputeSettlement {
        let resolved = ResolvedFacts {
            chain_epoch: self.token.chain_epoch,
            payload: Arc::new(payload),
        };
        ComputeSettlement {
            token: self.token,
            next: SettlementNext::QueuedVerify(resolved),
        }
    }

    pub(super) fn rejected(self, reason: RejectionKind) -> ComputeSettlement {
        ComputeSettlement {
            token: self.token,
            next: SettlementNext::Computed(ComputedOutcome::Rejected(reason)),
        }
    }
}

impl ContinuousResolveWork {
    pub(super) fn transaction(&self) -> &TransactionView {
        &self.tx
    }

    pub(super) fn missing(self, dependencies: ObservedDependencies) -> ComputeSettlement {
        ComputeSettlement {
            token: self.token,
            next: SettlementNext::Waiting(WaitCondition::Missing(dependencies)),
        }
    }

    pub(super) fn into_verify(self, payload: ResolvedPayload) -> ContinuousVerifyWork {
        let resolved = ResolvedFacts {
            chain_epoch: self.token.chain_epoch,
            payload: Arc::new(payload),
        };
        ContinuousVerifyWork {
            token: self.token,
            tx: self.tx,
            resolved,
        }
    }

    pub(super) fn rejected(self, reason: RejectionKind) -> ComputeSettlement {
        ComputeSettlement {
            token: self.token,
            next: SettlementNext::Computed(ComputedOutcome::Rejected(reason)),
        }
    }
}

impl VerifyWork {
    pub(super) fn verified(self) -> ComputeSettlement {
        let identity = TxIdentity::from_transaction(&self.tx);
        ComputeSettlement {
            next: SettlementNext::Computed(verified(&identity, self.resolved)),
            token: self.token,
        }
    }

    pub(super) fn rejected(self, reason: RejectionKind) -> ComputeSettlement {
        ComputeSettlement {
            token: self.token,
            next: SettlementNext::Computed(ComputedOutcome::Rejected(reason)),
        }
    }

    pub(super) fn internal_failure(self) -> ComputeSettlement {
        ComputeSettlement {
            token: self.token,
            next: SettlementNext::Computed(ComputedOutcome::InternalFailure),
        }
    }
}

impl ContinuousVerifyWork {
    pub(super) fn verified(self) -> ComputeSettlement {
        let identity = TxIdentity::from_transaction(&self.tx);
        ComputeSettlement {
            next: SettlementNext::Computed(verified(&identity, self.resolved)),
            token: self.token,
        }
    }

    pub(super) fn internal_failure(self) -> ComputeSettlement {
        ComputeSettlement {
            token: self.token,
            next: SettlementNext::Computed(ComputedOutcome::InternalFailure),
        }
    }

    pub(super) fn rejected(self, reason: RejectionKind) -> ComputeSettlement {
        ComputeSettlement {
            token: self.token,
            next: SettlementNext::Computed(ComputedOutcome::Rejected(reason)),
        }
    }
}

impl CheckedOutWork {
    pub(super) fn new(
        token: LeaseToken,
        tx: Arc<TransactionView>,
        queued: QueuedWork,
    ) -> Result<Self, WorkPermitMismatch> {
        match (token.permit, queued) {
            (WorkPermit::ResolveOnly, QueuedWork::Resolve) => {
                Ok(Self::Resolve(ResolveWork { token, tx }))
            }
            (WorkPermit::ResolveThenVerify, QueuedWork::Resolve) => {
                Ok(Self::ContinuousResolve(ContinuousResolveWork { token, tx }))
            }
            (WorkPermit::VerifyOnly, QueuedWork::Verify(resolved)) => {
                Ok(Self::Verify(VerifyWork {
                    token,
                    tx,
                    resolved,
                }))
            }
            _ => Err(WorkPermitMismatch::Incompatible),
        }
    }
}
