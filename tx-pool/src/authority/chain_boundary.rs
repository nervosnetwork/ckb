//! Production chain-update input and committed output.
//!
//! Block traversal and payload compaction happen before the authority lock.
//! The runtime consumes this sealed command exactly once against the supplied
//! snapshot; callers cannot provide a synthetic authority revision or rules
//! classification.

use super::{
    chain::{ChainBlockChanges, ChainPackagingMode},
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
    EffectCapacity,
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
            PlanError::Backpressure(Backpressure::EffectCapacity) => Self::EffectCapacity,
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

#[must_use = "a chain update command must be committed or explicitly discarded"]
pub(super) struct ChainUpdateCommand {
    pub(super) blocks: ChainBlockChanges,
    pub(super) changed_proposals: Vec<ProposalId>,
    pub(super) detached_proposals: Vec<ProposalId>,
    pub(super) committed_hashes: Vec<(ProposalShortId, Byte32)>,
    pub(super) candidate_uncles: Vec<UncleBlockView>,
    pub(super) had_detached_chain: bool,
    pub(super) packaging: ChainPackaging,
    pub(super) snapshot: Arc<Snapshot>,
}

impl ChainUpdateCommand {
    pub(super) fn new(
        detached_blocks: VecDeque<BlockView>,
        attached_blocks: VecDeque<BlockView>,
        detached_proposals: HashSet<ProposalShortId>,
        snapshot: Arc<Snapshot>,
        packaging: ChainPackaging,
    ) -> Result<Self, ChainBoundaryError> {
        let mut attached_transactions = Vec::new();
        let mut detached_transactions = Vec::new();
        let mut attached_headers = Vec::new();
        let mut detached_headers = Vec::new();
        let mut committed_hashes = Vec::new();
        let mut candidate_uncles = crate::block_assembler::CandidateUncles::new();

        attached_headers
            .try_reserve(attached_blocks.len())
            .map_err(|_| ChainBoundaryError::Allocation)?;
        detached_headers
            .try_reserve(detached_blocks.len())
            .map_err(|_| ChainBoundaryError::Allocation)?;
        for block in &attached_blocks {
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
        for block in &detached_blocks {
            let transactions = block.transactions();
            detached_transactions
                .try_reserve(transactions.len())
                .map_err(|_| ChainBoundaryError::Allocation)?;
            detached_transactions.extend(transactions.iter().cloned());
            detached_headers.push(crate::util::compact_packed(&block.header().hash()));
            if packaging.packages() {
                candidate_uncles
                    .try_insert(block.as_uncle())
                    .map_err(|_| ChainBoundaryError::CounterExhausted)?;
            }
        }

        let mut compact_detached_proposals = Vec::new();
        compact_detached_proposals
            .try_reserve(detached_proposals.len())
            .map_err(|_| ChainBoundaryError::Allocation)?;
        compact_detached_proposals.extend(
            detached_proposals
                .into_iter()
                .map(|proposal| ProposalId(crate::util::compact_packed(&proposal))),
        );
        let mut detached_proposals = compact_detached_proposals;
        detached_proposals.sort_unstable();
        detached_proposals.dedup();
        let mut changed_proposals = Vec::new();
        changed_proposals
            .try_reserve(detached_proposals.len())
            .map_err(|_| ChainBoundaryError::Allocation)?;
        changed_proposals.extend(detached_proposals.iter().cloned());

        let had_detached_chain = !detached_blocks.is_empty();
        Ok(Self {
            blocks: ChainBlockChanges::from_chain_update(
                attached_transactions,
                detached_transactions,
                attached_headers,
                detached_headers,
            ),
            changed_proposals,
            detached_proposals,
            committed_hashes,
            candidate_uncles: candidate_uncles.into_values(),
            had_detached_chain,
            packaging,
            snapshot,
        })
    }
}

#[must_use = "candidate uncles and the committed snapshot feed template refresh"]
pub(super) struct CommittedChainUpdate {
    pub(super) candidate_uncles: Vec<UncleBlockView>,
    pub(super) snapshot: Arc<Snapshot>,
}
