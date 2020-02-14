use crate::bytes::JsonBytes;
use crate::{
    BlockNumber, Byte32, Capacity, EpochNumber, EpochNumberWithFraction, ProposalShortId,
    Timestamp, Uint128, Uint32, Uint64, Version,
};
use ckb_types::{core, packed, prelude::*, H256};
use serde::{Deserialize, Serialize};
use std::convert::TryFrom;
use std::fmt;

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
#[serde(rename_all = "snake_case")]
pub enum ScriptHashType {
    Data,
    Type,
}

impl Default for ScriptHashType {
    fn default() -> Self {
        ScriptHashType::Data
    }
}

impl From<ScriptHashType> for core::ScriptHashType {
    fn from(json: ScriptHashType) -> Self {
        match json {
            ScriptHashType::Data => core::ScriptHashType::Data,
            ScriptHashType::Type => core::ScriptHashType::Type,
        }
    }
}

impl From<core::ScriptHashType> for ScriptHashType {
    fn from(core: core::ScriptHashType) -> ScriptHashType {
        match core {
            core::ScriptHashType::Data => ScriptHashType::Data,
            core::ScriptHashType::Type => ScriptHashType::Type,
        }
    }
}

impl fmt::Display for ScriptHashType {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        match self {
            ScriptHashType::Data => write!(f, "data"),
            ScriptHashType::Type => write!(f, "type"),
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
#[serde(deny_unknown_fields)]
pub struct Script {
    pub code_hash: H256,
    pub hash_type: ScriptHashType,
    pub args: JsonBytes,
}

impl From<Script> for packed::Script {
    fn from(json: Script) -> Self {
        let Script {
            args,
            code_hash,
            hash_type,
        } = json;
        let hash_type: core::ScriptHashType = hash_type.into();
        packed::Script::new_builder()
            .args(args.into_bytes().pack())
            .code_hash(code_hash.pack())
            .hash_type(hash_type.into())
            .build()
    }
}

impl From<packed::Script> for Script {
    fn from(input: packed::Script) -> Script {
        Script {
            code_hash: input.code_hash().unpack(),
            args: JsonBytes::from_bytes(input.args().unpack()),
            hash_type: core::ScriptHashType::try_from(input.hash_type())
                .expect("checked data")
                .into(),
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
#[serde(deny_unknown_fields)]
pub struct CellOutput {
    pub capacity: Capacity,
    pub lock: Script,
    #[serde(rename = "type")]
    pub type_: Option<Script>,
}

impl From<packed::CellOutput> for CellOutput {
    fn from(input: packed::CellOutput) -> CellOutput {
        CellOutput {
            capacity: input.capacity().unpack(),
            lock: input.lock().into(),
            type_: input.type_().to_opt().map(Into::into),
        }
    }
}

impl From<CellOutput> for packed::CellOutput {
    fn from(json: CellOutput) -> Self {
        let CellOutput {
            capacity,
            lock,
            type_,
        } = json;
        let type_builder = packed::ScriptOpt::new_builder();
        let type_ = match type_ {
            Some(type_) => type_builder.set(Some(type_.into())),
            None => type_builder,
        }
        .build();
        packed::CellOutput::new_builder()
            .capacity(capacity.pack())
            .lock(lock.into())
            .type_(type_)
            .build()
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
#[serde(deny_unknown_fields)]
pub struct OutPoint {
    pub tx_hash: H256,
    pub index: Uint32,
}

impl From<packed::OutPoint> for OutPoint {
    fn from(input: packed::OutPoint) -> OutPoint {
        let index: u32 = input.index().unpack();
        OutPoint {
            tx_hash: input.tx_hash().unpack(),
            index: index.into(),
        }
    }
}

impl From<OutPoint> for packed::OutPoint {
    fn from(json: OutPoint) -> Self {
        let OutPoint { tx_hash, index } = json;
        let index = index.value() as u32;
        packed::OutPoint::new_builder()
            .tx_hash(tx_hash.pack())
            .index(index.pack())
            .build()
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
#[serde(deny_unknown_fields)]
pub struct CellInput {
    pub since: Uint64,
    pub previous_output: OutPoint,
}

impl From<packed::CellInput> for CellInput {
    fn from(input: packed::CellInput) -> CellInput {
        CellInput {
            previous_output: input.previous_output().into(),
            since: input.since().unpack(),
        }
    }
}

impl From<CellInput> for packed::CellInput {
    fn from(json: CellInput) -> Self {
        let CellInput {
            previous_output,
            since,
        } = json;
        packed::CellInput::new_builder()
            .previous_output(previous_output.into())
            .since(since.pack())
            .build()
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
#[serde(rename_all = "snake_case")]
pub enum DepType {
    Code,
    DepGroup,
}

impl Default for DepType {
    fn default() -> Self {
        DepType::Code
    }
}

impl From<DepType> for core::DepType {
    fn from(json: DepType) -> Self {
        match json {
            DepType::Code => core::DepType::Code,
            DepType::DepGroup => core::DepType::DepGroup,
        }
    }
}

impl From<core::DepType> for DepType {
    fn from(core: core::DepType) -> DepType {
        match core {
            core::DepType::Code => DepType::Code,
            core::DepType::DepGroup => DepType::DepGroup,
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
#[serde(deny_unknown_fields)]
pub struct CellDep {
    pub out_point: OutPoint,
    pub dep_type: DepType,
}

impl From<packed::CellDep> for CellDep {
    fn from(input: packed::CellDep) -> Self {
        CellDep {
            out_point: input.out_point().into(),
            dep_type: core::DepType::try_from(input.dep_type())
                .expect("checked data")
                .into(),
        }
    }
}

impl From<CellDep> for packed::CellDep {
    fn from(json: CellDep) -> Self {
        let CellDep {
            out_point,
            dep_type,
        } = json;
        let dep_type: core::DepType = dep_type.into();
        packed::CellDep::new_builder()
            .out_point(out_point.into())
            .dep_type(dep_type.into())
            .build()
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
#[serde(deny_unknown_fields)]
pub struct Transaction {
    pub version: Version,
    pub cell_deps: Vec<CellDep>,
    pub header_deps: Vec<H256>,
    pub inputs: Vec<CellInput>,
    pub outputs: Vec<CellOutput>,
    pub outputs_data: Vec<JsonBytes>,
    pub witnesses: Vec<JsonBytes>,
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct TransactionView {
    #[serde(flatten)]
    pub inner: Transaction,
    pub hash: H256,
}

impl From<packed::Transaction> for Transaction {
    fn from(input: packed::Transaction) -> Self {
        let raw = input.raw();
        Self {
            version: raw.version().unpack(),
            cell_deps: raw.cell_deps().into_iter().map(Into::into).collect(),
            header_deps: raw
                .header_deps()
                .into_iter()
                .map(|d| Unpack::<H256>::unpack(&d))
                .collect(),
            inputs: raw.inputs().into_iter().map(Into::into).collect(),
            outputs: raw.outputs().into_iter().map(Into::into).collect(),
            outputs_data: raw.outputs_data().into_iter().map(Into::into).collect(),
            witnesses: input.witnesses().into_iter().map(Into::into).collect(),
        }
    }
}

impl From<core::TransactionView> for TransactionView {
    fn from(input: core::TransactionView) -> Self {
        Self {
            inner: input.data().into(),
            hash: input.hash().unpack(),
        }
    }
}

impl From<Transaction> for packed::Transaction {
    fn from(json: Transaction) -> Self {
        let Transaction {
            version,
            cell_deps,
            header_deps,
            inputs,
            outputs,
            witnesses,
            outputs_data,
        } = json;
        let raw = packed::RawTransaction::new_builder()
            .version(version.pack())
            .cell_deps(cell_deps.into_iter().map(Into::into).pack())
            .header_deps(header_deps.iter().map(Pack::pack).pack())
            .inputs(inputs.into_iter().map(Into::into).pack())
            .outputs(outputs.into_iter().map(Into::into).pack())
            .outputs_data(outputs_data.into_iter().map(Into::into).pack())
            .build();
        packed::Transaction::new_builder()
            .raw(raw)
            .witnesses(witnesses.into_iter().map(Into::into).pack())
            .build()
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct TransactionWithStatus {
    pub transaction: TransactionView,
    /// Indicate the Transaction status
    pub tx_status: TxStatus,
}

impl TransactionWithStatus {
    /// Build with pending status
    pub fn with_pending(tx: core::TransactionView) -> Self {
        Self {
            tx_status: TxStatus::pending(),
            transaction: tx.into(),
        }
    }

    /// Build with proposed status
    pub fn with_proposed(tx: core::TransactionView) -> Self {
        Self {
            tx_status: TxStatus::proposed(),
            transaction: tx.into(),
        }
    }

    /// Build with committed status
    pub fn with_committed(tx: core::TransactionView, hash: H256) -> Self {
        Self {
            tx_status: TxStatus::committed(hash),
            transaction: tx.into(),
        }
    }
}

/// Status for transaction
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Transaction on pool, not proposed
    Pending,
    /// Transaction on pool, proposed
    Proposed,
    /// Transaction commit on block
    Committed,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct TxStatus {
    pub status: Status,
    pub block_hash: Option<H256>,
}

impl TxStatus {
    pub fn pending() -> Self {
        Self {
            status: Status::Pending,
            block_hash: None,
        }
    }

    pub fn proposed() -> Self {
        Self {
            status: Status::Proposed,
            block_hash: None,
        }
    }

    pub fn committed(hash: H256) -> Self {
        Self {
            status: Status::Committed,
            block_hash: Some(hash),
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
#[serde(deny_unknown_fields)]
pub struct Header {
    pub version: Version,
    pub compact_target: Uint32,
    pub timestamp: Timestamp,
    pub number: BlockNumber,
    pub epoch: EpochNumberWithFraction,
    pub parent_hash: H256,
    pub transactions_root: H256,
    pub proposals_hash: H256,
    pub uncles_hash: H256,
    pub dao: Byte32,
    pub nonce: Uint128,
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct HeaderView {
    #[serde(flatten)]
    pub inner: Header,
    pub hash: H256,
}

impl From<packed::Header> for Header {
    fn from(input: packed::Header) -> Self {
        let raw = input.raw();
        Self {
            version: raw.version().unpack(),
            parent_hash: raw.parent_hash().unpack(),
            timestamp: raw.timestamp().unpack(),
            number: raw.number().unpack(),
            epoch: raw.epoch().unpack(),
            transactions_root: raw.transactions_root().unpack(),
            proposals_hash: raw.proposals_hash().unpack(),
            compact_target: raw.compact_target().unpack(),
            uncles_hash: raw.uncles_hash().unpack(),
            dao: raw.dao().into(),
            nonce: input.nonce().unpack(),
        }
    }
}

impl From<core::HeaderView> for HeaderView {
    fn from(input: core::HeaderView) -> Self {
        Self {
            inner: input.data().into(),
            hash: input.hash().unpack(),
        }
    }
}

impl From<HeaderView> for core::HeaderView {
    fn from(input: HeaderView) -> Self {
        let header: packed::Header = input.inner.into();
        header.into_view()
    }
}

impl From<Header> for packed::Header {
    fn from(json: Header) -> Self {
        let Header {
            version,
            parent_hash,
            timestamp,
            number,
            epoch,
            transactions_root,
            proposals_hash,
            compact_target,
            uncles_hash,
            dao,
            nonce,
        } = json;
        let raw = packed::RawHeader::new_builder()
            .version(version.pack())
            .parent_hash(parent_hash.pack())
            .timestamp(timestamp.pack())
            .number(number.pack())
            .epoch(epoch.pack())
            .transactions_root(transactions_root.pack())
            .proposals_hash(proposals_hash.pack())
            .compact_target(compact_target.pack())
            .uncles_hash(uncles_hash.pack())
            .dao(dao.into())
            .build();
        packed::Header::new_builder()
            .raw(raw)
            .nonce(nonce.pack())
            .build()
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
#[serde(deny_unknown_fields)]
pub struct UncleBlock {
    pub header: Header,
    pub proposals: Vec<ProposalShortId>,
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct UncleBlockView {
    pub header: HeaderView,
    pub proposals: Vec<ProposalShortId>,
}

impl From<packed::UncleBlock> for UncleBlock {
    fn from(input: packed::UncleBlock) -> Self {
        Self {
            header: input.header().into(),
            proposals: input.proposals().into_iter().map(Into::into).collect(),
        }
    }
}

impl From<core::UncleBlockView> for UncleBlockView {
    fn from(input: core::UncleBlockView) -> Self {
        let header = HeaderView {
            inner: input.data().header().into(),
            hash: input.hash().unpack(),
        };
        Self {
            header,
            proposals: input
                .data()
                .proposals()
                .into_iter()
                .map(Into::into)
                .collect(),
        }
    }
}

impl From<UncleBlock> for packed::UncleBlock {
    fn from(json: UncleBlock) -> Self {
        let UncleBlock { header, proposals } = json;
        packed::UncleBlock::new_builder()
            .header(header.into())
            .proposals(proposals.into_iter().map(Into::into).pack())
            .build()
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
#[serde(deny_unknown_fields)]
pub struct Block {
    pub header: Header,
    pub uncles: Vec<UncleBlock>,
    pub transactions: Vec<Transaction>,
    pub proposals: Vec<ProposalShortId>,
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct BlockView {
    pub header: HeaderView,
    pub uncles: Vec<UncleBlockView>,
    pub transactions: Vec<TransactionView>,
    pub proposals: Vec<ProposalShortId>,
}

impl From<packed::Block> for Block {
    fn from(input: packed::Block) -> Self {
        Self {
            header: input.header().into(),
            uncles: input.uncles().into_iter().map(Into::into).collect(),
            transactions: input.transactions().into_iter().map(Into::into).collect(),
            proposals: input.proposals().into_iter().map(Into::into).collect(),
        }
    }
}

impl From<core::BlockView> for BlockView {
    fn from(input: core::BlockView) -> Self {
        let block = input.data();
        let header = HeaderView {
            inner: block.header().into(),
            hash: input.hash().unpack(),
        };
        let uncles = block
            .uncles()
            .into_iter()
            .zip(input.uncle_hashes().into_iter())
            .map(|(uncle, hash)| {
                let header = HeaderView {
                    inner: uncle.header().into(),
                    hash: hash.unpack(),
                };
                UncleBlockView {
                    header,
                    proposals: uncle.proposals().into_iter().map(Into::into).collect(),
                }
            })
            .collect();
        let transactions = block
            .transactions()
            .into_iter()
            .zip(input.tx_hashes().iter())
            .map(|(tx, hash)| TransactionView {
                inner: tx.into(),
                hash: hash.unpack(),
            })
            .collect();
        Self {
            header,
            uncles,
            transactions,
            proposals: block.proposals().into_iter().map(Into::into).collect(),
        }
    }
}

impl From<Block> for packed::Block {
    fn from(json: Block) -> Self {
        let Block {
            header,
            uncles,
            transactions,
            proposals,
        } = json;
        packed::Block::new_builder()
            .header(header.into())
            .uncles(uncles.into_iter().map(Into::into).pack())
            .transactions(transactions.into_iter().map(Into::into).pack())
            .proposals(proposals.into_iter().map(Into::into).pack())
            .build()
    }
}

impl From<BlockView> for core::BlockView {
    fn from(input: BlockView) -> Self {
        let BlockView {
            header,
            uncles,
            transactions,
            proposals,
        } = input;
        let block = Block {
            header: header.inner,
            uncles: uncles
                .into_iter()
                .map(|u| {
                    let UncleBlockView { header, proposals } = u;
                    UncleBlock {
                        header: header.inner,
                        proposals,
                    }
                })
                .collect(),
            transactions: transactions.into_iter().map(|tx| tx.inner).collect(),
            proposals,
        };
        let block: packed::Block = block.into();
        block.into_view()
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct EpochView {
    pub number: EpochNumber,
    pub start_number: BlockNumber,
    pub length: BlockNumber,
    pub compact_target: Uint32,
}

impl EpochView {
    pub fn from_ext(ext: packed::EpochExt) -> EpochView {
        EpochView {
            number: ext.number().unpack(),
            start_number: ext.start_number().unpack(),
            length: ext.length().unpack(),
            compact_target: ext.compact_target().unpack(),
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct BlockReward {
    pub total: Capacity,
    pub primary: Capacity,
    pub secondary: Capacity,
    pub tx_fee: Capacity,
    pub proposal_reward: Capacity,
}

impl From<core::BlockReward> for BlockReward {
    fn from(core: core::BlockReward) -> Self {
        Self {
            total: core.total.into(),
            primary: core.primary.into(),
            secondary: core.secondary.into(),
            tx_fee: core.tx_fee.into(),
            proposal_reward: core.proposal_reward.into(),
        }
    }
}

impl From<BlockReward> for core::BlockReward {
    fn from(json: BlockReward) -> Self {
        Self {
            total: json.total.into(),
            primary: json.primary.into(),
            secondary: json.secondary.into(),
            tx_fee: json.tx_fee.into(),
            proposal_reward: json.proposal_reward.into(),
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct BlockIssuance {
    pub primary: Capacity,
    pub secondary: Capacity,
}

impl From<core::BlockIssuance> for BlockIssuance {
    fn from(core: core::BlockIssuance) -> Self {
        Self {
            primary: core.primary.into(),
            secondary: core.secondary.into(),
        }
    }
}

impl From<BlockIssuance> for core::BlockIssuance {
    fn from(json: BlockIssuance) -> Self {
        Self {
            primary: json.primary.into(),
            secondary: json.secondary.into(),
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct MinerReward {
    pub primary: Capacity,
    pub secondary: Capacity,
    pub committed: Capacity,
    pub proposal: Capacity,
}

impl From<core::MinerReward> for MinerReward {
    fn from(core: core::MinerReward) -> Self {
        Self {
            primary: core.primary.into(),
            secondary: core.secondary.into(),
            committed: core.committed.into(),
            proposal: core.proposal.into(),
        }
    }
}

impl From<MinerReward> for core::MinerReward {
    fn from(json: MinerReward) -> Self {
        Self {
            primary: json.primary.into(),
            secondary: json.secondary.into(),
            committed: json.committed.into(),
            proposal: json.proposal.into(),
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct BlockEconomicState {
    pub issuance: BlockIssuance,
    pub miner_reward: MinerReward,
    pub txs_fee: Capacity,
    pub finalized_at: H256,
}

impl From<core::BlockEconomicState> for BlockEconomicState {
    fn from(core: core::BlockEconomicState) -> Self {
        Self {
            issuance: core.issuance.into(),
            miner_reward: core.miner_reward.into(),
            txs_fee: core.txs_fee.into(),
            finalized_at: core.finalized_at.unpack(),
        }
    }
}

impl From<BlockEconomicState> for core::BlockEconomicState {
    fn from(json: BlockEconomicState) -> Self {
        Self {
            issuance: json.issuance.into(),
            miner_reward: json.miner_reward.into(),
            txs_fee: json.txs_fee.into(),
            finalized_at: json.finalized_at.pack(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_types::{bytes::Bytes, core::TransactionBuilder, packed::Byte32};
    use lazy_static::lazy_static;
    use proptest::{collection::size_range, prelude::*};
    use regex::Regex;

    fn mock_script(arg: Bytes) -> packed::Script {
        packed::ScriptBuilder::default()
            .code_hash(Byte32::zero())
            .args(arg.pack())
            .hash_type(core::ScriptHashType::Data.into())
            .build()
    }

    fn mock_cell_output(arg: Bytes) -> packed::CellOutput {
        packed::CellOutputBuilder::default()
            .capacity(core::Capacity::zero().pack())
            .lock(packed::Script::default())
            .type_(Some(mock_script(arg)).pack())
            .build()
    }

    fn mock_cell_input() -> packed::CellInput {
        packed::CellInput::new(packed::OutPoint::default(), 0)
    }

    fn mock_full_tx(data: Bytes, arg: Bytes) -> core::TransactionView {
        TransactionBuilder::default()
            .inputs(vec![mock_cell_input()])
            .outputs(vec![mock_cell_output(arg.clone())])
            .outputs_data(vec![data.pack()])
            .witness(arg.pack())
            .build()
    }

    fn mock_uncle() -> core::UncleBlockView {
        core::BlockBuilder::default()
            .proposals(vec![packed::ProposalShortId::default()].pack())
            .build()
            .as_uncle()
    }

    fn mock_full_block(data: Bytes, arg: Bytes) -> core::BlockView {
        core::BlockBuilder::default()
            .transactions(vec![mock_full_tx(data, arg)])
            .uncles(vec![mock_uncle()])
            .proposals(vec![packed::ProposalShortId::default()])
            .build()
    }

    fn _test_block_convert(data: Bytes, arg: Bytes) -> Result<(), TestCaseError> {
        let block = mock_full_block(data, arg);
        let json_block: BlockView = block.clone().into();
        let encoded = serde_json::to_string(&json_block).unwrap();
        let decode: BlockView = serde_json::from_str(&encoded).unwrap();
        let decode_block: core::BlockView = decode.into();
        header_field_format_check(&encoded);
        prop_assert_eq!(decode_block.data(), block.data());
        prop_assert_eq!(decode_block, block);
        Ok(())
    }

    fn header_field_format_check(json: &str) {
        lazy_static! {
            static ref RE: Regex = Regex::new("\"(version|compact_target|parent_hash|timestamp|number|epoch|transactions_root|proposals_hash|uncles_hash|dao|nonce)\":\"(?P<value>.*?\")").unwrap();
        }
        for caps in RE.captures_iter(json) {
            assert!(&caps["value"].starts_with("0x"));
        }
    }

    proptest! {
        #[test]
        fn test_block_convert(
            data in any_with::<Vec<u8>>(size_range(80).lift()),
            arg in any_with::<Vec<u8>>(size_range(80).lift()),
        ) {
            _test_block_convert(Bytes::from(data), Bytes::from(arg))?;
        }
    }
}
