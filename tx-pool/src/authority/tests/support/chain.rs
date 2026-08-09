use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) enum AdmissionEvidenceError {
    ScriptRulesChanged,
}

pub(in crate::authority) type FinalAdmissionError = AdmissionEvidenceError;
type DirectAdmissionError = AdmissionEvidenceError;

impl CellLocationReceipt {
    pub(in crate::authority) fn is_chain_input(&self, input: &OutPoint) -> bool {
        self.chain_inputs.binary_search(input).is_ok()
    }

    pub(in crate::authority) fn is_chain_dependency(&self, dependency: &OutPoint) -> bool {
        self.chain_dependencies.binary_search(dependency).is_ok()
    }
}

impl FinalAdmissionRejection {
    pub(in crate::authority) fn reason(&self) -> &CommittedPublicReject {
        &self.reason
    }
}

impl FinalAdmissionSubject {
    pub(in crate::authority) fn for_foundation(
        key: RawTxHash,
        expected: EntryVersion,
        view: ChainViewId,
        dependency_cut: DependencyCut,
    ) -> Self {
        Self {
            key,
            expected,
            view,
            dependency_cut,
        }
    }
}

impl DirectAdmissionReceipt {
    pub(in crate::authority) fn transaction(&self) -> &Arc<TransactionView> {
        &self.tx
    }
}

impl DirectAdmissionRejection {
    pub(in crate::authority) fn reason(&self) -> &CommittedPublicReject {
        &self.reason
    }
}

impl CellLocationReceipt {
    pub(in crate::authority) fn empty_for_foundation(view: &ChainViewId) -> Self {
        Self {
            view: view.clone(),
            chain_inputs: Arc::from([]),
            chain_dependencies: Arc::from([]),
        }
    }
}

impl VerificationContextReceipt {
    pub(in crate::authority) fn empty_for_foundation(
        view: ChainViewId,
        rules: ScriptVerificationRules,
    ) -> Self {
        Self {
            view,
            chain_inputs: Arc::from([]),
            chain_dependencies: Arc::from([]),
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

impl AcceptedProof {
    pub(in crate::authority) fn for_foundation(verified: VerifiedFacts) -> Self {
        Self {
            verified,
            sensitivity: AcceptedChainSensitivity::Stable,
        }
    }

    pub(in crate::authority) fn equivalent_after_atomic_stamp_compaction(
        &self,
        other: &Self,
        batch: ApplySequence,
        canonical_next: ApplySequence,
    ) -> bool {
        self.sensitivity == other.sensitivity
            && self.verified.equivalent_after_atomic_stamp_compaction(
                &other.verified,
                batch,
                canonical_next,
            )
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
            .with_context(context)
            .ok_or(AdmissionEvidenceError::ScriptRulesChanged)?;
        let (verified, async_process_start) = verified.into_accepted();
        Ok(MembershipReceipt {
            proof: AcceptedProof {
                verified,
                sensitivity,
            },
            proposal: ProposalContextReceipt::from_validation(status),
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
            key: self.key,
            expected: self.expected,
            membership: self.validation.validate_at_for_foundation(
                status,
                rules,
                AcceptedChainSensitivity::Stable,
                accepted_at,
            )?,
            payload_relation: ReadyPayloadRelation::Shared,
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
            key: self.key,
            expected: self.expected,
            membership: self
                .validation
                .validate_for_foundation(status, rules, sensitivity)?,
            payload_relation: ReadyPayloadRelation::Shared,
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
    accepted_validity: AcceptedValidityTransition,
    packaging: ChainPackagingMode,
}

impl ChainTransitionFacts {
    pub(in crate::authority) fn for_foundation(
        new_view: ChainViewId,
        blocks: ChainBlockChanges,
        changed_proposals: Vec<ProposalId>,
        detached_proposals: Vec<ProposalId>,
        packaging: ChainPackagingMode,
    ) -> Result<Self, ChainFactsError> {
        let had_detached_transactions = !blocks.detached.is_empty();
        let had_detached_headers = !blocks.detached_headers.is_empty();
        let canonical =
            CanonicalChainFacts::from_chain_update(blocks, changed_proposals, detached_proposals)?;
        Ok(Self {
            new_view,
            canonical,
            accepted_validity: if had_detached_transactions || had_detached_headers {
                AcceptedValidityTransition::ContextChanged
            } else {
                AcceptedValidityTransition::Preserved
            },
            packaging,
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
            self.packaging,
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
