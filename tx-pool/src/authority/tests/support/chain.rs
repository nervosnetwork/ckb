use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) enum AdmissionEvidenceError {
    ScriptRulesChanged,
}

pub(in crate::authority) type FinalAdmissionError = AdmissionEvidenceError;
type DirectAdmissionError = AdmissionEvidenceError;

impl FinalAdmissionRejection {
    pub(in crate::authority) fn reason(&self) -> &CommittedPublicReject {
        &self.reason
    }
}

impl VerificationContextReceipt {
    pub(in crate::authority) fn with_cells_for_foundation(
        view: ChainViewId,
        mut chain_inputs: Vec<OutPoint>,
        mut chain_dependencies: Vec<OutPoint>,
        rules: ScriptVerificationRules,
    ) -> Self {
        chain_inputs.sort_unstable();
        chain_inputs.dedup();
        chain_dependencies.sort_unstable();
        chain_dependencies.dedup();
        Self {
            view,
            chain_inputs: Arc::new(chain_inputs),
            chain_dependencies: Arc::new(chain_dependencies),
            time: TimeContextReceipt::from_validation(rules),
        }
    }

    fn refreshed_for_foundation(&self, view: ChainViewId, rules: ScriptVerificationRules) -> Self {
        Self {
            view,
            chain_inputs: Arc::clone(&self.chain_inputs),
            chain_dependencies: Arc::clone(&self.chain_dependencies),
            time: TimeContextReceipt::from_validation(rules),
        }
    }
}

impl MembershipValidationWork {
    fn validate_for_foundation(
        self,
        status: AcceptedStatus,
        rules: ScriptVerificationRules,
        sensitivity: AcceptedChainSensitivity,
    ) -> Result<MembershipReceipt, AdmissionEvidenceError> {
        self.validate_at_for_foundation(status, rules, sensitivity, AcceptedAtMillis::FOUNDATION)
    }

    fn validate_at_for_foundation(
        self,
        status: AcceptedStatus,
        rules: ScriptVerificationRules,
        sensitivity: AcceptedChainSensitivity,
        accepted_at: AcceptedAtMillis,
    ) -> Result<MembershipReceipt, AdmissionEvidenceError> {
        let context = self
            .verified
            .verification_context()
            .refreshed_for_foundation(self.view, rules);
        let verified = self
            .verified
            .with_context_for_foundation(context)
            .ok_or(AdmissionEvidenceError::ScriptRulesChanged)?;
        let (verified, async_process_start) = verified.into_accepted();
        Ok(MembershipReceipt {
            proof: AcceptedProof {
                verified,
                sensitivity,
            },
            proposal: ProposalContextReceipt::from_internal_status(status),
            accepted_at,
            async_process_start,
        })
    }
}

impl FinalAdmissionWork {
    pub(in crate::authority) fn validate_for_foundation(
        self,
        status: AcceptedStatus,
        rules: ScriptVerificationRules,
    ) -> Result<FinalAdmissionReceipt, FinalAdmissionError> {
        self.validate_with_sensitivity_for_foundation(
            status,
            rules,
            AcceptedChainSensitivity::Stable,
        )
    }

    pub(in crate::authority) fn validate_at_for_foundation(
        self,
        status: AcceptedStatus,
        rules: ScriptVerificationRules,
        accepted_at: AcceptedAtMillis,
    ) -> Result<FinalAdmissionReceipt, FinalAdmissionError> {
        Ok(FinalAdmissionReceipt {
            expected: self.expected,
            membership: self.validation.validate_at_for_foundation(
                status,
                rules,
                AcceptedChainSensitivity::Stable,
                accepted_at,
            )?,
        })
    }

    pub(in crate::authority) fn validate_context_sensitive_for_foundation(
        self,
        status: AcceptedStatus,
        rules: ScriptVerificationRules,
    ) -> Result<FinalAdmissionReceipt, FinalAdmissionError> {
        self.validate_with_sensitivity_for_foundation(
            status,
            rules,
            AcceptedChainSensitivity::TipContext,
        )
    }

    fn validate_with_sensitivity_for_foundation(
        self,
        status: AcceptedStatus,
        rules: ScriptVerificationRules,
        sensitivity: AcceptedChainSensitivity,
    ) -> Result<FinalAdmissionReceipt, FinalAdmissionError> {
        Ok(FinalAdmissionReceipt {
            expected: self.expected,
            membership: self
                .validation
                .validate_for_foundation(status, rules, sensitivity)?,
        })
    }
}

impl DirectAdmissionWork {
    pub(in crate::authority) fn validate_for_foundation(
        self,
        status: AcceptedStatus,
        rules: ScriptVerificationRules,
    ) -> Result<DirectAdmissionReceipt, DirectAdmissionError> {
        Ok(DirectAdmissionReceipt {
            tx: self.tx,
            membership: self.validation.validate_for_foundation(
                status,
                rules,
                AcceptedChainSensitivity::Stable,
            )?,
        })
    }
}

impl ChainBlockChanges {
    pub(in crate::authority) fn for_foundation(
        attached: Vec<TransactionView>,
        detached: Vec<TransactionView>,
        attached_headers: Vec<Byte32>,
        detached_headers: Vec<Byte32>,
    ) -> Self {
        Self::from_chain_update(attached, detached, attached_headers, detached_headers)
    }
}

#[derive(Debug)]
pub(in crate::authority) struct ChainTransitionFacts {
    new_view: ChainViewId,
    canonical: CanonicalChainFacts,
    proposals: ProposalTransitionFacts,
    accepted_validity: AcceptedValidityTransition,
}

impl ChainTransitionFacts {
    pub(in crate::authority) fn for_foundation(
        new_view: ChainViewId,
        blocks: ChainBlockChanges,
        changed_proposals: Vec<ProposalId>,
    ) -> Result<Self, ChainFactsError> {
        let had_detached_transactions = !blocks.detached.is_empty();
        let had_detached_headers = !blocks.detached_headers.is_empty();
        let changed = canonical_proposals(changed_proposals);
        let canonical = CanonicalChainFacts::from_chain_update(blocks)?;
        Ok(Self {
            new_view,
            canonical,
            proposals: ProposalTransitionFacts { changed },
            accepted_validity: if had_detached_transactions || had_detached_headers {
                AcceptedValidityTransition::ContextChanged
            } else {
                AcceptedValidityTransition::Preserved
            },
        })
    }

    pub(in crate::authority) fn revalidate_all_for_foundation(mut self) -> Self {
        self.accepted_validity = AcceptedValidityTransition::RulesChanged;
        self
    }

    pub(in crate::authority) fn as_view(&self) -> ChainTransitionFactsView<'_> {
        self.canonical.bind(
            self.new_view.clone(),
            self.accepted_validity,
            &self.proposals,
        )
    }
}

impl ChainValidationWork {
    pub(in crate::authority) fn validate_for_foundation(
        self,
        positions: Vec<(ProposalId, ProposalWindowPosition)>,
    ) -> Result<ChainTransitionReceipt, ChainValidationError> {
        self.validate_positions(positions)
    }
}
