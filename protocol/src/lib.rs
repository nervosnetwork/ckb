extern crate bigint;
extern crate ckb_core;
extern crate flatbuffers;

mod convert;
mod protocol_generated;

pub use protocol_generated::ckb::protocol::*;

pub struct FlatbuffersVectorIterator<'a, T: flatbuffers::Follow<'a> + 'a> {
    vector: flatbuffers::Vector<'a, T>,
    counter: usize,
}

impl<'a, T: flatbuffers::Follow<'a> + 'a> FlatbuffersVectorIterator<'a, T> {
    pub fn new(vector: flatbuffers::Vector<'a, T>) -> Self {
        Self { vector, counter: 0 }
    }
}

impl<'a, T: flatbuffers::Follow<'a> + 'a> Iterator for FlatbuffersVectorIterator<'a, T> {
    type Item = T::Inner;

    fn next(&mut self) -> Option<Self::Item> {
        if self.counter < self.vector.len() {
            let result = self.vector.get(self.counter);
            self.counter += 1;
            Some(result)
        } else {
            None
        }
    }
}

pub fn build_header_args<'a>(
    builder: &mut flatbuffers::FlatBufferBuilder<'a>,
    header: &ckb_core::header::Header,
) -> HeaderArgs<'a> {
    let parent_hash = Some(builder.create_vector(&header.parent_hash));
    let txs_commit = Some(builder.create_vector(&header.txs_commit));
    let txs_proposal = Some(builder.create_vector(&header.txs_proposal));
    let difficulty = Some(header.difficulty)
        .map(Into::into)
        .map(|bytes: [u8; 32]| builder.create_vector(&bytes));
    let proof = Some(builder.create_vector(&header.seal.proof));
    let cellbase_id = Some(builder.create_vector(&header.cellbase_id));
    let uncles_hash = Some(builder.create_vector(&header.uncles_hash));

    HeaderArgs {
        version: 0,
        parent_hash,
        timestamp: header.timestamp,
        number: header.number,
        txs_commit,
        txs_proposal,
        difficulty,
        nonce: header.seal.nonce,
        proof,
        cellbase_id,
        uncles_hash,
    }
}

pub fn build_block_args<'a>(
    builder: &mut flatbuffers::FlatBufferBuilder<'a>,
    block: &ckb_core::block::IndexedBlock,
) -> BlockArgs<'a> {
    let header_args = build_header_args(builder, &block.header);
    let header = Some(Header::create(builder, &header_args));
    let vec = block
        .commit_transactions
        .iter()
        .map(|transaction| {
            let transaction_args = build_transaction_args(builder, transaction);
            Transaction::create(builder, &transaction_args)
        }).collect::<Vec<_>>();
    let commit_transactions = Some(builder.create_vector(&vec));

    let vec = block
        .uncles
        .iter()
        .map(|uncle| {
            let uncle_block_args = build_uncle_block_args(builder, uncle);
            UncleBlock::create(builder, &uncle_block_args)
        }).collect::<Vec<_>>();
    let uncles = Some(builder.create_vector(&vec));
    let vec = block
        .proposal_transactions
        .iter()
        .flat_map(|id| id.iter().cloned())
        .collect::<Vec<_>>();
    let proposal_transactions = Some(builder.create_vector(&vec));
    BlockArgs {
        header,
        commit_transactions,
        uncles,
        proposal_transactions,
    }
}

pub fn build_transaction_args<'a>(
    builder: &mut flatbuffers::FlatBufferBuilder<'a>,
    transaction: &ckb_core::transaction::Transaction,
) -> TransactionArgs<'a> {
    let vec = transaction
        .deps
        .iter()
        .map(|dep| {
            let hash = Some(builder.create_vector(&dep.hash));
            let out_point_args = OutPointArgs {
                hash,
                index: dep.index,
            };
            OutPoint::create(builder, &out_point_args)
        }).collect::<Vec<_>>();
    let deps = Some(builder.create_vector(&vec));

    let vec = transaction
        .inputs
        .iter()
        .map(|input| {
            let hash = Some(builder.create_vector(&input.previous_output.hash));
            let vec = input
                .unlock
                .arguments
                .iter()
                .map(|argument| {
                    let value = Some(builder.create_vector(argument));
                    let argument_args = ArgumentArgs { value };
                    Argument::create(builder, &argument_args)
                }).collect::<Vec<_>>();
            let arguments = Some(builder.create_vector(&vec));

            let vec = input
                .unlock
                .redeem_arguments
                .iter()
                .map(|argument| {
                    let value = Some(builder.create_vector(argument));
                    let argument_args = ArgumentArgs { value };
                    Argument::create(builder, &argument_args)
                }).collect::<Vec<_>>();
            let redeem_arguments = Some(builder.create_vector(&vec));

            let redeem_reference = input.unlock.redeem_reference.map(|out_point| {
                let hash = Some(builder.create_vector(&out_point.hash));
                let out_point_args = OutPointArgs {
                    hash,
                    index: out_point.index,
                };
                OutPoint::create(builder, &out_point_args)
            });

            let redeem_script = input
                .unlock
                .redeem_script
                .clone()
                .map(|ref s| builder.create_vector(s));
            let script_args = ScriptArgs {
                version: input.unlock.version,
                redeem_script,
                arguments,
                redeem_arguments,
                redeem_reference,
            };
            let unlock = Some(Script::create(builder, &script_args));
            let input_args = CellInputArgs {
                hash,
                unlock,
                index: input.previous_output.index,
            };
            CellInput::create(builder, &input_args)
        }).collect::<Vec<_>>();
    let inputs = Some(builder.create_vector(&vec));

    let vec = transaction
        .outputs
        .iter()
        .map(|output| {
            let data = Some(builder.create_vector(&output.data));
            let lock = Some(builder.create_vector(&output.lock));
            let output_args = CellOutputArgs {
                capacity: output.capacity,
                data,
                lock,
            };
            CellOutput::create(builder, &output_args)
        }).collect::<Vec<_>>();
    let outputs = Some(builder.create_vector(&vec));

    TransactionArgs {
        version: transaction.version,
        deps,
        inputs,
        outputs,
    }
}

pub fn build_uncle_block_args<'a>(
    builder: &mut flatbuffers::FlatBufferBuilder<'a>,
    uncle: &ckb_core::uncle::UncleBlock,
) -> UncleBlockArgs<'a> {
    let header_args = build_header_args(builder, &uncle.header);
    let header = Some(Header::create(builder, &header_args));
    let transaction_args = build_transaction_args(builder, &uncle.cellbase);
    let cellbase = Some(Transaction::create(builder, &transaction_args));
    let vec = uncle
        .proposal_transactions
        .iter()
        .flat_map(|id| id.iter().cloned())
        .collect::<Vec<_>>();
    let proposal_transactions = Some(builder.create_vector(&vec));

    UncleBlockArgs {
        header,
        cellbase,
        proposal_transactions,
    }
}
