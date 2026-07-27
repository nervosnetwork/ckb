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

pub(crate) struct BlockTemplateBuilder {
    parts: TemplateParts,
    cellbase: TransactionView,
    work_id: u64,
    dao: Byte32,
    current_time: u64,
}

/// Complete content replacement scopes used by block-template workers.
///
/// Each variant carries every collection that its worker is allowed to
/// replace. `for_update` therefore clones only content that must survive the
/// update, while the type prevents a full rebuild from accidentally retaining
/// a stale collection.
pub(crate) enum TemplateContentUpdate {
    Full {
        uncles: Vec<UncleBlockView>,
        transactions: Vec<TxEntry>,
        proposals: Vec<ProposalShortId>,
        dao: Byte32,
    },
    Uncles {
        uncles: Vec<UncleBlockView>,
    },
    Proposals {
        uncles: Vec<UncleBlockView>,
        proposals: Vec<ProposalShortId>,
    },
    Transactions {
        transactions: Vec<TxEntry>,
        dao: Byte32,
    },
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
    pub(crate) fn for_update(template: &BlockTemplate, update: TemplateContentUpdate) -> Self {
        let (uncles, transactions, proposals, dao) = match update {
            TemplateContentUpdate::Full {
                uncles,
                transactions,
                proposals,
                dao,
            } => (uncles, transactions, proposals, dao),
            TemplateContentUpdate::Uncles { uncles } => (
                uncles,
                template.transactions.clone(),
                template.proposals.clone(),
                template.dao.clone(),
            ),
            TemplateContentUpdate::Proposals { uncles, proposals } => (
                uncles,
                template.transactions.clone(),
                proposals,
                template.dao.clone(),
            ),
            TemplateContentUpdate::Transactions { transactions, dao } => (
                template.uncles.clone(),
                transactions,
                template.proposals.clone(),
                dao,
            ),
        };
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
                uncles,
                transactions,
                proposals,
            },
            cellbase: template.cellbase.clone(),
            work_id: template.work_id,
            dao,
            current_time: template.current_time,
        }
    }

    pub(crate) fn work_id(&mut self, work_id: u64) -> &mut Self {
        self.work_id = work_id;
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
