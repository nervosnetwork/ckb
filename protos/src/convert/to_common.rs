use flatbuffers::{FlatBufferBuilder, WIPOffset};
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;

use ckb_core::{
    block::Block,
    extras::TransactionAddress,
    header::Header,
    script::Script,
    transaction::{CellInput, CellOutput, OutPoint, ProposalShortId, Transaction},
    uncle::UncleBlock,
    Bytes,
};

use crate as protos;

impl From<&H256> for protos::H256 {
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

impl From<&U256> for protos::H256 {
    fn from(u256: &U256) -> Self {
        let mut bytes = [0u8; 32];
        u256.into_little_endian(&mut bytes)
            .expect("u256 into_little_endian");
        Self::new(
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
            bytes[16], bytes[17], bytes[18], bytes[19], bytes[20], bytes[21], bytes[22], bytes[23],
            bytes[24], bytes[25], bytes[26], bytes[27], bytes[28], bytes[29], bytes[30], bytes[31],
        )
    }
}

impl From<&ProposalShortId> for protos::ProposalShortId {
    fn from(short_id: &ProposalShortId) -> Self {
        let bytes = *short_id;
        Self::new(
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            bytes[8], bytes[9],
        )
    }
}

impl From<&TransactionAddress> for protos::TransactionAddress {
    fn from(input: &TransactionAddress) -> Self {
        Self::new(&(&input.block_hash).into(), input.index as u32)
    }
}

impl<'a> protos::Bytes<'a> {
    pub fn build<'b>(fbb: &mut FlatBufferBuilder<'b>, seq: &[u8]) -> WIPOffset<protos::Bytes<'b>> {
        let seq = fbb.create_vector(seq);
        let mut builder = protos::BytesBuilder::new(fbb);
        builder.add_seq(seq);
        builder.finish()
    }
}

impl<'a> protos::Script<'a> {
    pub fn build<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        script: &Script,
    ) -> WIPOffset<protos::Script<'b>> {
        let vec = script
            .args
            .iter()
            .map(|argument| protos::Bytes::build(fbb, argument))
            .collect::<Vec<_>>();
        let args = fbb.create_vector(&vec);

        let code_hash = (&script.code_hash).into();

        let mut builder = protos::ScriptBuilder::new(fbb);
        builder.add_args(args);
        builder.add_code_hash(&code_hash);
        builder.finish()
    }
}

impl<'a> protos::CellOutput<'a> {
    pub fn build<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        cell_output: &CellOutput,
    ) -> WIPOffset<protos::CellOutput<'b>> {
        let data = protos::Bytes::build(fbb, &cell_output.data);
        let lock = protos::Script::build(fbb, &cell_output.lock);
        let type_ = cell_output
            .type_
            .as_ref()
            .map(|s| protos::Script::build(fbb, s));
        let mut builder = protos::CellOutputBuilder::new(fbb);
        builder.add_capacity(cell_output.capacity.as_u64());
        builder.add_data(data);
        builder.add_lock(lock);
        if let Some(s) = type_ {
            builder.add_type_(s);
        }
        builder.finish()
    }
}

impl<'a> protos::CellInput<'a> {
    pub fn build<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        cell_input: &CellInput,
    ) -> WIPOffset<protos::CellInput<'b>> {
        let tx_hash = cell_input
            .previous_output
            .cell
            .clone()
            .map(|cell| (&cell.tx_hash).into());
        let tx_index = cell_input
            .previous_output
            .cell
            .as_ref()
            .map(|cell| cell.index);
        let block_hash = cell_input
            .previous_output
            .block_hash
            .clone()
            .map(|hash| (&hash).into());

        let mut builder = protos::CellInputBuilder::new(fbb);
        if let Some(ref hash) = tx_hash {
            builder.add_tx_hash(hash);
        }
        if let Some(index) = tx_index {
            builder.add_index(index);
        }
        if let Some(ref hash) = block_hash {
            builder.add_block_hash(hash);
        }
        builder.add_since(cell_input.since);
        builder.finish()
    }
}

impl<'a> protos::Witness<'a> {
    pub fn build<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        witness: &[Bytes],
    ) -> WIPOffset<protos::Witness<'b>> {
        let data = witness
            .iter()
            .map(|item| protos::Bytes::build(fbb, item))
            .collect::<Vec<_>>();

        let data = fbb.create_vector(&data);
        let mut builder = protos::WitnessBuilder::new(fbb);
        builder.add_data(data);
        builder.finish()
    }
}

