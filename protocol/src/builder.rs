use crate::protocol_generated::ckb::protocol::{
    Block as FbsBlock, BlockBuilder, BlockProposalBuilder, BlockTransactionsBuilder,
    Bytes as FbsBytes, BytesBuilder, CellInput as FbsCellInput, CellInputBuilder,
    CellOutput as FbsCellOutput, CellOutputBuilder, CompactBlock, CompactBlockBuilder,
    GetBlockProposalBuilder, GetBlockTransactionsBuilder, GetBlocks as FbsGetBlocks,
    GetBlocksBuilder, GetHeaders as FbsGetHeaders, GetHeadersBuilder, Header as FbsHeader,
    HeaderBuilder, Headers as FbsHeaders, HeadersBuilder, OutPoint as FbsOutPoint, OutPointBuilder,
    PrefilledTransactionBuilder, RelayMessage, RelayMessageBuilder, RelayPayload,
    Script as FbsScript, ScriptBuilder, SyncMessage, SyncMessageBuilder, SyncPayload,
    Transaction as FbsTransaction, TransactionBuilder, UncleBlock as FbsUncleBlock,
    UncleBlockBuilder,
};
use crate::{short_transaction_id, short_transaction_id_keys};
use ckb_core::block::Block;
use ckb_core::header::{BlockNumber, Header};
use ckb_core::script::Script;
use ckb_core::transaction::{CellInput, CellOutput, OutPoint, ProposalShortId, Transaction};
use ckb_core::uncle::UncleBlock;
use flatbuffers::{FlatBufferBuilder, WIPOffset};
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use rand::{thread_rng, Rng};
use std::collections::HashSet;

fn uint_to_bytes(uint: &U256) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    uint.into_little_endian(&mut bytes)
        .expect("uint into_little_endian");
    bytes
}

impl<'a> FbsBytes<'a> {
    pub fn build<'b>(fbb: &mut FlatBufferBuilder<'b>, seq: &[u8]) -> WIPOffset<FbsBytes<'b>> {
        let seq = fbb.create_vector(seq);
        let mut builder = BytesBuilder::new(fbb);
        builder.add_seq(seq);
        builder.finish()
    }
}

impl<'a> FbsHeader<'a> {
    pub fn build<'b>(fbb: &mut FlatBufferBuilder<'b>, header: &Header) -> WIPOffset<FbsHeader<'b>> {
        let parent_hash = FbsBytes::build(fbb, header.parent_hash().as_bytes());
        let txs_commit = FbsBytes::build(fbb, header.txs_commit().as_bytes());
        let txs_proposal = FbsBytes::build(fbb, header.txs_proposal().as_bytes());
        let difficulty = FbsBytes::build(fbb, &uint_to_bytes(header.difficulty()));
        let proof = FbsBytes::build(fbb, &header.proof());
        let cellbase_id = FbsBytes::build(fbb, header.cellbase_id().as_bytes());
        let uncles_hash = FbsBytes::build(fbb, header.uncles_hash().as_bytes());
        let mut builder = HeaderBuilder::new(fbb);
        builder.add_version(header.version());
        builder.add_parent_hash(parent_hash);
        builder.add_timestamp(header.timestamp());
        builder.add_number(header.number());
        builder.add_txs_commit(txs_commit);
        builder.add_txs_proposal(txs_proposal);
        builder.add_difficulty(difficulty);
        builder.add_nonce(header.nonce());
        builder.add_proof(proof);
        builder.add_cellbase_id(cellbase_id);
        builder.add_uncles_hash(uncles_hash);
        builder.add_uncles_count(header.uncles_count());
        builder.finish()
    }
}

impl<'a> FbsTransaction<'a> {
    pub fn build<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        transaction: &Transaction,
    ) -> WIPOffset<FbsTransaction<'b>> {
        let vec = transaction
            .deps()
            .iter()
            .map(|out_point| FbsOutPoint::build(fbb, out_point))
            .collect::<Vec<_>>();
        let deps = fbb.create_vector(&vec);

        let vec = transaction
            .inputs()
            .iter()
            .map(|cell_input| FbsCellInput::build(fbb, cell_input))
            .collect::<Vec<_>>();
        let inputs = fbb.create_vector(&vec);

        let vec = transaction
            .outputs()
            .iter()
            .map(|cell_output| FbsCellOutput::build(fbb, cell_output))
            .collect::<Vec<_>>();
        let outputs = fbb.create_vector(&vec);

        let mut builder = TransactionBuilder::new(fbb);
        builder.add_version(transaction.version());
        builder.add_deps(deps);
        builder.add_inputs(inputs);
        builder.add_outputs(outputs);
        builder.finish()
    }
}

