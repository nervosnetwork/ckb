use bigint::H256;
use ckb_core;
use protocol_generated::ckb::protocol as ckb_protocol;
use FlatbuffersVectorIterator;

impl<'a> From<ckb_protocol::Block<'a>> for ckb_core::block::Block {
    fn from(block: ckb_protocol::Block<'a>) -> Self {
        let commit_transactions =
            FlatbuffersVectorIterator::new(block.commit_transactions().unwrap())
                .map(Into::into)
                .collect();

        let uncles = FlatbuffersVectorIterator::new(block.uncles().unwrap())
            .map(Into::into)
            .collect();

        let proposal_transactions =
            FlatbuffersVectorIterator::new(block.proposal_transactions().unwrap())
                .filter_map(|s| {
                    s.seq()
                        .and_then(ckb_core::transaction::ProposalShortId::from_slice)
                }).collect();

        ckb_core::block::BlockBuilder::default()
            .header(block.header().unwrap().into())
            .uncles(uncles)
            .commit_transactions(commit_transactions)
            .proposal_transactions(proposal_transactions)
            .build()
    }
}

impl<'a> From<ckb_protocol::UncleBlock<'a>> for ckb_core::uncle::UncleBlock {
    fn from(uncle_block: ckb_protocol::UncleBlock<'a>) -> Self {
        ckb_core::uncle::UncleBlock {
            header: uncle_block.header().unwrap().into(),
            cellbase: uncle_block.cellbase().unwrap().into(),
            proposal_transactions: FlatbuffersVectorIterator::new(
                uncle_block.proposal_transactions().unwrap(),
            ).filter_map(|s| {
                s.seq()
                    .and_then(ckb_core::transaction::ProposalShortId::from_slice)
            }).collect(),
        }
    }
}

impl<'a> From<ckb_protocol::Header<'a>> for ckb_core::header::Header {
    fn from(header: ckb_protocol::Header<'a>) -> Self {
        ckb_core::header::HeaderBuilder::default()
            .version(header.version())
            .parent_hash(&H256::from_slice(
                header.parent_hash().and_then(|b| b.seq()).unwrap(),
            )).timestamp(header.timestamp())
            .number(header.number())
            .txs_commit(&H256::from_slice(
                header.txs_commit().and_then(|b| b.seq()).unwrap(),
            )).txs_proposal(&H256::from_slice(
                header.txs_proposal().and_then(|b| b.seq()).unwrap(),
            )).difficulty(
                &H256::from_slice(header.difficulty().and_then(|b| b.seq()).unwrap()).into(),
            ).cellbase_id(&H256::from_slice(
                header.cellbase_id().and_then(|b| b.seq()).unwrap(),
            )).uncles_hash(&H256::from_slice(
                header.uncles_hash().and_then(|b| b.seq()).unwrap(),
            )).nonce(header.nonce())
            .proof(header.proof().and_then(|b| b.seq()).unwrap())
            .build()
    }
}

impl<'a> From<ckb_protocol::Transaction<'a>> for ckb_core::transaction::Transaction {
    fn from(transaction: ckb_protocol::Transaction<'a>) -> Self {
        let deps = FlatbuffersVectorIterator::new(transaction.deps().unwrap())
            .map(Into::into)
            .collect();

        let inputs = FlatbuffersVectorIterator::new(transaction.inputs().unwrap())
            .map(Into::into)
            .collect();

        let outputs = FlatbuffersVectorIterator::new(transaction.outputs().unwrap())
            .map(Into::into)
            .collect();

        ckb_core::transaction::TransactionBuilder::default()
            .version(transaction.version())
            .deps(deps)
            .inputs(inputs)
            .outputs(outputs)
            .build()
    }
}

impl<'a> From<ckb_protocol::OutPoint<'a>> for ckb_core::transaction::OutPoint {
    fn from(out_point: ckb_protocol::OutPoint<'a>) -> Self {
        ckb_core::transaction::OutPoint {
            hash: H256::from_slice(out_point.hash().and_then(|b| b.seq()).unwrap()),
            index: out_point.index(),
        }
    }
}

impl<'a> From<ckb_protocol::Script<'a>> for ckb_core::script::Script {
    fn from(script: ckb_protocol::Script<'a>) -> Self {
        let args = FlatbuffersVectorIterator::new(script.args().unwrap())
            .map(|argument| argument.seq().unwrap().to_vec())
            .collect();

        let signed_args = FlatbuffersVectorIterator::new(script.signed_args().unwrap())
            .map(|argument| argument.seq().unwrap().to_vec())
            .collect();

        ckb_core::script::Script {
            version: script.version(),
            args,
            binary: script.binary().and_then(|s| s.seq()).map(|s| s.to_vec()),
            signed_args,
            reference: script
                .reference()
                .and_then(|s| s.seq())
                .map(|s| H256::from_slice(s)),
        }
    }
}

impl<'a> From<ckb_protocol::CellInput<'a>> for ckb_core::transaction::CellInput {
    fn from(cell_input: ckb_protocol::CellInput<'a>) -> Self {
        ckb_core::transaction::CellInput {
            previous_output: ckb_core::transaction::OutPoint {
                hash: H256::from_slice(cell_input.hash().and_then(|b| b.seq()).unwrap()),
                index: cell_input.index(),
            },
            unlock: cell_input.unlock().unwrap().into(),
        }
    }
}

impl<'a> From<ckb_protocol::CellOutput<'a>> for ckb_core::transaction::CellOutput {
    fn from(cell_output: ckb_protocol::CellOutput<'a>) -> Self {
        ckb_core::transaction::CellOutput {
            capacity: cell_output.capacity(),
            data: cell_output.data().and_then(|b| b.seq()).unwrap().to_vec(),
            lock: H256::from_slice(cell_output.lock().and_then(|b| b.seq()).unwrap()),
            contract: cell_output.contract().map(Into::into),
        }
    }
}
