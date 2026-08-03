//! Production chain-update input and committed output.
//!
//! Block traversal and payload compaction happen before the authority lock.
//! The runtime consumes this sealed command exactly once against the supplied
//! snapshot; callers cannot provide a synthetic authority revision or rules
//! classification.

use super::{
    chain::{CanonicalChainFacts, ChainBlockChanges, ChainFactsError, ChainPackagingMode},
    plan::{AuthorityFault, Backpressure, PlanError},
    state::ProposalId,
};
use ckb_snapshot::Snapshot;
use ckb_types::{
    core::{BlockView, UncleBlockView},
    packed::{Byte32, ProposalShortId},
};
use std::{collections::HashSet, collections::VecDeque, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ChainPackaging {
    Package,
    ObserveOnly,
}

impl ChainPackaging {
    pub(super) fn authority_mode(self) -> ChainPackagingMode {
        match self {
            Self::Package => ChainPackagingMode::Package,
            Self::ObserveOnly => ChainPackagingMode::ObserveOnly,
        }
    }

    fn packages(self) -> bool {
        matches!(self, Self::Package)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ChainBoundaryError {
    Allocation,
    LifecycleClosed,
    CounterExhausted,
    InvalidFacts,
    InvalidSnapshotEvidence,
    Fault(AuthorityFault),
}

impl From<PlanError> for ChainBoundaryError {
    fn from(error: PlanError) -> Self {
        match error {
            PlanError::Backpressure(Backpressure::Allocation) => Self::Allocation,
            // Chain publication is rebuildable and its effect compiler
            // collapses journal pressure to GenerationReset. Observing raw
            // effect-capacity pressure here therefore proves compiler drift.
            PlanError::Backpressure(Backpressure::EffectCapacity) => {
                Self::Fault(AuthorityFault::EffectProjection)
            }
            PlanError::EffectClosed => Self::LifecycleClosed,
            PlanError::Fault(AuthorityFault::CounterExhausted) => Self::CounterExhausted,
            PlanError::Fault(fault) => Self::Fault(fault),
            PlanError::Backpressure(Backpressure::ProposalCollision) => {
                Self::Fault(AuthorityFault::IndexProjection)
            }
            PlanError::Backpressure(
                Backpressure::TotalResources
                | Backpressure::RemoteResources
                | Backpressure::PeerResources
                | Backpressure::AcceptedResources,
            ) => Self::Fault(AuthorityFault::ResourceProjection),
            PlanError::Backpressure(
                Backpressure::ComputeResources | Backpressure::GenerationReplacement,
            ) => Self::Fault(AuthorityFault::SchedulerProjection),
            PlanError::Duplicate
            | PlanError::PayloadVariant
            | PlanError::Membership(_)
            | PlanError::IngressRevoked(_)
            | PlanError::Stale(_) => Self::Fault(AuthorityFault::MembershipProjection),
        }
    }
}

/// Raw ordered-reorg input retained until all fallible block traversal and
/// canonicalization completes. A preparation failure returns this exact value
/// rather than asking the service to clone or reconstruct attacker-sized
/// block collections.
#[must_use = "a chain update request must be prepared or explicitly discarded"]
pub(super) struct ChainUpdateRequest {
    detached_blocks: VecDeque<BlockView>,
    attached_blocks: VecDeque<BlockView>,
    detached_proposals: HashSet<ProposalShortId>,
    snapshot: Arc<Snapshot>,
    packaging: ChainPackaging,
}

impl ChainUpdateRequest {
    pub(super) fn new(
        detached_blocks: VecDeque<BlockView>,
        attached_blocks: VecDeque<BlockView>,
        detached_proposals: HashSet<ProposalShortId>,
        snapshot: Arc<Snapshot>,
        packaging: ChainPackaging,
    ) -> Self {
        Self {
            detached_blocks,
            attached_blocks,
            detached_proposals,
            snapshot,
            packaging,
        }
    }

    pub(super) fn prepare(self) -> Result<ChainUpdateCommand, ChainUpdatePreparationFailure> {
        match self.prepare_borrowed() {
            Ok(command) => Ok(command),
            Err(error) => Err(ChainUpdatePreparationFailure {
                error,
                request: self,
            }),
        }
    }

    fn prepare_borrowed(&self) -> Result<ChainUpdateCommand, ChainBoundaryError> {
        let mut attached_transactions = Vec::new();
        let mut detached_transactions = Vec::new();
        let mut attached_headers = Vec::new();
        let mut detached_headers = Vec::new();
        let mut committed_hashes = Vec::new();
        let mut candidate_uncles = crate::block_assembler::CandidateUncles::new();

        attached_headers
            .try_reserve(self.attached_blocks.len())
            .map_err(|_| ChainBoundaryError::Allocation)?;
        detached_headers
            .try_reserve(self.detached_blocks.len())
            .map_err(|_| ChainBoundaryError::Allocation)?;
        for block in &self.attached_blocks {
            let transactions = block.transactions();
            attached_transactions
                .try_reserve(transactions.len())
                .map_err(|_| ChainBoundaryError::Allocation)?;
            committed_hashes
                .try_reserve(transactions.len().saturating_sub(1))
                .map_err(|_| ChainBoundaryError::Allocation)?;
            attached_transactions.extend(transactions.iter().cloned());
            committed_hashes.extend(
                transactions
                    .iter()
                    .filter(|tx| !tx.is_cellbase())
                    .map(|tx| {
                        (
                            crate::util::compact_packed(&tx.proposal_short_id()),
                            crate::util::compact_packed(&tx.hash()),
                        )
                    }),
            );
            attached_headers.push(crate::util::compact_packed(&block.header().hash()));
        }
        for block in &self.detached_blocks {
            let transactions = block.transactions();
            detached_transactions
                .try_reserve(transactions.len())
                .map_err(|_| ChainBoundaryError::Allocation)?;
            detached_transactions.extend(transactions.iter().cloned());
            detached_headers.push(crate::util::compact_packed(&block.header().hash()));
            if self.packaging.packages() {
                candidate_uncles
                    .try_insert(block.as_uncle())
                    .map_err(|_| ChainBoundaryError::CounterExhausted)?;
            }
        }

        let mut detached_proposals = Vec::new();
        detached_proposals
            .try_reserve(self.detached_proposals.len())
            .map_err(|_| ChainBoundaryError::Allocation)?;
        detached_proposals.extend(
            self.detached_proposals
                .iter()
                .map(|proposal| ProposalId(crate::util::compact_packed(proposal))),
        );
        detached_proposals.sort_unstable();
        detached_proposals.dedup();
        let mut changed_proposals = Vec::new();
        changed_proposals
            .try_reserve(detached_proposals.len())
            .map_err(|_| ChainBoundaryError::Allocation)?;
        changed_proposals.extend(detached_proposals.iter().cloned());
        let facts = CanonicalChainFacts::from_chain_update(
            ChainBlockChanges::from_chain_update(
                attached_transactions,
                detached_transactions,
                attached_headers,
                detached_headers,
            ),
            changed_proposals,
            detached_proposals,
        )
        .map_err(map_chain_facts_error)?;

        Ok(ChainUpdateCommand {
            facts,
            committed_hashes,
            candidate_uncles: candidate_uncles.into_values(),
            had_detached_chain: !self.detached_blocks.is_empty(),
            packaging: self.packaging,
            snapshot: Arc::clone(&self.snapshot),
        })
    }
}

#[must_use = "a failed chain preparation still owns the exact request"]
pub(super) struct ChainUpdatePreparationFailure {
    error: ChainBoundaryError,
    request: ChainUpdateRequest,
}

impl ChainUpdatePreparationFailure {
    pub(super) fn into_parts(self) -> (ChainBoundaryError, ChainUpdateRequest) {
        (self.error, self.request)
    }
}

impl std::fmt::Debug for ChainUpdatePreparationFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ChainUpdatePreparationFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

#[must_use = "a chain update command must be committed or explicitly discarded"]
pub(super) struct ChainUpdateCommand {
    pub(super) facts: CanonicalChainFacts,
    pub(super) committed_hashes: Vec<(ProposalShortId, Byte32)>,
    pub(super) candidate_uncles: Vec<UncleBlockView>,
    pub(super) had_detached_chain: bool,
    pub(super) packaging: ChainPackaging,
    pub(super) snapshot: Arc<Snapshot>,
}

#[must_use = "a failed chain Apply still owns the prepared command"]
pub(super) struct ChainUpdateFailure {
    error: ChainBoundaryError,
    command: ChainUpdateCommand,
}

impl ChainUpdateFailure {
    pub(super) fn new(error: ChainBoundaryError, command: ChainUpdateCommand) -> Self {
        Self { error, command }
    }

    pub(super) fn into_parts(self) -> (ChainBoundaryError, ChainUpdateCommand) {
        (self.error, self.command)
    }
}

impl std::fmt::Debug for ChainUpdateFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ChainUpdateFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

fn map_chain_facts_error(error: ChainFactsError) -> ChainBoundaryError {
    match error {
        ChainFactsError::Allocation => ChainBoundaryError::Allocation,
        ChainFactsError::DuplicateTransaction | ChainFactsError::DuplicateHeader => {
            ChainBoundaryError::InvalidFacts
        }
    }
}

#[must_use = "candidate uncles and the committed snapshot feed template refresh"]
pub(super) struct CommittedChainUpdate {
    pub(super) candidate_uncles: Vec<UncleBlockView>,
    pub(super) snapshot: Arc<Snapshot>,
}
