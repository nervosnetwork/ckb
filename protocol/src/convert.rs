use crate::protocol_generated::ckb::protocol as ckb_protocol;
use crate::FlatbuffersVectorIterator;
use ckb_core;
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;

impl From<&H256> for ckb_protocol::H256 {
    fn from(h256: &H256) -> Self {
        let bytes = h256.as_fixed_bytes();
        Self::new(
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
            bytes[16], bytes[17], bytes[18], bytes[19], bytes[20], bytes[21], bytes[22], bytes[23],
            bytes[24], bytes[25], bytes[26], bytes[27], bytes[28], bytes[29], bytes[30], bytes[31],
        )
    }
}

impl From<&ckb_protocol::H256> for H256 {
    fn from(h256: &ckb_protocol::H256) -> Self {
        H256::from_slice(&[
            h256.u0(),
            h256.u1(),
            h256.u2(),
            h256.u3(),
            h256.u4(),
            h256.u5(),
            h256.u6(),
            h256.u7(),
            h256.u8_(),
            h256.u9(),
            h256.u10(),
            h256.u11(),
            h256.u12(),
            h256.u13(),
            h256.u14(),
            h256.u15(),
            h256.u16_(),
            h256.u17(),
            h256.u18(),
            h256.u19(),
            h256.u20(),
            h256.u21(),
            h256.u22(),
            h256.u23(),
            h256.u24(),
            h256.u25(),
            h256.u26(),
            h256.u27(),
            h256.u28(),
            h256.u29(),
            h256.u30(),
            h256.u31(),
        ])
        .unwrap()
    }
}

impl From<&ckb_core::transaction::ProposalShortId> for ckb_protocol::ProposalShortId {
    fn from(short_id: &ckb_core::transaction::ProposalShortId) -> Self {
        let bytes = *short_id;
        Self::new(
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            bytes[8], bytes[9],
        )
    }
}

impl From<&ckb_protocol::ProposalShortId> for ckb_core::transaction::ProposalShortId {
    fn from(short_id: &ckb_protocol::ProposalShortId) -> Self {
        Self::from_slice(&[
            short_id.u0(),
            short_id.u1(),
            short_id.u2(),
            short_id.u3(),
            short_id.u4(),
            short_id.u5(),
            short_id.u6(),
            short_id.u7(),
            short_id.u8_(),
            short_id.u9(),
        ])
        .unwrap()
    }
}

impl<'a> From<ckb_protocol::Block<'a>> for ckb_core::block::Block {
    fn from(block: ckb_protocol::Block<'a>) -> Self {
        let commit_transactions =
            FlatbuffersVectorIterator::new(block.commit_transactions().unwrap())
                .map(Into::into)
                .collect();

        let uncles = FlatbuffersVectorIterator::new(block.uncles().unwrap())
            .map(Into::into)
            .collect();

        let proposal_transactions = block
            .proposal_transactions()
            .unwrap()
            .iter()
            .map(Into::into)
            .collect();

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
            proposal_transactions: uncle_block
                .proposal_transactions()
                .unwrap()
                .iter()
                .map(Into::into)
                .collect(),
        }
    }
}

impl<'a> From<ckb_protocol::Header<'a>> for ckb_core::header::Header {
    fn from(header: ckb_protocol::Header<'a>) -> Self {
        ckb_core::header::HeaderBuilder::default()
            .version(header.version())
            .parent_hash(header.parent_hash().unwrap().into())
            .timestamp(header.timestamp())
            .number(header.number())
            .txs_commit(header.txs_commit().unwrap().into())
            .txs_proposal(header.txs_proposal().unwrap().into())
            .difficulty(
                U256::from_little_endian(header.difficulty().and_then(|b| b.seq()).unwrap())
                    .unwrap(),
            )
            .cellbase_id(header.cellbase_id().unwrap().into())
            .uncles_hash(header.uncles_hash().unwrap().into())
            .nonce(header.nonce())
            .proof(header.proof().and_then(|b| b.seq()).unwrap().to_vec())
            .uncles_count(header.uncles_count())
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
            hash: out_point.hash().unwrap().into(),
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
            reference: script.reference().map(Into::into),
        }
    }
}

impl<'a> From<ckb_protocol::CellInput<'a>> for ckb_core::transaction::CellInput {
    fn from(cell_input: ckb_protocol::CellInput<'a>) -> Self {
        ckb_core::transaction::CellInput {
            previous_output: ckb_core::transaction::OutPoint {
                hash: cell_input.hash().unwrap().into(),
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
            lock: cell_output.lock().unwrap().into(),
            type_: cell_output.type_().map(Into::into),
        }
    }
}

impl<'a> From<ckb_protocol::IndexTransaction<'a>> for ckb_core::transaction::IndexTransaction {
    fn from(it: ckb_protocol::IndexTransaction<'a>) -> Self {
        ckb_core::transaction::IndexTransaction {
            index: it.index() as usize,
            transaction: it.transaction().unwrap().into(),
        }
    }
}