impl<'a> FbsOutPoint<'a> {
    pub fn build<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        out_point: &OutPoint,
    ) -> WIPOffset<FbsOutPoint<'b>> {
        let hash = FbsBytes::build(fbb, out_point.hash.as_bytes());
        let mut builder = OutPointBuilder::new(fbb);
        builder.add_hash(hash);
        builder.add_index(out_point.index);
        builder.finish()
    }
}

impl<'a> FbsCellInput<'a> {
    pub fn build<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        cell_input: &CellInput,
    ) -> WIPOffset<FbsCellInput<'b>> {
        let hash = FbsBytes::build(fbb, cell_input.previous_output.hash.as_bytes());
        let unlock = FbsScript::build(fbb, &cell_input.unlock);

        let mut builder = CellInputBuilder::new(fbb);
        builder.add_hash(hash);
        builder.add_index(cell_input.previous_output.index);
        builder.add_unlock(unlock);
        builder.finish()
    }
}

impl<'a> FbsScript<'a> {
    pub fn build<'b>(fbb: &mut FlatBufferBuilder<'b>, script: &Script) -> WIPOffset<FbsScript<'b>> {
        let vec = script
            .args
            .iter()
            .map(|argument| FbsBytes::build(fbb, argument))
            .collect::<Vec<_>>();
        let args = fbb.create_vector(&vec);

        let binary = script.binary.as_ref().map(|s| FbsBytes::build(fbb, s));

        let reference = script
            .reference
            .as_ref()
            .map(|b| FbsBytes::build(fbb, b.as_bytes()));

        let vec = script
            .signed_args
            .iter()
            .map(|argument| FbsBytes::build(fbb, argument))
            .collect::<Vec<_>>();
        let signed_args = fbb.create_vector(&vec);

        let mut builder = ScriptBuilder::new(fbb);
        builder.add_version(script.version);
        builder.add_args(args);
        if let Some(s) = binary {
            builder.add_binary(s);
        }
        if let Some(r) = reference {
            builder.add_reference(r);
        }
        builder.add_signed_args(signed_args);
        builder.finish()
    }
}

impl<'a> FbsCellOutput<'a> {
    pub fn build<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        cell_output: &CellOutput,
    ) -> WIPOffset<FbsCellOutput<'b>> {
        let data = FbsBytes::build(fbb, &cell_output.data);
        let lock = FbsBytes::build(fbb, &cell_output.lock.as_bytes());
        let contract = cell_output
            .contract
            .as_ref()
            .map(|s| FbsScript::build(fbb, s));
        let mut builder = CellOutputBuilder::new(fbb);
        builder.add_capacity(cell_output.capacity);
        builder.add_data(data);
        builder.add_lock(lock);
        if let Some(s) = contract {
            builder.add_contract(s);
        }
        builder.finish()
    }
}

impl<'a> FbsBlock<'a> {
    pub fn build<'b>(fbb: &mut FlatBufferBuilder<'b>, block: &Block) -> WIPOffset<FbsBlock<'b>> {
        let header = FbsHeader::build(fbb, &block.header());

        let vec = block
            .uncles()
            .iter()
            .map(|uncle| FbsUncleBlock::build(fbb, uncle))
            .collect::<Vec<_>>();
        let uncles = fbb.create_vector(&vec);

        let vec = block
            .commit_transactions()
            .iter()
            .map(|transaction| FbsTransaction::build(fbb, transaction))
            .collect::<Vec<_>>();
        let commit_transactions = fbb.create_vector(&vec);

        let vec = block
            .proposal_transactions()
            .iter()
            .map(|id| FbsBytes::build(fbb, &id[..]))
            .collect::<Vec<_>>();
        let proposal_transactions = fbb.create_vector(&vec);

        let mut builder = BlockBuilder::new(fbb);
        builder.add_header(header);
        builder.add_uncles(uncles);
        builder.add_commit_transactions(commit_transactions);
        builder.add_proposal_transactions(proposal_transactions);
        builder.finish()
    }
}

