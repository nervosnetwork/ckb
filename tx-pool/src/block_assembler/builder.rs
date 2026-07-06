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
pub(crate) struct BlockTemplateBuilder {
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
    pub(crate) cellbase: Option<TransactionView>,
    pub(crate) work_id: Option<u64>,
    pub(crate) dao: Option<Byte32>,
    pub(crate) current_time: Option<u64>,
    pub(crate) extension: Option<Bytes>,
}

impl BlockTemplateBuilder {
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
            version,
            compact_target: current_epoch.compact_target(),

            number: candidate_number,
            epoch: current_epoch.number_with_fraction(candidate_number),
            parent_hash: tip_hash,
            cycles_limit,
            bytes_limit: max_block_bytes,
            uncles_count_limit,
            // option
            uncles: vec![],
            transactions: vec![],
            proposals: vec![],
            cellbase: None,
            work_id: None,
            dao: None,
            current_time: None,
            extension: None,
        })
    }

    pub(crate) fn from_template(template: &BlockTemplate) -> Self {
        Self {
            version: template.version,
            compact_target: template.compact_target,
            number: template.number,
            epoch: template.epoch,
            parent_hash: template.parent_hash.clone(),
            cycles_limit: template.cycles_limit,
            bytes_limit: template.bytes_limit,
            uncles_count_limit: template.uncles_count_limit,
            extension: template.extension.clone(),
            // option
            uncles: template.uncles.clone(),
            transactions: template.transactions.clone(),
            proposals: template.proposals.clone(),
            cellbase: Some(template.cellbase.clone()),
            work_id: None,
            dao: Some(template.dao.clone()),
            current_time: None,
        }
    }

    pub(crate) fn uncles(&mut self, uncles: impl IntoIterator<Item = UncleBlockView>) -> &mut Self {
        self.uncles.extend(uncles);
        self
    }

    pub(crate) fn set_uncles(&mut self, uncles: Vec<UncleBlockView>) -> &mut Self {
        self.uncles = uncles;
        self
    }

    pub(crate) fn transactions(
        &mut self,
        transactions: impl IntoIterator<Item = TxEntry>,
    ) -> &mut Self {
        self.transactions.extend(transactions);
        self
    }

    pub(crate) fn set_transactions(&mut self, transactions: Vec<TxEntry>) -> &mut Self {
        self.transactions = transactions;
        self
    }

    pub(crate) fn proposals(
        &mut self,
        proposals: impl IntoIterator<Item = ProposalShortId>,
    ) -> &mut Self {
        self.proposals.extend(proposals);
        self
    }

    pub(crate) fn set_proposals(&mut self, proposals: Vec<ProposalShortId>) -> &mut Self {
        self.proposals = proposals;
        self
    }

    pub(crate) fn cellbase(&mut self, cellbase: TransactionView) -> &mut Self {
        self.cellbase = Some(cellbase);
        self
    }

    pub(crate) fn work_id(&mut self, work_id: u64) -> &mut Self {
        self.work_id = Some(work_id);
        self
    }

    pub(crate) fn dao(&mut self, dao: Byte32) -> &mut Self {
        self.dao = Some(dao);
        self
    }

    pub(crate) fn current_time(&mut self, current_time: u64) -> &mut Self {
        self.current_time = Some(current_time);
        self
    }

    pub(crate) fn extension(&mut self, extension: Bytes) -> &mut Self {
        self.extension = Some(extension);
        self
    }

    pub(crate) fn build(self) -> BlockTemplate {
        assert!(self.cellbase.is_some(), "cellbase must be set");
        assert!(self.work_id.is_some(), "work_id must be set");
        assert!(self.current_time.is_some(), "current_time must be set");
        assert!(self.dao.is_some(), "dao must be set");

        BlockTemplate {
            version: self.version,
            compact_target: self.compact_target,

            number: self.number,
            epoch: self.epoch,
            parent_hash: self.parent_hash,
            cycles_limit: self.cycles_limit,
            bytes_limit: self.bytes_limit,
            uncles_count_limit: self.uncles_count_limit,
            uncles: self.uncles,
            transactions: self.transactions,
            proposals: self.proposals,
            cellbase: self.cellbase.expect("cellbase assert checked"),
            work_id: self.work_id.expect("work_id assert checked"),
            dao: self.dao.expect("dao assert checked"),
            current_time: self.current_time.expect("current_time assert checked"),
            extension: self.extension,
        }
    }
}
