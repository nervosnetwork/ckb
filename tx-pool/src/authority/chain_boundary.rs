//! Production chain-update input and committed output.
//!
//! Block traversal and payload compaction happen before the authority lock.
//! The runtime consumes this sealed command exactly once against the supplied
//! snapshot; callers cannot provide a synthetic authority revision or rules
//! classification.

use super::{
    chain::{CanonicalChainFacts, ChainBlockChanges, ChainFactsError},
    plan::{AuthorityFault, Backpressure, PlanError},
};
use crate::block_assembler::{BoundedCandidateUncle, CandidateUncleMutationError};
use ckb_snapshot::Snapshot;
use ckb_types::{
    core::BlockView,
    packed::{Byte32, ProposalShortId},
};
use std::{collections::VecDeque, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CandidateUncleCollection {
    CollectCandidateUncles,
    SkipCandidateUncles,
}

impl CandidateUncleCollection {
    fn collects_candidate_uncles(self) -> bool {
        matches!(self, Self::CollectCandidateUncles)
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
    snapshot: Arc<Snapshot>,
    candidate_uncles: CandidateUncleCollection,
}

impl ChainUpdateRequest {
    pub(super) fn new(
        detached_blocks: VecDeque<BlockView>,
        attached_blocks: VecDeque<BlockView>,
        snapshot: Arc<Snapshot>,
        candidate_uncles: CandidateUncleCollection,
    ) -> Self {
        Self {
            detached_blocks,
            attached_blocks,
            snapshot,
            candidate_uncles,
        }
    }

    pub(super) fn prepare(self) -> Result<ChainUpdateCommand, ChainUpdatePreparationFailure> {
        match self.prepare_borrowed() {
            Ok(mut command) => {
                // The fee estimator is a post-commit projection of the exact
                // attached block sequence. Move that evidence into the sealed
                // command so it cannot run before the authority transition or
                // be reconstructed from a different chain cut afterwards.
                command.attached_blocks = self.attached_blocks;
                Ok(command)
            }
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
            if self.candidate_uncles.collects_candidate_uncles() {
                // This is a fresh, bounded template projection rather than
                // authoritative chain evidence. Source exhaustion can only
                // underfill its derived candidate prefix; it must not reject
                // the exact chain transition or invalidate tx-pool state.
                match candidate_uncles.try_insert(block.as_uncle()) {
                    Ok(_) | Err(CandidateUncleMutationError::SourceVersionExhausted) => {}
                    Err(CandidateUncleMutationError::Allocation) => {
                        return Err(ChainBoundaryError::Allocation);
                    }
                    Err(
                        CandidateUncleMutationError::Arithmetic
                        | CandidateUncleMutationError::TooLarge { .. },
                    ) => return Err(ChainBoundaryError::InvalidFacts),
                }
            }
        }

        let facts = CanonicalChainFacts::from_chain_update(ChainBlockChanges::from_chain_update(
            attached_transactions,
            detached_transactions,
            attached_headers,
            detached_headers,
        ))
        .map_err(map_chain_facts_error)?;

        Ok(ChainUpdateCommand {
            facts,
            committed_hashes,
            candidate_uncles: candidate_uncles
                .into_values()
                .map_err(|_| ChainBoundaryError::Allocation)?,
            attached_blocks: VecDeque::new(),
            had_detached_chain: !self.detached_blocks.is_empty(),
            snapshot: Arc::clone(&self.snapshot),
        })
    }

    /// Allocation pressure cannot postpone the authoritative snapshot change.
    /// Discard the optional detailed reconciliation and retain only the exact
    /// snapshot needed for an empty-generation replacement.
    pub(super) fn into_generation_replacement(self) -> ChainGenerationReplacement {
        ChainGenerationReplacement {
            snapshot: self.snapshot,
        }
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
    pub(super) candidate_uncles: Vec<BoundedCandidateUncle>,
    /// Ordered evidence consumed by the fee-estimator projection only after
    /// this command commits. Empty during borrowed preparation and filled by
    /// `prepare` through ownership transfer from the raw request.
    pub(super) attached_blocks: VecDeque<BlockView>,
    pub(super) had_detached_chain: bool,
    pub(super) snapshot: Arc<Snapshot>,
}

impl ChainUpdateCommand {
    pub(super) fn into_generation_replacement(self) -> ChainGenerationReplacement {
        ChainGenerationReplacement {
            snapshot: self.snapshot,
        }
    }
}

/// Minimum allocation-free ordered-chain consequence. It installs the exact
/// new snapshot while replacing all tx-pool ownership with an empty generation;
/// detached recovery, fee-estimator input and candidate uncles are derived
/// optimizations and cannot veto this safety transition.
#[must_use = "a chain generation replacement must be committed or explicitly discarded"]
pub(super) struct ChainGenerationReplacement {
    snapshot: Arc<Snapshot>,
}

impl ChainGenerationReplacement {
    pub(super) fn from_snapshot(snapshot: Arc<Snapshot>) -> Self {
        Self { snapshot }
    }

    pub(super) fn into_snapshot(self) -> Arc<Snapshot> {
        self.snapshot
    }
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

#[must_use = "post-commit chain projections must consume this exact committed cut"]
pub(super) struct CommittedChainUpdate {
    pub(super) candidate_uncles: Vec<BoundedCandidateUncle>,
    pub(super) attached_blocks: VecDeque<BlockView>,
    pub(super) snapshot: Arc<Snapshot>,
}