impl<'a> FbsUncleBlock<'a> {
    pub fn build<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        uncle_block: &UncleBlock,
    ) -> WIPOffset<FbsUncleBlock<'b>> {
        // TODO how to avoid clone here?
        let header = FbsHeader::build(fbb, &uncle_block.header().clone());
        let cellbase = FbsTransaction::build(fbb, &uncle_block.cellbase);
        let vec = uncle_block
            .proposal_transactions
            .iter()
            .map(|id| FbsBytes::build(fbb, &id[..]))
            .collect::<Vec<_>>();
        let proposal_transactions = fbb.create_vector(&vec);

        let mut builder = UncleBlockBuilder::new(fbb);
        builder.add_header(header);
        builder.add_cellbase(cellbase);
        builder.add_proposal_transactions(proposal_transactions);
        builder.finish()
    }
}

impl<'a> FbsHeaders<'a> {
    pub fn build<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        headers: &[Header],
    ) -> WIPOffset<FbsHeaders<'b>> {
        let vec = headers
            .iter()
            .map(|header| FbsHeader::build(fbb, header))
            .collect::<Vec<_>>();
        let headers = fbb.create_vector(&vec);
        let mut builder = HeadersBuilder::new(fbb);
        builder.add_headers(headers);
        builder.finish()
    }
}

impl<'a> FbsGetHeaders<'a> {
    pub fn build<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        block_locator_hashes: &[H256],
    ) -> WIPOffset<FbsGetHeaders<'b>> {
        let vec = block_locator_hashes
            .iter()
            .map(|hash| FbsBytes::build(fbb, hash.as_bytes()))
            .collect::<Vec<_>>();
        let block_locator_hashes = fbb.create_vector(&vec);
        let mut builder = GetHeadersBuilder::new(fbb);
        // TODO remove version from protocol?
        builder.add_version(0);
        builder.add_block_locator_hashes(block_locator_hashes);
        // TODO PENDING hash_stop
        // builder.add_hash_stop(...)
        builder.finish()
    }
}

impl<'a> FbsGetBlocks<'a> {
    pub fn build<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        block_hashes: &[H256],
    ) -> WIPOffset<FbsGetBlocks<'b>> {
        let vec = block_hashes
            .iter()
            .map(|hash| FbsBytes::build(fbb, hash.as_bytes()))
            .collect::<Vec<_>>();
        let block_hashes = fbb.create_vector(&vec);
        let mut builder = GetBlocksBuilder::new(fbb);
        builder.add_block_hashes(block_hashes);
        builder.finish()
    }
}

impl<'a> SyncMessage<'a> {
    pub fn build_get_headers<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        block_locator_hashes: &[H256],
    ) -> WIPOffset<SyncMessage<'b>> {
        let fbs_get_headers = FbsGetHeaders::build(fbb, block_locator_hashes);
        let mut builder = SyncMessageBuilder::new(fbb);
        builder.add_payload_type(SyncPayload::GetHeaders);
        builder.add_payload(fbs_get_headers.as_union_value());
        builder.finish()
    }

    pub fn build_headers<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        headers: &[Header],
    ) -> WIPOffset<SyncMessage<'b>> {
        let fbs_headers = FbsHeaders::build(fbb, headers);
        let mut builder = SyncMessageBuilder::new(fbb);
        builder.add_payload_type(SyncPayload::Headers);
        builder.add_payload(fbs_headers.as_union_value());
        builder.finish()
    }

    pub fn build_get_blocks<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        block_hashes: &[H256],
    ) -> WIPOffset<SyncMessage<'b>> {
        let fbs_get_blocks = FbsGetBlocks::build(fbb, block_hashes);
        let mut builder = SyncMessageBuilder::new(fbb);
        builder.add_payload_type(SyncPayload::GetBlocks);
        builder.add_payload(fbs_get_blocks.as_union_value());
        builder.finish()
    }

    pub fn build_block<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        block: &Block,
    ) -> WIPOffset<SyncMessage<'b>> {
        let fbs_block = FbsBlock::build(fbb, &block);
        let mut builder = SyncMessageBuilder::new(fbb);
        builder.add_payload_type(SyncPayload::Block);
        builder.add_payload(fbs_block.as_union_value());
        builder.finish()
    }
}

