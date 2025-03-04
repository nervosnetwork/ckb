use crate::{
    BlockNumber, Byte32, Cycle, EpochNumberWithFraction, Header, JsonBytes, ProposalShortId,
    Timestamp, Transaction, Uint32, Uint64, Version,
};
use ckb_types::{H256, packed, prelude::*};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::convert::From;

/// A block template for miners.
///
/// Miners optional pick transactions and then assemble the final block.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug, JsonSchema)]
pub struct BlockTemplate {
    /// Block version.
    ///
    /// Miners must use it unchanged in the assembled block.
    pub version: Version,
    /// The compacted difficulty target for the new block.
    ///
    /// Miners must use it unchanged in the assembled block.
    pub compact_target: Uint32,
    /// The timestamp for the new block.
    ///
    /// CKB node guarantees that this timestamp is larger than the median of the previous 37 blocks.
    ///
    /// Miners can increase it to the current time. It is not recommended to decrease it, since it may violate the median block timestamp consensus rule.
    pub current_time: Timestamp,
    /// The block number for the new block.
    ///
    /// Miners must use it unchanged in the assembled block.
    pub number: BlockNumber,
    /// The epoch progress information for the new block.
    ///
    /// Miners must use it unchanged in the assembled block.
    pub epoch: EpochNumberWithFraction,
    /// The parent block hash of the new block.
    ///
    /// Miners must use it unchanged in the assembled block.
    pub parent_hash: H256,
    /// The cycles limit.
    ///
    /// Miners must keep the total cycles below this limit, otherwise, the CKB node will reject the block
    /// submission.
    ///
    /// It is guaranteed that the block does not exceed the limit if miners do not add new
    /// transactions to the block.
    pub cycles_limit: Cycle,
    /// The block serialized size limit.
    ///
    /// Miners must keep the block size below this limit, otherwise, the CKB node will reject the block
    /// submission.
    ///
    /// It is guaranteed that the block does not exceed the limit if miners do not add new
    /// transaction commitments.
    pub bytes_limit: Uint64,
    /// The uncle count limit.
    ///
    /// Miners must keep the uncles count below this limit, otherwise, the CKB node will reject the
    /// block submission.
    pub uncles_count_limit: Uint64,
    /// Provided valid uncle blocks candidates for the new block.
    ///
    /// Miners must include the uncles marked as `required` in the assembled new block.
    pub uncles: Vec<UncleTemplate>,
    /// Provided valid transactions which can be committed in the new block.
    ///
    /// Miners must include the transactions marked as `required` in the assembled new block.
    pub transactions: Vec<TransactionTemplate>,
    /// Provided proposal ids list of transactions for the new block.
    pub proposals: Vec<ProposalShortId>,
    /// Provided cellbase transaction template.
    ///
    /// Miners must use it as the cellbase transaction without changes in the assembled block.
    pub cellbase: CellbaseTemplate,
    /// Work ID. The miner must submit the new assembled and resolved block using the same work ID.
    pub work_id: Uint64,
    /// Reference DAO field.
    ///
    /// This field is only valid when miners use all and only use the provided transactions in the
    /// template. Two fields must be updated when miners want to select transactions:
    ///
    /// * `S_i`, bytes 16 to 23
    /// * `U_i`, bytes 24 to 31
    ///
    /// See RFC [Deposit and Withdraw in Nervos DAO](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0023-dao-deposit-withdraw/0023-dao-deposit-withdraw.md#calculation).
    pub dao: Byte32,
    /// The extension for the new block.
    ///
    /// This is a field introduced in [CKB RFC 0031]. Since the activation of [CKB RFC 0044], this
    /// field is at least 32 bytes, and at most 96 bytes. The consensus rule of first 32 bytes is
    /// defined in the RFC 0044.
    ///
    /// [CKB RFC 0031]: https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0031-variable-length-header-field/0031-variable-length-header-field.md
    /// [CKB RFC 0044]: https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0044-ckb-light-client/0044-ckb-light-client.md
    #[serde(default)]
    pub extension: Option<JsonBytes>,
}

