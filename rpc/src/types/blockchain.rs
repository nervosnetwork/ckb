use crate::types::proposal_short_id::ProposalShortId;
use crate::types::Bytes;
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
    pub(crate) version: u8,
    pub(crate) args: Vec<Bytes>,
    pub(crate) reference: Option<H256>,
    pub(crate) binary: Option<Bytes>,
    pub(crate) signed_args: Vec<Bytes>,
}

impl From<Script> for CoreScript {
    fn from(json: Script) -> CoreScript {
        let Script {
            version,
            args,
            reference,
            binary,
            signed_args,
        } = json;
        CoreScript::new(
            version,
            args.into_iter().map(|arg| arg.into_vec()).collect(),
            reference,
            binary.map(|b| b.into_vec()),
            signed_args.into_iter().map(|arg| arg.into_vec()).collect(),
        )
    }
}

impl From<CoreScript> for Script {
    fn from(core: CoreScript) -> Script {
        let (version, args, reference, binary, signed_args) = core.destruct();
        Script {
            version,
            reference,
            args: args.into_iter().map(Bytes::new).collect(),
            binary: binary.map(Bytes::new),
            signed_args: signed_args.into_iter().map(Bytes::new).collect(),
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct CellOutput {
    pub(crate) capacity: Capacity,
    pub(crate) data: Bytes,
    pub(crate) lock: H256,
    #[serde(rename = "type")]
    pub(crate) type_: Option<Script>,
}

impl From<CoreCellOutput> for CellOutput {
    fn from(core: CoreCellOutput) -> CellOutput {
        let (capacity, data, lock, type_) = core.destruct();
        CellOutput {
            capacity,
            data: Bytes::new(data),
            lock,
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
        CoreCellOutput::new(capacity, data.into_vec(), lock, type_.map(Into::into))
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct OutPoint {
    pub(crate) hash: H256,
    pub(crate) index: u32,
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
    pub(crate) previous_output: OutPoint,
    pub(crate) unlock: Script,
}

impl From<CoreCellInput> for CellInput {
    fn from(core: CoreCellInput) -> CellInput {
        let (previous_output, unlock) = core.destruct();
        CellInput {
            previous_output: previous_output.into(),
            unlock: unlock.into(),
        }
    }
}

impl From<CellInput> for CoreCellInput {
    fn from(json: CellInput) -> CoreCellInput {
        let CellInput {
            previous_output,
            unlock,
        } = json;
        CoreCellInput::new(previous_output.into(), unlock.into())
    }
}

#[derive(Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, Debug)]
pub struct Transaction {
    pub(crate) version: u32,
    pub(crate) deps: Vec<OutPoint>,
    pub(crate) inputs: Vec<CellInput>,
    pub(crate) outputs: Vec<CellOutput>,
    #[serde(skip_deserializing)]
    pub(crate) hash: H256,
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
    pub(crate) nonce: u64,
    pub(crate) proof: Bytes,
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
    pub(crate) version: u32,
    pub(crate) parent_hash: H256,
    pub(crate) timestamp: u64,
    pub(crate) number: BlockNumber,
    pub(crate) txs_commit: H256,
    pub(crate) txs_proposal: H256,
    pub(crate) difficulty: U256,
    pub(crate) cellbase_id: H256,
    pub(crate) uncles_hash: H256,
    pub(crate) uncles_count: u32,
    pub(crate) seal: Seal,
    #[serde(skip_deserializing)]
    pub(crate) hash: H256,
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
    pub(crate) header: Header,
    pub(crate) cellbase: Transaction,
    pub(crate) proposal_transactions: Vec<ProposalShortId>,
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
    pub(crate) header: Header,
    pub(crate) uncles: Vec<UncleBlock>,
    pub(crate) commit_transactions: Vec<Transaction>,
    pub(crate) proposal_transactions: Vec<ProposalShortId>,
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