impl<'a> CompactBlock<'a> {
    pub fn build<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        block: &Block,
        prefilled_transactions_indexes: &HashSet<usize>,
    ) -> WIPOffset<CompactBlock<'b>> {
        let nonce: u64 = thread_rng().gen();
        // always prefill cellbase
        let prefilled_transactions_len = prefilled_transactions_indexes.len() + 1;
        let mut short_ids: Vec<_> = Vec::with_capacity(
            block
                .commit_transactions()
                .len()
                .saturating_sub(prefilled_transactions_len),
        );
        let mut prefilled_transactions = Vec::with_capacity(prefilled_transactions_len);

        let (key0, key1) = short_transaction_id_keys(block.header().nonce(), nonce);
        for (transaction_index, transaction) in block.commit_transactions().iter().enumerate() {
            if prefilled_transactions_indexes.contains(&transaction_index)
                || transaction.is_cellbase()
            {
                let fbs_transaction = FbsTransaction::build(fbb, transaction);
                let mut builder = PrefilledTransactionBuilder::new(fbb);
                builder.add_index(transaction_index as u32);
                builder.add_transaction(fbs_transaction);
                prefilled_transactions.push(builder.finish());
            } else {
                short_ids.push(FbsBytes::build(
                    fbb,
                    &short_transaction_id(key0, key1, &transaction.hash()),
                ));
            }
        }

        let header = FbsHeader::build(fbb, &block.header());
        let short_ids = fbb.create_vector(&short_ids);
        let prefilled_transactions = fbb.create_vector(&prefilled_transactions);
        let vec = block
            .uncles()
            .iter()
            .map(|uncle| FbsUncleBlock::build(fbb, uncle))
            .collect::<Vec<_>>();
        let uncles = fbb.create_vector(&vec);
        let vec = block
            .proposal_transactions()
            .iter()
            .map(|id| FbsBytes::build(fbb, &id[..]))
            .collect::<Vec<_>>();
        let proposal_transactions = fbb.create_vector(&vec);

        let mut builder = CompactBlockBuilder::new(fbb);
        builder.add_header(header);
        builder.add_nonce(nonce);
        builder.add_short_ids(short_ids);
        builder.add_prefilled_transactions(prefilled_transactions);
        builder.add_uncles(uncles);
        builder.add_proposal_transactions(proposal_transactions);
        builder.finish()
    }
}

