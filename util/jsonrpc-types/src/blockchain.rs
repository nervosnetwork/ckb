use crate::bytes::JsonBytes;
use crate::{BlockNumber, Capacity, EpochNumber, ProposalShortId, Timestamp, Unsigned, Version};
use ckb_core::block::{Block as CoreBlock, BlockBuilder};
use ckb_core::extras::EpochExt as CoreEpochExt;
use ckb_core::header::{Header as CoreHeader, HeaderBuilder, Seal as CoreSeal};
use ckb_core::script::Script as CoreScript;
use ckb_core::transaction::{
    CellInput as CoreCellInput, CellOutPoint as CoreCellOutPoint, CellOutput as CoreCellOutput,
    OutPoint as CoreOutPoint, Transaction as CoreTransaction, TransactionBuilder,
    Witness as CoreWitness,
};
use ckb_core::uncle::UncleBlock as CoreUncleBlock;
use ckb_core::Capacity as CoreCapacity;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use serde_derive::{Deserialize, Serialize};

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct Script {
    pub args: Vec<JsonBytes>,
    pub code_hash: H256,
}

impl From<Script> for CoreScript {
    fn from(json: Script) -> Self {
        let Script { args, code_hash } = json;
        CoreScript::new(
            args.into_iter().map(JsonBytes::into_bytes).collect(),
            code_hash,
        )
    }
}

