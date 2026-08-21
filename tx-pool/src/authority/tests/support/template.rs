use super::*;

impl TemplateCandidate {
    pub(in crate::authority) fn proposal(&self) -> &ProposalId {
        &self.proposal
    }

    pub(in crate::authority) fn status(&self) -> AcceptedStatus {
        self.status
    }
}

impl AuthorityTemplateReadReceipt {
    pub(in crate::authority) fn source_cut(
        &self,
        uncles: CandidateUncleSourceReceipt,
    ) -> TemplateSourceCut {
        TemplateSourceCut::new(self.sources, uncles)
    }

    pub(in crate::authority) fn selected_len(&self) -> usize {
        self.captured.len()
    }
}

impl TemplateSelectionReceipt {
    pub(in crate::authority) fn proposals(
        &self,
        limit: usize,
    ) -> Result<Vec<ProposalId>, TemplateReadError> {
        let ordered = self.ordered_indices([AcceptedStatus::Pending])?;
        let selected = limit.min(ordered.len());
        let mut proposals = Vec::new();
        proposals
            .try_reserve(selected)
            .map_err(|_| TemplateReadError::Allocation)?;
        for index in ordered.into_iter().take(selected) {
            let candidate = self
                .candidates
                .get(index)
                .ok_or(TemplateReadError::Projection)?;
            proposals.push(candidate.proposal.clone());
        }
        Ok(proposals)
    }

    pub(in crate::authority) fn pending_rank(
        &self,
        hash: &RawTxHash,
    ) -> Result<Option<usize>, TemplateReadError> {
        let mut position = None;
        for (rank, index) in self
            .ordered_indices([AcceptedStatus::Pending, AcceptedStatus::Gap])?
            .into_iter()
            .enumerate()
        {
            let candidate = self
                .candidates
                .get(index)
                .ok_or(TemplateReadError::Projection)?;
            if &candidate.hash == hash {
                position = Some(rank);
                break;
            }
        }
        position
            .map(|rank| rank.checked_add(1).ok_or(TemplateReadError::Arithmetic))
            .transpose()
    }

    pub(in crate::authority) fn proposed_parent_first(
        &self,
    ) -> Result<Vec<&TemplateCandidate>, TemplateReadError> {
        self.proposed_parent_first_complete_scan()
    }

    pub(in crate::authority) fn proposed_parent_first_for_foundation(
        &self,
    ) -> Result<Vec<&TemplateCandidate>, TemplateReadError> {
        self.proposed_parent_first_complete_scan()
    }

    fn proposed_parent_first_complete_scan(
        &self,
    ) -> Result<Vec<&TemplateCandidate>, TemplateReadError> {
        let by_hash = self.candidate_index()?;
        let causally_selected = self.causally_eligible_proposed(&by_hash)?;
        let selected = self.order_conditionally_safe(causally_selected, &by_hash)?;
        let mut ordered = Vec::new();
        ordered
            .try_reserve(selected.len())
            .map_err(|_| TemplateReadError::Allocation)?;
        for index in selected {
            ordered.push(
                self.candidates
                    .get(index)
                    .ok_or(TemplateReadError::Projection)?,
            );
        }
        Ok(ordered)
    }
}

impl TemplateConvergence {
    pub(in crate::authority) fn for_foundation(initial: TemplateSourceCut) -> Self {
        Self::new(initial, ResetEpoch::INITIAL)
    }

    pub(in crate::authority) fn begin_partial_for_foundation(
        &mut self,
        component: TemplateComponent,
        sources: TemplateSourceCut,
        base_revision: TemplateRevision,
    ) -> PartialTemplateBuild {
        match component {
            TemplateComponent::Proposals => self.begin_proposals(sources.pool, base_revision),
            TemplateComponent::Transactions => self.begin_transactions(sources.pool, base_revision),
            TemplateComponent::Uncles => self.begin_uncles(sources, base_revision),
        }
    }

    pub(in crate::authority) fn begin_full(
        &mut self,
        sources: TemplateSourceCut,
    ) -> FullTemplateBuild {
        self.observe_sources(sources);
        FullTemplateBuild {
            expected_reset: self.desired_reset,
            expected_reset_chain: self.desired_reset_chain,
            sources,
            coverage: TemplateCoverage::full(sources),
        }
    }

    fn begin_proposals(
        &mut self,
        sources: TemplatePoolSourceCut,
        base_revision: TemplateRevision,
    ) -> PartialTemplateBuild {
        self.observe_pool_sources(sources);
        PartialTemplateBuild {
            expected_revision: base_revision,
            coverage: PartialTemplateCoverage::Proposals(sources.proposal_cut()),
        }
    }

    fn begin_transactions(
        &mut self,
        sources: TemplatePoolSourceCut,
        base_revision: TemplateRevision,
    ) -> PartialTemplateBuild {
        self.observe_pool_sources(sources);
        PartialTemplateBuild {
            expected_revision: base_revision,
            coverage: PartialTemplateCoverage::Transactions(sources.transaction_cut()),
        }
    }

    fn begin_uncles(
        &mut self,
        sources: TemplateSourceCut,
        base_revision: TemplateRevision,
    ) -> PartialTemplateBuild {
        self.observe_sources(sources);
        PartialTemplateBuild {
            expected_revision: base_revision,
            coverage: PartialTemplateCoverage::Uncles(sources.uncle_cut()),
        }
    }

    pub(in crate::authority) fn is_converged(&self, published_reset: ResetEpoch) -> bool {
        self.desired_reset == published_reset
            && self.full_required.is_none()
            && [
                TemplateComponent::Proposals,
                TemplateComponent::Transactions,
                TemplateComponent::Uncles,
            ]
            .into_iter()
            .all(|component| !self.is_pending(component))
    }

    pub(in crate::authority) fn force_reset_epoch_exhaustion_for_foundation(&mut self) {
        self.desired_reset = ResetEpoch::MAX;
    }
}
