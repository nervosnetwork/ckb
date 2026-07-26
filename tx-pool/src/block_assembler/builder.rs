//! Block template and its mutable builder.

use crate::component::entry::TxEntry;
use crate::error::BlockAssemblerError;
use ckb_snapshot::Snapshot;
use ckb_types::{
    core::{
        BlockNumber, Cycle, EpochExt, EpochNumberWithFraction, TransactionView, UncleBlockView,
        Version,
    },
    packed::{Byte32, Bytes, ProposalShortId},
};

#[derive(Clone)]
pub(crate) struct BlockTemplate {
    pub(crate) version: Version,
    pub(crate) compact_target: u32,
    pub(crate) number: BlockNumber,
    pub(crate) epoch: EpochNumberWithFraction,
    pub(crate) parent_hash: Byte32,
    pub(crate) cycles_limit: Cycle,
    pub(crate) bytes_limit: u64,
    pub(crate) uncles_count_limit: u8,

    // option
    pub(crate) uncles: Vec<UncleBlockView>,
    pub(crate) transactions: Vec<TxEntry>,
    pub(crate) proposals: Vec<ProposalShortId>,
    pub(crate) cellbase: TransactionView,
    pub(crate) work_id: u64,
    pub(crate) dao: Byte32,
    pub(crate) current_time: u64,
    pub(crate) extension: Option<Bytes>,
}

#[derive(Clone)]
struct TemplateParts {
    version: Version,
    compact_target: u32,
    number: BlockNumber,
    epoch: EpochNumberWithFraction,
    parent_hash: Byte32,
    cycles_limit: Cycle,
    bytes_limit: u64,
    uncles_count_limit: u8,
    uncles: Vec<UncleBlockView>,
    transactions: Vec<TxEntry>,
    proposals: Vec<ProposalShortId>,
    extension: Option<Bytes>,
}

/// Initial template state. Required fields are arguments to `build`, so an
/// incomplete template cannot be represented or accidentally published.
pub(crate) struct BlockTemplateDraft {
    parts: TemplateParts,
}

#[derive(Clone)]
pub(crate) struct BlockTemplateBuilder {
    parts: TemplateParts,
    cellbase: TransactionView,
    work_id: u64,
    dao: Byte32,
    current_time: u64,
}

impl BlockTemplateDraft {
    pub(crate) fn new(
        snapshot: &Snapshot,
        current_epoch: &EpochExt,
    ) -> Result<Self, BlockAssemblerError> {
        let consensus = snapshot.consensus();
        let tip_header = snapshot.tip_header();
        let tip_hash = tip_header.hash();
        let candidate_number = tip_header
            .number()
            .checked_add(1)
            .ok_or(BlockAssemblerError::Overflow)?;

        let version = consensus.block_version();
        let max_block_bytes = consensus.max_block_bytes();
        let cycles_limit = consensus.max_block_cycles();
        let uncles_count_limit = consensus.max_uncles_num() as u8;

        Ok(Self {
            parts: TemplateParts {
                version,
                compact_target: current_epoch.compact_target(),
                number: candidate_number,
                epoch: current_epoch.number_with_fraction(candidate_number),
                parent_hash: tip_hash,
                cycles_limit,
                bytes_limit: max_block_bytes,
                uncles_count_limit,
                uncles: vec![],
                transactions: vec![],
                proposals: vec![],
                extension: None,
            },
        })
    }

    pub(crate) fn uncles(&mut self, uncles: impl IntoIterator<Item = UncleBlockView>) -> &mut Self {
        self.parts.uncles.extend(uncles);
        self
    }

    pub(crate) fn extension(&mut self, extension: Bytes) -> &mut Self {
        self.parts.extension = Some(extension);
        self
    }

    pub(crate) fn build(
        self,
        cellbase: TransactionView,
        work_id: u64,
        dao: Byte32,
        current_time: u64,
    ) -> BlockTemplate {
        BlockTemplateBuilder {
            parts: self.parts,
            cellbase,
            work_id,
            dao,
            current_time,
        }
        .build()
    }
}

impl BlockTemplateBuilder {
    pub(crate) fn from_template(template: &BlockTemplate) -> Self {
        Self {
            parts: TemplateParts {
                version: template.version,
                compact_target: template.compact_target,
                number: template.number,
                epoch: template.epoch,
                parent_hash: template.parent_hash.clone(),
                cycles_limit: template.cycles_limit,
                bytes_limit: template.bytes_limit,
                uncles_count_limit: template.uncles_count_limit,
                extension: template.extension.clone(),
                uncles: template.uncles.clone(),
                transactions: template.transactions.clone(),
                proposals: template.proposals.clone(),
            },
            cellbase: template.cellbase.clone(),
            work_id: template.work_id,
            dao: template.dao.clone(),
            current_time: template.current_time,
        }
    }

    pub(crate) fn set_uncles(&mut self, uncles: Vec<UncleBlockView>) -> &mut Self {
        self.parts.uncles = uncles;
        self
    }

    pub(crate) fn set_transactions(&mut self, transactions: Vec<TxEntry>) -> &mut Self {
        self.parts.transactions = transactions;
        self
    }

    pub(crate) fn set_proposals(&mut self, proposals: Vec<ProposalShortId>) -> &mut Self {
        self.parts.proposals = proposals;
        self
    }

    pub(crate) fn work_id(&mut self, work_id: u64) -> &mut Self {
        self.work_id = work_id;
        self
    }

    pub(crate) fn dao(&mut self, dao: Byte32) -> &mut Self {
        self.dao = dao;
        self
    }

    pub(crate) fn current_time(&mut self, current_time: u64) -> &mut Self {
        self.current_time = current_time;
        self
    }

    pub(crate) fn build(self) -> BlockTemplate {
        let TemplateParts {
            version,
            compact_target,
            number,
            epoch,
            parent_hash,
            cycles_limit,
            bytes_limit,
            uncles_count_limit,
            uncles,
            transactions,
            proposals,
            extension,
        } = self.parts;
        BlockTemplate {
            version,
            compact_target,
            number,
            epoch,
            parent_hash,
            cycles_limit,
            bytes_limit,
            uncles_count_limit,
            uncles,
            transactions,
            proposals,
            cellbase: self.cellbase,
            work_id: self.work_id,
            dao: self.dao,
            current_time: self.current_time,
            extension,
        }
    }
}