impl From<CoreScript> for Script {
    fn from(core: CoreScript) -> Script {
        let (args, code_hash) = core.destruct();
        Script {
            code_hash,
            args: args.into_iter().map(JsonBytes::from_bytes).collect(),
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct CellOutput {
    pub capacity: Capacity,
    pub data: JsonBytes,
    pub lock: Script,
    #[serde(rename = "type")]
    pub type_: Option<Script>,
}

impl From<CoreCellOutput> for CellOutput {
    fn from(core: CoreCellOutput) -> CellOutput {
        let (capacity, data, lock, type_) = core.destruct();
        CellOutput {
            capacity: Capacity(capacity),
            data: JsonBytes::from_bytes(data),
            lock: lock.into(),
            type_: type_.map(Into::into),
        }
    }
}

impl From<CellOutput> for CoreCellOutput {
    fn from(json: CellOutput) -> Self {
        let CellOutput {
            capacity,
            data,
            lock,
            type_,
        } = json;

        let type_ = match type_ {
            Some(type_) => Some(type_.into()),
            None => None,
        };

        CoreCellOutput::new(capacity.0, data.into_bytes(), lock.into(), type_)
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct CellOutPoint {
    pub tx_hash: H256,
    pub index: Unsigned,
}

impl From<CoreCellOutPoint> for CellOutPoint {
    fn from(core: CoreCellOutPoint) -> CellOutPoint {
        let (tx_hash, index) = core.destruct();
        CellOutPoint {
            tx_hash,
            index: Unsigned(u64::from(index)),
        }
    }
}

impl From<CellOutPoint> for CoreCellOutPoint {
    fn from(json: CellOutPoint) -> Self {
        let CellOutPoint { tx_hash, index } = json;
        CoreCellOutPoint {
            tx_hash,
            index: index.0 as u32,
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct OutPoint {
    pub cell: Option<CellOutPoint>,
    pub block_hash: Option<H256>,
}

impl From<CoreOutPoint> for OutPoint {
    fn from(core: CoreOutPoint) -> OutPoint {
        let (block_hash, cell) = core.destruct();
        OutPoint {
            cell: cell.map(Into::into),
            block_hash: block_hash.map(Into::into),
        }
    }
}

impl From<OutPoint> for CoreOutPoint {
    fn from(json: OutPoint) -> Self {
        let OutPoint { cell, block_hash } = json;
        CoreOutPoint {
            cell: cell.map(Into::into),
            block_hash,
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct CellInput {
    pub previous_output: OutPoint,
    pub since: Unsigned,
}

impl From<CoreCellInput> for CellInput {
    fn from(core: CoreCellInput) -> CellInput {
        let (previous_output, since) = core.destruct();
        CellInput {
            previous_output: previous_output.into(),
            since: Unsigned(since),
        }
    }
}

impl From<CellInput> for CoreCellInput {
    fn from(json: CellInput) -> Self {
        let CellInput {
            previous_output,
            since,
        } = json;
        CoreCellInput::new(previous_output.into(), since.0)
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct Witness {
    data: Vec<JsonBytes>,
}

impl<'a> From<&'a CoreWitness> for Witness {
    fn from(core: &CoreWitness) -> Witness {
        Witness {
            data: core.iter().cloned().map(JsonBytes::from_bytes).collect(),
        }
    }
}

impl From<Witness> for CoreWitness {
    fn from(json: Witness) -> Self {
        json.data.into_iter().map(JsonBytes::into_bytes).collect()
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct Transaction {
    pub version: Version,
    pub deps: Vec<OutPoint>,
    pub inputs: Vec<CellInput>,
    pub outputs: Vec<CellOutput>,
    pub witnesses: Vec<Witness>,
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct TransactionView {
    #[serde(flatten)]
    pub inner: Transaction,
    pub hash: H256,
}

impl<'a> From<&'a CoreTransaction> for Transaction {
    fn from(core: &CoreTransaction) -> Self {
        Self {
            version: Version(core.version()),
            deps: core.deps().iter().cloned().map(Into::into).collect(),
            inputs: core.inputs().iter().cloned().map(Into::into).collect(),
            outputs: core.outputs().iter().cloned().map(Into::into).collect(),
            witnesses: core.witnesses().iter().map(Into::into).collect(),
        }
    }
}

impl<'a> From<&'a CoreTransaction> for TransactionView {
    fn from(core: &CoreTransaction) -> Self {
        Self {
            hash: core.hash().to_owned(),
            inner: core.into(),
        }
    }
}

impl From<Transaction> for CoreTransaction {
    fn from(json: Transaction) -> Self {
        let Transaction {
            version,
            deps,
            inputs,
            outputs,
            witnesses,
        } = json;

        TransactionBuilder::default()
            .version(version.0)
            .deps(deps)
            .inputs(inputs)
            .outputs(outputs)
            .witnesses(witnesses)
            .build()
    }
}

impl From<TransactionView> for CoreTransaction {
    fn from(json: TransactionView) -> Self {
        json.inner.into()
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
    pub fn with_pending(tx: CoreTransaction) -> Self {
        Self {
            tx_status: TxStatus::pending(),
            transaction: (&tx).into(),
        }
    }

    /// Build with proposed status
    pub fn with_proposed(tx: CoreTransaction) -> Self {
        Self {
            tx_status: TxStatus::proposed(),
            transaction: (&tx).into(),
        }
    }

    /// Build with committed status
    pub fn with_committed(tx: CoreTransaction, hash: H256) -> Self {
        Self {
            tx_status: TxStatus::committed(hash),
            transaction: (&tx).into(),
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
pub struct Seal {
    pub nonce: Unsigned,
    pub proof: JsonBytes,
}

impl From<CoreSeal> for Seal {
    fn from(core: CoreSeal) -> Seal {
        let (nonce, proof) = core.destruct();
        Seal {
            nonce: Unsigned(nonce),
            proof: JsonBytes::from_bytes(proof),
        }
    }
}

impl From<Seal> for CoreSeal {
    fn from(json: Seal) -> Self {
        let Seal { nonce, proof } = json;
        CoreSeal::new(nonce.0, proof.into_bytes())
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct Header {
    pub version: Version,
    pub parent_hash: H256,
    pub timestamp: Timestamp,
    pub number: BlockNumber,
    pub epoch: EpochNumber,
    pub transactions_root: H256,
    pub witnesses_root: H256,
    pub proposals_hash: H256,
    pub difficulty: U256,
    pub uncles_hash: H256,
    pub uncles_count: Unsigned,
    pub dao: JsonBytes,
    pub seal: Seal,
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct HeaderView {
    #[serde(flatten)]
    pub inner: Header,
    pub hash: H256,
}

impl<'a> From<&'a CoreHeader> for Header {
    fn from(core: &CoreHeader) -> Self {
        Self {
            version: Version(core.version()),
            parent_hash: core.parent_hash().to_owned(),
            timestamp: Timestamp(core.timestamp()),
            number: BlockNumber(core.number()),
            epoch: EpochNumber(core.epoch()),
            transactions_root: core.transactions_root().to_owned(),
            witnesses_root: core.witnesses_root().to_owned(),
            proposals_hash: core.proposals_hash().to_owned(),
            difficulty: core.difficulty().to_owned(),
            uncles_hash: core.uncles_hash().to_owned(),
            uncles_count: Unsigned(u64::from(core.uncles_count())),
            dao: JsonBytes::from_bytes(core.dao().to_owned()),
            seal: core.seal().to_owned().into(),
        }
    }
}

impl<'a> From<&'a CoreHeader> for HeaderView {
    fn from(core: &CoreHeader) -> Self {
        Self {
            hash: core.hash().to_owned(),
            inner: core.into(),
        }
    }
}

impl From<Header> for CoreHeader {
    fn from(json: Header) -> Self {
        let Header {
            version,
            parent_hash,
            timestamp,
            number,
            epoch,
            transactions_root,
            witnesses_root,
            proposals_hash,
            difficulty,
            uncles_hash,
            uncles_count,
            seal,
            dao,
        } = json;

        HeaderBuilder::default()
            .version(version.0)
            .parent_hash(parent_hash)
            .timestamp(timestamp.0)
            .number(number.0)
            .epoch(epoch.0)
            .transactions_root(transactions_root)
            .witnesses_root(witnesses_root)
            .proposals_hash(proposals_hash)
            .difficulty(difficulty)
            .uncles_hash(uncles_hash)
            .uncles_count(uncles_count.0 as u32)
            .seal(seal.into())
            .dao(dao.into_bytes())
            .build()
    }
}

impl From<HeaderView> for CoreHeader {
    fn from(json: HeaderView) -> Self {
        json.inner.into()
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct UncleBlock {
    pub header: Header,
    pub proposals: Vec<ProposalShortId>,
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct UncleBlockView {
    pub header: HeaderView,
    pub proposals: Vec<ProposalShortId>,
}

impl<'a> From<&'a CoreUncleBlock> for UncleBlock {
    fn from(core: &CoreUncleBlock) -> Self {
        Self {
            header: core.header().into(),
            proposals: core.proposals().iter().cloned().map(Into::into).collect(),
        }
    }
}

impl<'a> From<&'a CoreUncleBlock> for UncleBlockView {
    fn from(core: &CoreUncleBlock) -> Self {
        Self {
            header: core.header().into(),
            proposals: core.proposals().iter().cloned().map(Into::into).collect(),
        }
    }
}

impl From<UncleBlock> for CoreUncleBlock {
    fn from(json: UncleBlock) -> Self {
        let UncleBlock { header, proposals } = json;
        CoreUncleBlock::new(
            header.into(),
            proposals.into_iter().map(Into::into).collect::<Vec<_>>(),
        )
    }
}

impl From<UncleBlockView> for CoreUncleBlock {
    fn from(json: UncleBlockView) -> Self {
        let UncleBlockView { header, proposals } = json;
        CoreUncleBlock::new(
            header.into(),
            proposals.into_iter().map(Into::into).collect::<Vec<_>>(),
        )
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
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

impl<'a> From<&'a CoreBlock> for Block {
    fn from(core: &CoreBlock) -> Self {
        Self {
            header: core.header().into(),
            uncles: core.uncles().iter().map(Into::into).collect(),
            transactions: core.transactions().iter().map(Into::into).collect(),
            proposals: core.proposals().iter().cloned().map(Into::into).collect(),
        }
    }
}

impl<'a> From<&'a CoreBlock> for BlockView {
    fn from(core: &CoreBlock) -> Self {
        Self {
            header: core.header().into(),
            uncles: core.uncles().iter().map(Into::into).collect(),
            transactions: core.transactions().iter().map(Into::into).collect(),
            proposals: core.proposals().iter().cloned().map(Into::into).collect(),
        }
    }
}

impl From<Block> for CoreBlock {
    fn from(json: Block) -> Self {
        let Block {
            header,
            uncles,
            transactions,
            proposals,
        } = json;

        BlockBuilder::default()
            .header(header)
            .uncles(uncles)
            .transactions(transactions)
            .proposals(proposals)
            .build()
    }
}

impl From<BlockView> for CoreBlock {
    fn from(json: BlockView) -> Self {
        let BlockView {
            header,
            uncles,
            transactions,
            proposals,
        } = json;

        BlockBuilder::default()
            .header(header)
            .uncles(uncles)
            .transactions(transactions)
            .proposals(proposals)
            .build()
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct EpochView {
    pub number: EpochNumber,
    pub epoch_reward: Capacity,
    pub start_number: BlockNumber,
    pub length: BlockNumber,
    pub difficulty: U256,
}

impl EpochView {
    pub fn from_ext(epoch_reward: CoreCapacity, ext: &CoreEpochExt) -> EpochView {
        EpochView {
            number: EpochNumber(ext.number()),
            start_number: BlockNumber(ext.start_number()),
            length: BlockNumber(ext.length()),
            difficulty: ext.difficulty().clone(),
            epoch_reward: Capacity(epoch_reward),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_core::transaction::ProposalShortId as CoreProposalShortId;
    use ckb_core::{Bytes, Capacity};
    use proptest::{collection::size_range, prelude::*};

    fn mock_script(arg: Bytes) -> CoreScript {
        CoreScript::new(vec![arg], H256::default())
    }

    fn mock_cell_output(data: Bytes, arg: Bytes) -> CoreCellOutput {
        CoreCellOutput::new(
            Capacity::zero(),
            data,
            CoreScript::default(),
            Some(mock_script(arg)),
        )
    }

    fn mock_cell_input() -> CoreCellInput {
        CoreCellInput::new(CoreOutPoint::default(), 0)
    }

    fn mock_full_tx(data: Bytes, arg: Bytes) -> CoreTransaction {
        TransactionBuilder::default()
            .deps(vec![CoreOutPoint::default()])
            .inputs(vec![mock_cell_input()])
            .outputs(vec![mock_cell_output(data, arg.clone())])
            .witness(vec![arg])
            .build()
    }

    fn mock_uncle() -> CoreUncleBlock {
        CoreUncleBlock::new(
            HeaderBuilder::default().build(),
            vec![CoreProposalShortId::default()],
        )
    }

    fn mock_full_block(data: Bytes, arg: Bytes) -> CoreBlock {
        BlockBuilder::default()
            .transactions(vec![mock_full_tx(data, arg)])
            .uncles(vec![mock_uncle()])
            .proposals(vec![CoreProposalShortId::default()])
            .build()
    }

    fn _test_block_convert(data: Bytes, arg: Bytes) -> Result<(), TestCaseError> {
        let block = mock_full_block(data, arg);
        let json_block: Block = (&block).into();
        let encoded = serde_json::to_string(&json_block).unwrap();
        let decode: Block = serde_json::from_str(&encoded).unwrap();
        let decode_block: CoreBlock = decode.into();
        prop_assert_eq!(decode_block, block);
        Ok(())
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
