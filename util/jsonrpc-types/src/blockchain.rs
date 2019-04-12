use crate::proposal_short_id::ProposalShortId;
use crate::Bytes;
use ckb_core::block::{Block as CoreBlock, BlockBuilder};
use ckb_core::header::{Header as CoreHeader, HeaderBuilder, Seal as CoreSeal};
use ckb_core::script::Script as CoreScript;
use ckb_core::transaction::{
    CellInput as CoreCellInput, CellOutput as CoreCellOutput, OutPoint as CoreOutPoint,
    Transaction as CoreTransaction, TransactionBuilder, Witness as CoreWitness,
};
use ckb_core::uncle::UncleBlock as CoreUncleBlock;
use ckb_core::{BlockNumber, Capacity};
use ckb_util::{TryFrom, TryInto};
use failure::Error as FailureError;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use serde_derive::{Deserialize, Serialize};

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct Script {
    pub args: Vec<Bytes>,
    pub binary_hash: H256,
}

impl TryFrom<Script> for CoreScript {
    type Error = FailureError;

    fn try_from(json: Script) -> Result<Self, Self::Error> {
        let Script { args, binary_hash } = json;
        Ok(CoreScript::new(
            args.into_iter().map(Bytes::into_vec).collect(),
            binary_hash,
        ))
    }
}

