//! Conversion from the internal [`BlockTemplate`] to the JSON-RPC type.

use crate::block_assembler::builder::BlockTemplate;
use crate::component::entry::TxEntry;
use ckb_jsonrpc_types::{
    BlockTemplate as JsonBlockTemplate, CellbaseTemplate, TransactionTemplate, UncleTemplate,
};
use ckb_types::core::{TransactionView, UncleBlockView};

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
            uncles_count_limit: u64::from(template.uncles_count_limit).into(),
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
