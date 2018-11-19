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

        ckb_core::block::Block {
            header: block.header().unwrap().into(),
            commit_transactions,
            uncles,
            proposal_transactions: block
                .proposal_transactions()
                .unwrap()
                .chunks(10)
                .filter_map(|s| ckb_core::transaction::ProposalShortId::from_slice(s))
                .collect(),
        }
    }
}

impl<'a> From<ckb_protocol::Block<'a>> for ckb_core::block::IndexedBlock {
    fn from(block: ckb_protocol::Block<'a>) -> Self {
        let b: ckb_core::block::Block = block.into();
        b.into()
    }
}

impl<'a> From<ckb_protocol::UncleBlock<'a>> for ckb_core::uncle::UncleBlock {
    fn from(uncle_block: ckb_protocol::UncleBlock<'a>) -> Self {
        ckb_core::uncle::UncleBlock {
            header: uncle_block.header().unwrap().into(),
            cellbase: uncle_block.cellbase().unwrap().into(),
            proposal_transactions: uncle_block
                .proposal_transactions()
                .unwrap()
                .chunks(10)
                .filter_map(|s| ckb_core::transaction::ProposalShortId::from_slice(s))
                .collect(),
        }
    }
}

impl<'a> From<ckb_protocol::Header<'a>> for ckb_core::header::Header {
    fn from(header: ckb_protocol::Header<'a>) -> Self {
        ckb_core::header::Header {
            raw: ckb_core::header::RawHeader {
                version: header.version(),
                parent_hash: H256::from_slice(header.parent_hash().unwrap()),
                timestamp: header.timestamp(),
                number: header.number(),
                txs_commit: H256::from_slice(header.txs_commit().unwrap()),
                txs_proposal: H256::from_slice(header.txs_proposal().unwrap()),
                difficulty: H256::from_slice(header.difficulty().unwrap()).into(),
                cellbase_id: H256::from_slice(header.cellbase_id().unwrap()),
                uncles_hash: H256::from_slice(header.uncles_hash().unwrap()),
            },
            seal: ckb_core::header::Seal {
                nonce: header.nonce(),
                proof: header.proof().unwrap().to_vec(),
            },
        }
    }
}

impl<'a> From<ckb_protocol::Header<'a>> for ckb_core::header::IndexedHeader {
    fn from(header: ckb_protocol::Header<'a>) -> Self {
        let header: ckb_core::header::Header = header.into();
        header.into()
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

        ckb_core::transaction::Transaction {
            version: transaction.version(),
            deps,
            inputs,
            outputs,
        }
    }
}

impl<'a> From<ckb_protocol::Transaction<'a>> for ckb_core::transaction::IndexedTransaction {
    fn from(transaction: ckb_protocol::Transaction<'a>) -> Self {
        let tx: ckb_core::transaction::Transaction = transaction.into();
        tx.into()
    }
}

impl<'a> From<ckb_protocol::OutPoint<'a>> for ckb_core::transaction::OutPoint {
    fn from(out_point: ckb_protocol::OutPoint<'a>) -> Self {
        ckb_core::transaction::OutPoint {
            hash: H256::from_slice(out_point.hash().unwrap()),
            index: out_point.index(),
        }
    }
}

impl<'a> From<ckb_protocol::CellInput<'a>> for ckb_core::transaction::CellInput {
    fn from(cell_input: ckb_protocol::CellInput<'a>) -> Self {
        let script = cell_input.unlock().unwrap();
        let arguments = FlatbuffersVectorIterator::new(script.arguments().unwrap())
            .map(|argument| argument.value().unwrap().to_vec())
            .collect();

        let redeem_arguments = FlatbuffersVectorIterator::new(script.redeem_arguments().unwrap())
            .map(|argument| argument.value().unwrap().to_vec())
            .collect();

        ckb_core::transaction::CellInput {
            previous_output: ckb_core::transaction::OutPoint {
                hash: H256::from_slice(cell_input.hash().unwrap()),
                index: cell_input.index(),
            },
            unlock: ckb_core::script::Script {
                version: script.version(),
                arguments,
                redeem_script: script.redeem_script().map(|s| s.to_vec()),
                redeem_arguments,
                redeem_reference: script.redeem_reference().map(Into::into),
            },
        }
    }
}

impl<'a> From<ckb_protocol::CellOutput<'a>> for ckb_core::transaction::CellOutput {
    fn from(cell_output: ckb_protocol::CellOutput<'a>) -> Self {
        ckb_core::transaction::CellOutput {
            capacity: cell_output.capacity(),
            data: cell_output.data().unwrap().to_vec(),
            lock: H256::from_slice(cell_output.lock().unwrap()),
        }
    }
}