impl From<CoreScript> for Script {
    fn from(core: CoreScript) -> Script {
        let (args, binary_hash) = core.destruct();
        Script {
            binary_hash,
            args: args.into_iter().map(Bytes::new).collect(),
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct CellOutput {
    pub capacity: String,
    pub data: Bytes,
    pub lock: Script,
    #[serde(rename = "type")]
    pub type_: Option<Script>,
}

impl From<CoreCellOutput> for CellOutput {
    fn from(core: CoreCellOutput) -> CellOutput {
        let (capacity, data, lock, type_) = core.destruct();
        CellOutput {
            capacity: capacity.to_string(),
            data: Bytes::new(data),
            lock: lock.into(),
            type_: type_.map(Into::into),
        }
    }
}

impl TryFrom<CellOutput> for CoreCellOutput {
    type Error = FailureError;

    fn try_from(json: CellOutput) -> Result<Self, Self::Error> {
        let CellOutput {
            capacity,
            data,
            lock,
            type_,
        } = json;

        let type_ = match type_ {
            Some(type_) => Some(TryInto::try_into(type_)?),
            None => None,
        };

        Ok(CoreCellOutput::new(
            capacity.parse::<Capacity>()?,
            data.into_vec(),
            lock.try_into()?,
            type_,
        ))
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct OutPoint {
    pub hash: H256,
    pub index: u32,
}

impl From<CoreOutPoint> for OutPoint {
    fn from(core: CoreOutPoint) -> OutPoint {
        let (hash, index) = core.destruct();
        OutPoint { hash, index }
    }
}

impl TryFrom<OutPoint> for CoreOutPoint {
    type Error = FailureError;

    fn try_from(json: OutPoint) -> Result<Self, Self::Error> {
        let OutPoint { hash, index } = json;
        Ok(CoreOutPoint::new(hash, index))
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct CellInput {
    pub previous_output: OutPoint,
    pub valid_since: u64,
    pub args: Vec<Bytes>,
}

impl From<CoreCellInput> for CellInput {
    fn from(core: CoreCellInput) -> CellInput {
        let (previous_output, valid_since, args) = core.destruct();
        CellInput {
            previous_output: previous_output.into(),
            valid_since,
            args: args.into_iter().map(Bytes::new).collect(),
        }
    }
}

impl TryFrom<CellInput> for CoreCellInput {
    type Error = FailureError;

    fn try_from(json: CellInput) -> Result<Self, Self::Error> {
        let CellInput {
            previous_output,
            valid_since,
            args,
        } = json;
        Ok(CoreCellInput::new(
            previous_output.try_into()?,
            valid_since,
            args.into_iter().map(Bytes::into_vec).collect(),
        ))
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct Witness {
    data: Vec<Bytes>,
}

impl<'a> From<&'a CoreWitness> for Witness {
    fn from(core: &CoreWitness) -> Witness {
        Witness {
            data: core.iter().cloned().map(Bytes::new).collect(),
        }
    }
}

impl TryFrom<Witness> for CoreWitness {
    type Error = FailureError;

    fn try_from(json: Witness) -> Result<Self, Self::Error> {
        Ok(json.data.into_iter().map(Bytes::into_vec).collect())
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct Transaction {
    pub version: u32,
    pub deps: Vec<OutPoint>,
    pub inputs: Vec<CellInput>,
    pub outputs: Vec<CellOutput>,
    pub witnesses: Vec<Witness>,
    #[serde(skip_deserializing)]
    pub hash: H256,
}

impl<'a> From<&'a CoreTransaction> for Transaction {
    fn from(core: &CoreTransaction) -> Transaction {
        let hash = core.hash();

        Transaction {
            version: core.version(),
            deps: core.deps().iter().cloned().map(Into::into).collect(),
            inputs: core.inputs().iter().cloned().map(Into::into).collect(),
            outputs: core.outputs().iter().cloned().map(Into::into).collect(),
            witnesses: core.witnesses().iter().map(Into::into).collect(),
            hash,
        }
    }
}

impl TryFrom<Transaction> for CoreTransaction {
    type Error = FailureError;

    fn try_from(json: Transaction) -> Result<Self, Self::Error> {
        let Transaction {
            version,
            deps,
            inputs,
            outputs,
            witnesses,
            ..
        } = json;

        Ok(TransactionBuilder::default()
            .version(version)
            .deps(
                deps.into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()?,
            )
            .inputs(
                inputs
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()?,
            )
            .outputs(
                outputs
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()?,
            )
            .witnesses(
                witnesses
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()?,
            )
            .build())
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct Seal {
    pub nonce: String,
    pub proof: Bytes,
}

impl From<CoreSeal> for Seal {
    fn from(core: CoreSeal) -> Seal {
        let (nonce, proof) = core.destruct();
        Seal {
            nonce: nonce.to_string(),
            proof: Bytes::new(proof),
        }
    }
}

impl TryFrom<Seal> for CoreSeal {
    type Error = FailureError;

    fn try_from(json: Seal) -> Result<Self, Self::Error> {
        let Seal { nonce, proof } = json;
        Ok(CoreSeal::new(nonce.parse::<u64>()?, proof.into_vec()))
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct Header {
    pub version: u32,
    pub parent_hash: H256,
    pub timestamp: String,
    pub number: String,
    pub txs_commit: H256,
    pub txs_proposal: H256,
    pub witnesses_root: H256,
    pub difficulty: U256,
    pub uncles_hash: H256,
    pub uncles_count: u32,
    pub seal: Seal,
    #[serde(skip_deserializing)]
    pub hash: H256,
}

impl<'a> From<&'a CoreHeader> for Header {
    fn from(core: &CoreHeader) -> Header {
        Header {
            version: core.version(),
            parent_hash: core.parent_hash().clone(),
            timestamp: core.timestamp().to_string(),
            number: core.number().to_string(),
            txs_commit: core.txs_commit().clone(),
            txs_proposal: core.txs_proposal().clone(),
            witnesses_root: core.witnesses_root().clone(),
            difficulty: core.difficulty().clone(),
            uncles_hash: core.uncles_hash().clone(),
            uncles_count: core.uncles_count(),
            seal: core.seal().clone().into(),
            hash: core.hash().clone(),
        }
    }
}

impl TryFrom<Header> for CoreHeader {
    type Error = FailureError;

    fn try_from(json: Header) -> Result<Self, Self::Error> {
        let Header {
            version,
            parent_hash,
            timestamp,
            number,
            txs_commit,
            txs_proposal,
            witnesses_root,
            difficulty,
            uncles_hash,
            uncles_count,
            seal,
            ..
        } = json;

        Ok(HeaderBuilder::default()
            .version(version)
            .parent_hash(parent_hash)
            .timestamp(timestamp.parse::<u64>()?)
            .number(number.parse::<BlockNumber>()?)
            .txs_commit(txs_commit)
            .txs_proposal(txs_proposal)
            .witnesses_root(witnesses_root)
            .difficulty(difficulty)
            .uncles_hash(uncles_hash)
            .uncles_count(uncles_count)
            .seal(seal.try_into()?)
            .build())
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct UncleBlock {
    pub header: Header,
    pub proposal_transactions: Vec<ProposalShortId>,
}

impl<'a> From<&'a CoreUncleBlock> for UncleBlock {
    fn from(core: &CoreUncleBlock) -> UncleBlock {
        UncleBlock {
            header: core.header().into(),
            proposal_transactions: core
                .proposal_transactions()
                .iter()
                .cloned()
                .map(Into::into)
                .collect(),
        }
    }
}

impl TryFrom<UncleBlock> for CoreUncleBlock {
    type Error = FailureError;

    fn try_from(json: UncleBlock) -> Result<Self, Self::Error> {
        let UncleBlock {
            header,
            proposal_transactions,
        } = json;
        Ok(CoreUncleBlock::new(
            header.try_into()?,
            proposal_transactions
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
        ))
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct Block {
    pub header: Header,
    pub uncles: Vec<UncleBlock>,
    pub commit_transactions: Vec<Transaction>,
    pub proposal_transactions: Vec<ProposalShortId>,
}

impl<'a> From<&'a CoreBlock> for Block {
    fn from(core: &CoreBlock) -> Block {
        Block {
            header: core.header().into(),
            uncles: core.uncles().iter().map(Into::into).collect(),
            commit_transactions: core.commit_transactions().iter().map(Into::into).collect(),
            proposal_transactions: core
                .proposal_transactions()
                .iter()
                .cloned()
                .map(Into::into)
                .collect(),
        }
    }
}

impl TryFrom<Block> for CoreBlock {
    type Error = FailureError;

    fn try_from(json: Block) -> Result<Self, Self::Error> {
        let Block {
            header,
            uncles,
            commit_transactions,
            proposal_transactions,
        } = json;

        Ok(BlockBuilder::default()
            .header(header.try_into()?)
            .uncles(
                uncles
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()?,
            )
            .commit_transactions(
                commit_transactions
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()?,
            )
            .proposal_transactions(
                proposal_transactions
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, _>>()?,
            )
            .build())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_core::transaction::ProposalShortId as CoreProposalShortId;
    use proptest::{collection::size_range, prelude::*};

    fn mock_script(arg: Vec<u8>) -> CoreScript {
        CoreScript::new(vec![arg], H256::default())
    }

    fn mock_cell_output(data: Vec<u8>, arg: Vec<u8>) -> CoreCellOutput {
        CoreCellOutput::new(0, data, CoreScript::default(), Some(mock_script(arg)))
    }

    fn mock_cell_input(arg: Vec<u8>) -> CoreCellInput {
        CoreCellInput::new(CoreOutPoint::default(), 0, vec![arg])
    }

    fn mock_full_tx(data: Vec<u8>, arg: Vec<u8>) -> CoreTransaction {
        TransactionBuilder::default()
            .deps(vec![CoreOutPoint::default()])
            .inputs(vec![mock_cell_input(arg.clone())])
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

    fn mock_full_block(data: Vec<u8>, arg: Vec<u8>) -> CoreBlock {
        BlockBuilder::default()
            .commit_transactions(vec![mock_full_tx(data, arg)])
            .uncles(vec![mock_uncle()])
            .proposal_transactions(vec![CoreProposalShortId::default()])
            .build()
    }

    fn _test_block_convert(data: Vec<u8>, arg: Vec<u8>) -> Result<(), TestCaseError> {
        let block = mock_full_block(data, arg);
        let json_block: Block = (&block).into();
        let encoded = serde_json::to_string(&json_block).unwrap();
        let decode: Block = serde_json::from_str(&encoded).unwrap();
        let decode_block: CoreBlock = decode.try_into().unwrap();
        prop_assert_eq!(decode_block, block);
        Ok(())
    }

    proptest! {
        #[test]
        fn test_block_convert(
            data in any_with::<Vec<u8>>(size_range(80).lift()),
            arg in any_with::<Vec<u8>>(size_range(80).lift()),
        ) {
            _test_block_convert(data, arg)?;
        }
    }
}