impl From<BlockTemplate> for packed::Block {
    fn from(block_template: BlockTemplate) -> packed::Block {
        let BlockTemplate {
            version,
            compact_target,
            current_time,
            number,
            epoch,
            parent_hash,
            uncles,
            transactions,
            proposals,
            cellbase,
            dao,
            extension,
            ..
        } = block_template;
        let raw = packed::RawHeader::new_builder()
            .version(version.pack())
            .compact_target(compact_target.pack())
            .parent_hash(parent_hash.pack())
            .timestamp(current_time.pack())
            .number(number.pack())
            .epoch(epoch.pack())
            .dao(dao.into())
            .build();
        let header = packed::Header::new_builder().raw(raw).build();
        let txs = packed::TransactionVec::new_builder()
            .push(cellbase.into())
            .extend(transactions.into_iter().map(|tx| tx.into()))
            .build();
        if let Some(extension) = extension {
            let extension: packed::Bytes = extension.into();
            packed::BlockV1::new_builder()
                .header(header)
                .uncles(
                    uncles
                        .into_iter()
                        .map(|u| u.into())
                        .collect::<Vec<packed::UncleBlock>>()
                        .pack(),
                )
                .transactions(txs)
                .proposals(
                    proposals
                        .into_iter()
                        .map(|p| p.into())
                        .collect::<Vec<packed::ProposalShortId>>()
                        .pack(),
                )
                .extension(extension)
                .build()
                .as_v0()
        } else {
            packed::Block::new_builder()
                .header(header)
                .uncles(
                    uncles
                        .into_iter()
                        .map(|u| u.into())
                        .collect::<Vec<packed::UncleBlock>>()
                        .pack(),
                )
                .transactions(txs)
                .proposals(
                    proposals
                        .into_iter()
                        .map(|p| p.into())
                        .collect::<Vec<packed::ProposalShortId>>()
                        .pack(),
                )
                .build()
        }
        .reset_header()
    }
}

/// The uncle block template of the new block for miners.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug, JsonSchema)]
pub struct UncleTemplate {
    /// The uncle block hash.
    pub hash: H256,
    /// Whether miners must include this uncle in the submit block.
    pub required: bool,
    /// The proposals of the uncle block.
    ///
    /// Miners must keep this unchanged when including this uncle in the new block.
    pub proposals: Vec<ProposalShortId>,
    /// The header of the uncle block.
    ///
    /// Miners must keep this unchanged when including this uncle in the new block.
    pub header: Header,
}

impl From<UncleTemplate> for packed::UncleBlock {
    fn from(template: UncleTemplate) -> Self {
        let UncleTemplate {
            proposals, header, ..
        } = template;
        packed::UncleBlock::new_builder()
            .header(header.into())
            .proposals(proposals.into_iter().map(Into::into).pack())
            .build()
    }
}

/// The cellbase transaction template of the new block for miners.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug, JsonSchema)]
pub struct CellbaseTemplate {
    /// The cellbase transaction hash.
    pub hash: H256,
    /// The hint of how many cycles this transaction consumes.
    ///
    /// Miners can utilize this field to ensure that the total cycles do not
    /// exceed the limit while selecting transactions.
    pub cycles: Option<Cycle>,
    /// The cellbase transaction.
    pub data: Transaction,
}

impl From<CellbaseTemplate> for packed::Transaction {
    fn from(template: CellbaseTemplate) -> Self {
        let CellbaseTemplate { data, .. } = template;
        data.into()
    }
}

/// Transaction template which is ready to be committed in the new block.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug, JsonSchema)]
pub struct TransactionTemplate {
    /// Transaction hash.
    pub hash: H256,
    /// Whether miner must include this transaction in the new block.
    pub required: bool,
    /// The hint of how many cycles this transaction consumes.
    ///
    /// Miners can utilize this field to ensure that the total cycles do not
    /// exceed the limit while selecting transactions.
    pub cycles: Option<Cycle>,
    /// Transaction dependencies.
    ///
    /// This is a hint to help miners selecting transactions.
    ///
    /// This transaction can only be committed if its dependencies are also committed in the new block.
    ///
    /// This field is a list of indices into the array `transactions` in the block template.
    ///
    /// For example, `depends = [1, 2]` means this transaction depends on
    /// `block_template.transactions[1]` and `block_template.transactions[2]`.
    pub depends: Option<Vec<Uint64>>,
    /// The transaction.
    ///
    /// Miners must keep it unchanged when including it in the new block.
    pub data: Transaction,
}

impl From<TransactionTemplate> for packed::Transaction {
    fn from(template: TransactionTemplate) -> Self {
        let TransactionTemplate { data, .. } = template;
        data.into()
    }
}
