use crate::proposal_short_id::ProposalShortId;
use crate::Bytes;
use ckb_core::block::{Block as CoreBlock, BlockBuilder};
use ckb_core::header::{Header as CoreHeader, HeaderBuilder, Seal as CoreSeal};
use ckb_core::script::Script as CoreScript;
use ckb_core::transaction::{
    CellInput as CoreCellInput, CellOutput as CoreCellOutput, OutPoint as CoreOutPoint,
    Transaction as CoreTransaction, TransactionBuilder,
};
use ckb_core::uncle::UncleBlock as CoreUncleBlock;
use ckb_core::{BlockNumber, Capacity};
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use serde_derive::{Deserialize, Serialize};

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct Script {
    pub version: u8,
    pub args: Vec<Bytes>,
    pub binary_hash: H256,
}

impl From<Script> for CoreScript {
    fn from(json: Script) -> CoreScript {
        let Script {
            version,
            args,
            binary_hash,
        } = json;
        CoreScript::new(
            version,
            args.into_iter().map(|arg| arg.into_vec()).collect(),
            binary_hash,
        )
    }
}

impl From<CoreScript> for Script {
    fn from(core: CoreScript) -> Script {
        let (version, args, binary_hash) = core.destruct();
        Script {
            version,
            binary_hash,
            args: args.into_iter().map(Bytes::new).collect(),
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct CellOutput {
    pub capacity: Capacity,
    pub data: Bytes,
    pub lock: Script,
    #[serde(rename = "type")]
    pub type_: Option<Script>,
}

impl From<CoreCellOutput> for CellOutput {
    fn from(core: CoreCellOutput) -> CellOutput {
        let (capacity, data, lock, type_) = core.destruct();
        CellOutput {
            capacity,
            data: Bytes::new(data),
            lock: lock.into(),
            type_: type_.map(Into::into),
        }
    }
}

impl From<CellOutput> for CoreCellOutput {
    fn from(json: CellOutput) -> CoreCellOutput {
        let CellOutput {
            capacity,
            data,
            lock,
            type_,
        } = json;
        CoreCellOutput::new(
            capacity,
            data.into_vec(),
            lock.into(),
            type_.map(Into::into),
        )
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

impl From<OutPoint> for CoreOutPoint {
    fn from(json: OutPoint) -> CoreOutPoint {
        let OutPoint { hash, index } = json;
        CoreOutPoint::new(hash, index)
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct CellInput {
    pub previous_output: OutPoint,
    pub args: Vec<Bytes>,
}

impl From<CoreCellInput> for CellInput {
    fn from(core: CoreCellInput) -> CellInput {
        let (previous_output, args) = core.destruct();
        CellInput {
            previous_output: previous_output.into(),
            args: args.into_iter().map(Bytes::new).collect(),
        }
    }
}

impl From<CellInput> for CoreCellInput {
    fn from(json: CellInput) -> CoreCellInput {
        let CellInput {
            previous_output,
            args,
        } = json;
        CoreCellInput::new(
            previous_output.into(),
            args.into_iter().map(|arg| arg.into_vec()).collect(),
        )
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct Transaction {
    pub version: u32,
    pub deps: Vec<OutPoint>,
    pub inputs: Vec<CellInput>,
    pub outputs: Vec<CellOutput>,
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
            hash,
        }
    }
}

impl From<Transaction> for CoreTransaction {
    fn from(json: Transaction) -> CoreTransaction {
        let Transaction {
            version,
            deps,
            inputs,
            outputs,
            ..
        } = json;

        TransactionBuilder::default()
            .version(version)
            .deps(deps.into_iter().map(Into::into).collect())
            .inputs(inputs.into_iter().map(Into::into).collect())
            .outputs(outputs.into_iter().map(Into::into).collect())
            .build()
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct Seal {
    pub nonce: u64,
    pub proof: Bytes,
}

impl From<CoreSeal> for Seal {
    fn from(core: CoreSeal) -> Seal {
        let (nonce, proof) = core.destruct();
        Seal {
            nonce,
            proof: Bytes::new(proof),
        }
    }
}

impl From<Seal> for CoreSeal {
    fn from(json: Seal) -> CoreSeal {
        let Seal { nonce, proof } = json;
        CoreSeal::new(nonce, proof.into_vec())
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct Header {
    pub version: u32,
    pub parent_hash: H256,
    pub timestamp: u64,
    pub number: BlockNumber,
    pub txs_commit: H256,
    pub txs_proposal: H256,
    pub difficulty: U256,
    pub cellbase_id: H256,
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
            timestamp: core.timestamp(),
            number: core.number(),
            txs_commit: core.txs_commit().clone(),
            txs_proposal: core.txs_proposal().clone(),
            difficulty: core.difficulty().clone(),
            cellbase_id: core.cellbase_id().clone(),
            uncles_hash: core.uncles_hash().clone(),
            uncles_count: core.uncles_count(),
            seal: core.seal().clone().into(),
            hash: core.hash().clone(),
        }
    }
}

impl From<Header> for CoreHeader {
    fn from(json: Header) -> CoreHeader {
        let Header {
            version,
            parent_hash,
            timestamp,
            number,
            txs_commit,
            txs_proposal,
            difficulty,
            cellbase_id,
            uncles_hash,
            uncles_count,
            seal,
            ..
        } = json;

        HeaderBuilder::default()
            .version(version)
            .parent_hash(parent_hash)
            .timestamp(timestamp)
            .number(number)
            .txs_commit(txs_commit)
            .txs_proposal(txs_proposal)
            .difficulty(difficulty)
            .cellbase_id(cellbase_id)
            .uncles_hash(uncles_hash)
            .uncles_count(uncles_count)
            .seal(seal.into())
            .build()
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct UncleBlock {
    pub header: Header,
    pub cellbase: Transaction,
    pub proposal_transactions: Vec<ProposalShortId>,
}

impl<'a> From<&'a CoreUncleBlock> for UncleBlock {
    fn from(core: &CoreUncleBlock) -> UncleBlock {
        UncleBlock {
            header: core.header().into(),
            cellbase: core.cellbase().into(),
            proposal_transactions: core
                .proposal_transactions()
                .iter()
                .cloned()
                .map(Into::into)
                .collect(),
        }
    }
}

impl From<UncleBlock> for CoreUncleBlock {
    fn from(json: UncleBlock) -> CoreUncleBlock {
        let UncleBlock {
            header,
            cellbase,
            proposal_transactions,
        } = json;
        CoreUncleBlock::new(
            header.into(),
            cellbase.into(),
            proposal_transactions.into_iter().map(Into::into).collect(),
        )
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

impl From<Block> for CoreBlock {
    fn from(json: Block) -> CoreBlock {
        let Block {
            header,
            uncles,
            commit_transactions,
            proposal_transactions,
        } = json;

        BlockBuilder::default()
            .header(header.into())
            .uncles(uncles.into_iter().map(Into::into).collect())
            .commit_transactions(commit_transactions.into_iter().map(Into::into).collect())
            .proposal_transactions(proposal_transactions.into_iter().map(Into::into).collect())
            .build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_core::transaction::ProposalShortId as CoreProposalShortId;
    use proptest::{collection::size_range, prelude::*};

    fn mock_script(arg: Vec<u8>) -> CoreScript {
        CoreScript::new(0, vec![arg], H256::default())
    }

    fn mock_cell_output(data: Vec<u8>, arg: Vec<u8>) -> CoreCellOutput {
        CoreCellOutput::new(0, data, CoreScript::default(), Some(mock_script(arg)))
    }

    fn mock_cell_input(arg: Vec<u8>) -> CoreCellInput {
        CoreCellInput::new(CoreOutPoint::default(), vec![arg])
    }

    fn mock_full_tx(data: Vec<u8>, arg: Vec<u8>) -> CoreTransaction {
        TransactionBuilder::default()
            .deps(vec![CoreOutPoint::default()])
            .inputs(vec![mock_cell_input(arg.clone())])
            .outputs(vec![mock_cell_output(data, arg)])
            .build()
    }

    fn mock_uncle(data: Vec<u8>, arg: Vec<u8>) -> CoreUncleBlock {
        CoreUncleBlock::new(
            HeaderBuilder::default().build(),
            mock_full_tx(data, arg),
            vec![CoreProposalShortId::default()],
        )
    }

    fn mock_full_block(data: Vec<u8>, arg: Vec<u8>) -> CoreBlock {
        BlockBuilder::default()
            .uncles(vec![mock_uncle(data.clone(), arg.clone())])
            .commit_transactions(vec![mock_full_tx(data, arg)])
            .proposal_transactions(vec![CoreProposalShortId::default()])
            .build()
    }

    fn _test_block_convert(data: Vec<u8>, arg: Vec<u8>) -> Result<(), TestCaseError> {
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
            _test_block_convert(data, arg)?;
        }
    }
}