impl<'a> RelayMessage<'a> {
    pub fn build_compact_block<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        block: &Block,
        prefilled_transactions_indexes: &HashSet<usize>,
    ) -> WIPOffset<RelayMessage<'b>> {
        let compact_block = CompactBlock::build(fbb, block, prefilled_transactions_indexes);
        let mut builder = RelayMessageBuilder::new(fbb);
        builder.add_payload_type(RelayPayload::CompactBlock);
        builder.add_payload(compact_block.as_union_value());
        builder.finish()
    }

    pub fn build_transaction<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        transaction: &Transaction,
    ) -> WIPOffset<RelayMessage<'b>> {
        let fbs_transaction = FbsTransaction::build(fbb, transaction);
        let mut builder = RelayMessageBuilder::new(fbb);
        builder.add_payload_type(RelayPayload::Transaction);
        builder.add_payload(fbs_transaction.as_union_value());
        builder.finish()
    }

    pub fn build_get_block_transactions<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        hash: &H256,
        indexes: &[u32],
    ) -> WIPOffset<RelayMessage<'b>> {
        let get_block_transactions = {
            let hash = FbsBytes::build(fbb, hash.as_bytes());
            let indexes = fbb.create_vector(indexes);
            let mut builder = GetBlockTransactionsBuilder::new(fbb);
            builder.add_hash(hash);
            builder.add_indexes(indexes);
            builder.finish()
        };

        let mut builder = RelayMessageBuilder::new(fbb);
        builder.add_payload_type(RelayPayload::GetBlockTransactions);
        builder.add_payload(get_block_transactions.as_union_value());
        builder.finish()
    }

    pub fn build_block_transactions<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        hash: &H256,
        transactions: &[Transaction],
    ) -> WIPOffset<RelayMessage<'b>> {
        let block_transactions = {
            let hash = FbsBytes::build(fbb, hash.as_bytes());;
            let vec = transactions
                .iter()
                .map(|transaction| FbsTransaction::build(fbb, transaction))
                .collect::<Vec<_>>();
            let transactions = fbb.create_vector(&vec);

            let mut builder = BlockTransactionsBuilder::new(fbb);
            builder.add_hash(hash);
            builder.add_transactions(transactions);
            builder.finish()
        };

        let mut builder = RelayMessageBuilder::new(fbb);
        builder.add_payload_type(RelayPayload::BlockTransactions);
        builder.add_payload(block_transactions.as_union_value());
        builder.finish()
    }

    pub fn build_get_block_proposal<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        block_number: BlockNumber,
        proposal_transactions: &[ProposalShortId],
    ) -> WIPOffset<RelayMessage<'b>> {
        let get_block_proposal = {
            let vec = proposal_transactions
                .iter()
                .map(|id| FbsBytes::build(fbb, &id[..]))
                .collect::<Vec<_>>();
            let proposal_transactions = fbb.create_vector(&vec);
            let mut builder = GetBlockProposalBuilder::new(fbb);
            builder.add_block_number(block_number);
            builder.add_proposal_transactions(proposal_transactions);
            builder.finish()
        };

        let mut builder = RelayMessageBuilder::new(fbb);
        builder.add_payload_type(RelayPayload::GetBlockProposal);
        builder.add_payload(get_block_proposal.as_union_value());
        builder.finish()
    }

    pub fn build_block_proposal<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        transactions: &[Transaction],
    ) -> WIPOffset<RelayMessage<'b>> {
        let block_proposal = {
            let vec = transactions
                .iter()
                .map(|transaction| FbsTransaction::build(fbb, transaction))
                .collect::<Vec<_>>();
            let transactions = fbb.create_vector(&vec);

            let mut builder = BlockProposalBuilder::new(fbb);
            builder.add_transactions(transactions);
            builder.finish()
        };

        let mut builder = RelayMessageBuilder::new(fbb);
        builder.add_payload_type(RelayPayload::BlockProposal);
        builder.add_payload(block_proposal.as_union_value());
        builder.finish()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use ckb_core::block::BlockBuilder;
    use ckb_core::header::HeaderBuilder;
    use ckb_core::transaction::TransactionBuilder;
    use flatbuffers::get_root;

    #[test]
    fn build_and_convert_header() {
        let header = HeaderBuilder::default().build();
        let builder = &mut FlatBufferBuilder::new();
        let b = FbsHeader::build(builder, &header);
        builder.finish(b, None);

        let fbs_header = get_root::<FbsHeader>(builder.finished_data());
        assert_eq!(header, fbs_header.into());
    }

    #[test]
    fn build_and_convert_block() {
        let block = BlockBuilder::default()
            .header(HeaderBuilder::default().build())
            .build();
        let builder = &mut FlatBufferBuilder::new();
        let b = FbsBlock::build(builder, &block);
        builder.finish(b, None);

        let fbs_block = get_root::<FbsBlock>(builder.finished_data());
        assert_eq!(block, fbs_block.into());
    }

    #[test]
    fn build_compcat_block_prefilled_transactions_indexes_overflow() {
        let block = BlockBuilder::default()
            .header(HeaderBuilder::default().build())
            .commit_transaction(TransactionBuilder::default().build())
            .build();
        let builder = &mut FlatBufferBuilder::new();
        let mut prefilled_transactions_indexes = HashSet::new();
        prefilled_transactions_indexes.insert(0);
        prefilled_transactions_indexes.insert(2);
        let b = CompactBlock::build(builder, &block, &prefilled_transactions_indexes);
        builder.finish(b, None);

        let fbs_compact_block = get_root::<CompactBlock>(builder.finished_data());
        assert_eq!(1, fbs_compact_block.prefilled_transactions().unwrap().len());
    }
}
