//! Complete block template, published state and JSON-RPC conversion.

use crate::component::entry::TxEntry;
use crate::error::BlockAssemblerError;
use ckb_jsonrpc_types::{
    BlockTemplate as JsonBlockTemplate, CellbaseTemplate, TransactionTemplate, UncleTemplate,
};
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
    pub(crate) uncles_count_limit: u64,

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

impl BlockTemplate {
    pub(crate) fn new(
        snapshot: &Snapshot,
        current_epoch: &EpochExt,
        cellbase: TransactionView,
        work_id: u64,
        dao: Byte32,
        current_time: u64,
    ) -> Result<Self, BlockAssemblerError> {
        let consensus = snapshot.consensus();
        let candidate_number = snapshot
            .tip_header()
            .number()
            .checked_add(1)
            .ok_or(BlockAssemblerError::Overflow)?;
        let uncles_count_limit =
            u64::try_from(consensus.max_uncles_num()).map_err(|_| BlockAssemblerError::Overflow)?;
        Ok(Self {
            version: consensus.block_version(),
            compact_target: current_epoch.compact_target(),
            number: candidate_number,
            epoch: current_epoch.number_with_fraction(candidate_number),
            parent_hash: snapshot.tip_hash(),
            cycles_limit: consensus.max_block_cycles(),
            bytes_limit: consensus.max_block_bytes(),
            uncles_count_limit,
            uncles: Vec::new(),
            transactions: Vec::new(),
            proposals: Vec::new(),
            cellbase,
            work_id,
            dao,
            current_time,
            extension: None,
        })
    }
}

pub(crate) struct CurrentTemplate {
    pub(crate) template: BlockTemplate,
    pub(crate) source: Option<crate::authority::TemplateSource>,
}

impl<'a> From<&'a BlockTemplate> for JsonBlockTemplate {
    fn from(template: &'a BlockTemplate) -> JsonBlockTemplate {
        JsonBlockTemplate {
            version: template.version.into(),
            compact_target: template.compact_target.into(),
            number: template.number.into(),
            epoch: template.epoch.into(),
            parent_hash: (&template.parent_hash).into(),
            cycles_limit: template.cycles_limit.into(),
            bytes_limit: template.bytes_limit.into(),
            uncles_count_limit: template.uncles_count_limit.into(),
            uncles: template.uncles.iter().map(uncle_to_template).collect(),
            transactions: template
                .transactions
                .iter()
                .map(tx_entry_to_template)
                .collect(),
            proposals: template.proposals.iter().map(Into::into).collect(),
            cellbase: cellbase_to_template(&template.cellbase),
            work_id: template.work_id.into(),
            dao: template.dao.clone().into(),
            current_time: template.current_time.into(),
            extension: template.extension.as_ref().map(Into::into),
        }
    }
}

pub(crate) fn uncle_to_template(uncle: &UncleBlockView) -> UncleTemplate {
    UncleTemplate {
        hash: uncle.hash().into(),
        required: false,
        proposals: uncle
            .data()
            .proposals()
            .into_iter()
            .map(Into::into)
            .collect(),
        header: uncle.data().header().into(),
    }
}

pub(crate) fn tx_entry_to_template(entry: &TxEntry) -> TransactionTemplate {
    TransactionTemplate {
        hash: entry.transaction().hash().into(),
        required: false, // not supported by CKB
        cycles: Some(entry.cycles.into()),
        depends: None, // not supported by CKB
        data: entry.transaction().data().into(),
    }
}

pub(crate) fn cellbase_to_template(tx: &TransactionView) -> CellbaseTemplate {
    CellbaseTemplate {
        hash: tx.hash().into(),
        cycles: None,
        data: tx.data().into(),
    }
}