impl<'a> protos::Transaction<'a> {
    pub fn build<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        transaction: &Transaction,
    ) -> WIPOffset<protos::Transaction<'b>> {
        let vec = transaction
            .deps()
            .iter()
            .map(|out_point| protos::OutPoint::build(fbb, out_point))
            .collect::<Vec<_>>();
        let deps = fbb.create_vector(&vec);

        let vec = transaction
            .inputs()
            .iter()
            .map(|cell_input| protos::CellInput::build(fbb, cell_input))
            .collect::<Vec<_>>();
        let inputs = fbb.create_vector(&vec);

        let vec = transaction
            .outputs()
            .iter()
            .map(|cell_output| protos::CellOutput::build(fbb, cell_output))
            .collect::<Vec<_>>();
        let outputs = fbb.create_vector(&vec);

        let vec = transaction
            .witnesses()
            .iter()
            .map(|witness| protos::Witness::build(fbb, witness))
            .collect::<Vec<_>>();
        let witnesses = fbb.create_vector(&vec);

        let mut builder = protos::TransactionBuilder::new(fbb);
        builder.add_version(transaction.version());
        builder.add_deps(deps);
        builder.add_inputs(inputs);
        builder.add_outputs(outputs);
        builder.add_witnesses(witnesses);
        builder.finish()
    }
}

impl<'a> protos::OutPoint<'a> {
    pub fn build<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        out_point: &OutPoint,
    ) -> WIPOffset<protos::OutPoint<'b>> {
        let tx_hash = out_point.cell.clone().map(|tx| (&tx.tx_hash).into());
        let tx_index = out_point.cell.as_ref().map(|tx| tx.index);
        let block_hash = out_point.block_hash.clone().map(|hash| (&hash).into());

        let mut builder = protos::OutPointBuilder::new(fbb);
        if let Some(ref hash) = tx_hash {
            builder.add_tx_hash(hash);
        }
        if let Some(index) = tx_index {
            builder.add_index(index);
        }
        if let Some(ref hash) = block_hash {
            builder.add_block_hash(hash);
        }
        builder.finish()
    }
}

impl<'a> protos::Header<'a> {
    pub fn build<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        header: &Header,
    ) -> WIPOffset<protos::Header<'b>> {
        let parent_hash = header.parent_hash().into();
        let transactions_root = header.transactions_root().into();
        let witnesses_root = header.witnesses_root().into();
        let proposals_hash = header.proposals_hash().into();
        let difficulty = header.difficulty().into();
        let proof = protos::Bytes::build(fbb, &header.proof());
        let uncles_hash = header.uncles_hash().into();
        let mut builder = protos::HeaderBuilder::new(fbb);
        builder.add_version(header.version());
        builder.add_parent_hash(&parent_hash);
        builder.add_timestamp(header.timestamp());
        builder.add_number(header.number());
        builder.add_epoch(header.epoch());
        builder.add_transactions_root(&transactions_root);
        builder.add_proposals_hash(&proposals_hash);
        builder.add_witnesses_root(&witnesses_root);
        builder.add_difficulty(&difficulty);
        builder.add_nonce(header.nonce());
        builder.add_proof(proof);
        builder.add_uncles_hash(&uncles_hash);
        builder.add_uncles_count(header.uncles_count());
        builder.finish()
    }
}

impl<'a> protos::UncleBlock<'a> {
    pub fn build<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        uncle_block: &UncleBlock,
    ) -> WIPOffset<protos::UncleBlock<'b>> {
        let header = protos::Header::build(fbb, &uncle_block.header());
        let vec = uncle_block
            .proposals
            .iter()
            .map(Into::into)
            .collect::<Vec<protos::ProposalShortId>>();
        let proposals = fbb.create_vector(&vec);

        let mut builder = protos::UncleBlockBuilder::new(fbb);
        builder.add_header(header);
        builder.add_proposals(proposals);
        builder.finish()
    }
}

impl<'a> protos::Block<'a> {
    pub fn build<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        block: &Block,
    ) -> WIPOffset<protos::Block<'b>> {
        let header = protos::Header::build(fbb, &block.header());

        let vec = block
            .uncles()
            .iter()
            .map(|uncle| protos::UncleBlock::build(fbb, uncle))
            .collect::<Vec<_>>();
        let uncles = fbb.create_vector(&vec);

        let vec = block
            .transactions()
            .iter()
            .map(|transaction| protos::Transaction::build(fbb, transaction))
            .collect::<Vec<_>>();
        let transactions = fbb.create_vector(&vec);

        let vec = block
            .proposals()
            .iter()
            .map(Into::into)
            .collect::<Vec<protos::ProposalShortId>>();
        let proposals = fbb.create_vector(&vec);

        let mut builder = protos::BlockBuilder::new(fbb);
        builder.add_header(header);
        builder.add_uncles(uncles);
        builder.add_transactions(transactions);
        builder.add_proposals(proposals);
        builder.finish()
    }
}

impl<'a> protos::BlockBody<'a> {
    pub fn build<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        transactions: &[Transaction],
    ) -> WIPOffset<protos::BlockBody<'b>> {
        let vec = transactions
            .iter()
            .map(|transaction| protos::Transaction::build(fbb, transaction))
            .collect::<Vec<_>>();
        let transactions = fbb.create_vector(&vec);

        let mut builder = protos::BlockBodyBuilder::new(fbb);
        builder.add_transactions(transactions);
        builder.finish()
    }
}
