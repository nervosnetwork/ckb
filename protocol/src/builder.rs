use crate::protocol_generated::ckb::protocol::{
    Alert as FbsAlert, AlertBuilder, AlertMessage, AlertMessageBuilder, Block as FbsBlock,
    BlockBuilder, BlockProposalBuilder, BlockTransactionsBuilder, Bytes as FbsBytes, BytesBuilder,
    CellInput as FbsCellInput, CellInputBuilder, CellOutput as FbsCellOutput, CellOutputBuilder,
    CompactBlock, CompactBlockBuilder, FilteredBlock, FilteredBlockBuilder,
    GetBlockProposalBuilder, GetBlockTransactionsBuilder, GetBlocks as FbsGetBlocks,
    GetBlocksBuilder, GetHeaders as FbsGetHeaders, GetHeadersBuilder,
    GetRelayTransaction as FbsGetRelayTransaction, GetRelayTransactionBuilder, Header as FbsHeader,
    HeaderBuilder, Headers as FbsHeaders, HeadersBuilder, IndexTransactionBuilder,
    MerkleProofBuilder, OutPoint as FbsOutPoint, OutPointBuilder,
    ProposalShortId as FbsProposalShortId, RelayMessage, RelayMessageBuilder, RelayPayload,
    RelayTransaction as FbsRelayTransaction, RelayTransactionBuilder,
    RelayTransactionHash as FbsRelayTransactionHash, RelayTransactionHashBuilder,
    Script as FbsScript, ScriptBuilder, SyncMessage, SyncMessageBuilder, SyncPayload,
    Time as FbsTime, TimeBuilder, TimeMessage, TimeMessageBuilder, Transaction as FbsTransaction,
    TransactionBuilder, UncleBlock as FbsUncleBlock, UncleBlockBuilder, Witness as FbsWitness,
    WitnessBuilder, H256 as FbsH256,
};
use crate::{short_transaction_id, short_transaction_id_keys};
use ckb_core::alert::Alert;
use ckb_core::block::Block;
use ckb_core::header::{BlockNumber, Header};
use ckb_core::script::Script;
use ckb_core::transaction::{CellInput, CellOutput, OutPoint, ProposalShortId, Transaction};
use ckb_core::uncle::UncleBlock;
use ckb_core::{Bytes as CoreBytes, Cycle};
use ckb_merkle_tree::build_merkle_proof;
use flatbuffers::{FlatBufferBuilder, WIPOffset};
use numext_fixed_hash::H256;
use numext_fixed_uint::U256;
use rand::{thread_rng, Rng};
use std::borrow::ToOwned;
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
        let parent_hash = header.parent_hash().into();
        let transactions_root = header.transactions_root().into();
        let witnesses_root = header.witnesses_root().into();
        let proposals_hash = header.proposals_hash().into();
        let difficulty = FbsBytes::build(fbb, &uint_to_bytes(header.difficulty()));
        let proof = FbsBytes::build(fbb, &header.proof());
        let dao = FbsBytes::build(fbb, &header.dao());
        let uncles_hash = header.uncles_hash().into();
        let mut builder = HeaderBuilder::new(fbb);
        builder.add_version(header.version());
        builder.add_parent_hash(&parent_hash);
        builder.add_timestamp(header.timestamp());
        builder.add_number(header.number());
        builder.add_epoch(header.epoch());
        builder.add_transactions_root(&transactions_root);
        builder.add_proposals_hash(&proposals_hash);
        builder.add_witnesses_root(&witnesses_root);
        builder.add_difficulty(difficulty);
        builder.add_nonce(header.nonce());
        builder.add_proof(proof);
        builder.add_dao(dao);
        builder.add_uncles_hash(&uncles_hash);
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

        let vec = transaction
            .witnesses()
            .iter()
            .map(|witness| FbsWitness::build(fbb, witness))
            .collect::<Vec<_>>();
        let witnesses = fbb.create_vector(&vec);

        let mut builder = TransactionBuilder::new(fbb);
        builder.add_version(transaction.version());
        builder.add_deps(deps);
        builder.add_inputs(inputs);
        builder.add_outputs(outputs);
        builder.add_witnesses(witnesses);
        builder.finish()
    }
}

impl<'a> FbsOutPoint<'a> {
    pub fn build<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        out_point: &OutPoint,
    ) -> WIPOffset<FbsOutPoint<'b>> {
        let tx_hash = out_point.cell.clone().map(|tx| (&tx.tx_hash).into());
        let tx_index = out_point.cell.as_ref().map(|tx| tx.index);
        let block_hash = out_point.block_hash.clone().map(|hash| (&hash).into());

        let mut builder = OutPointBuilder::new(fbb);
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

impl<'a> FbsRelayTransaction<'a> {
    pub fn build<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        transaction: &Transaction,
        cycles: Cycle,
    ) -> WIPOffset<FbsRelayTransaction<'b>> {
        let tx = FbsTransaction::build(fbb, transaction);
        let mut builder = RelayTransactionBuilder::new(fbb);
        builder.add_transaction(tx);
        builder.add_cycles(cycles);
        builder.finish()
    }
}

impl<'a> FbsRelayTransactionHash<'a> {
    pub fn build<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        tx_hash: &H256,
    ) -> WIPOffset<FbsRelayTransactionHash<'b>> {
        let mut builder = RelayTransactionHashBuilder::new(fbb);
        let tx_hash = tx_hash.into();
        builder.add_tx_hash(&tx_hash);
        builder.finish()
    }
}

impl<'a> FbsGetRelayTransaction<'a> {
    pub fn build<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        tx_hash: &H256,
    ) -> WIPOffset<FbsGetRelayTransaction<'b>> {
        let mut builder = GetRelayTransactionBuilder::new(fbb);
        let tx_hash = tx_hash.into();
        builder.add_tx_hash(&tx_hash);
        builder.finish()
    }
}

impl<'a> FbsCellInput<'a> {
    pub fn build<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        cell_input: &CellInput,
    ) -> WIPOffset<FbsCellInput<'b>> {
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

        let mut builder = CellInputBuilder::new(fbb);
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

impl<'a> FbsScript<'a> {
    pub fn build<'b>(fbb: &mut FlatBufferBuilder<'b>, script: &Script) -> WIPOffset<FbsScript<'b>> {
        let vec = script
            .args
            .iter()
            .map(|argument| FbsBytes::build(fbb, argument))
            .collect::<Vec<_>>();
        let args = fbb.create_vector(&vec);

        let code_hash = (&script.code_hash).into();

        let mut builder = ScriptBuilder::new(fbb);
        builder.add_args(args);
        builder.add_code_hash(&code_hash);
        builder.finish()
    }
}

impl<'a> FbsWitness<'a> {
    pub fn build<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        witness: &[CoreBytes],
    ) -> WIPOffset<FbsWitness<'b>> {
        let data = witness
            .iter()
            .map(|item| FbsBytes::build(fbb, item))
            .collect::<Vec<_>>();

        let data = fbb.create_vector(&data);
        let mut builder = WitnessBuilder::new(fbb);
        builder.add_data(data);
        builder.finish()
    }
}

impl<'a> FbsCellOutput<'a> {
    pub fn build<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        cell_output: &CellOutput,
    ) -> WIPOffset<FbsCellOutput<'b>> {
        let data = FbsBytes::build(fbb, &cell_output.data);
        let lock = FbsScript::build(fbb, &cell_output.lock);
        let type_ = cell_output.type_.as_ref().map(|s| FbsScript::build(fbb, s));
        let mut builder = CellOutputBuilder::new(fbb);
        builder.add_capacity(cell_output.capacity.as_u64());
        builder.add_data(data);
        builder.add_lock(lock);
        if let Some(s) = type_ {
            builder.add_type_(s);
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
            .transactions()
            .iter()
            .map(|transaction| FbsTransaction::build(fbb, transaction))
            .collect::<Vec<_>>();
        let transactions = fbb.create_vector(&vec);

        let vec = block
            .proposals()
            .iter()
            .map(Into::into)
            .collect::<Vec<FbsProposalShortId>>();
        let proposals = fbb.create_vector(&vec);

        let mut builder = BlockBuilder::new(fbb);
        builder.add_header(header);
        builder.add_uncles(uncles);
        builder.add_transactions(transactions);
        builder.add_proposals(proposals);
        builder.finish()
    }
}

impl<'a> FbsUncleBlock<'a> {
    pub fn build<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        uncle_block: &UncleBlock,
    ) -> WIPOffset<FbsUncleBlock<'b>> {
        let header = FbsHeader::build(fbb, &uncle_block.header());
        let vec = uncle_block
            .proposals
            .iter()
            .map(Into::into)
            .collect::<Vec<FbsProposalShortId>>();
        let proposals = fbb.create_vector(&vec);

        let mut builder = UncleBlockBuilder::new(fbb);
        builder.add_header(header);
        builder.add_proposals(proposals);
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
            .map(Into::into)
            .collect::<Vec<FbsH256>>();
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
            .map(Into::into)
            .collect::<Vec<FbsH256>>();
        let block_hashes = fbb.create_vector(&vec);
        let mut builder = GetBlocksBuilder::new(fbb);
        builder.add_block_hashes(block_hashes);
        builder.finish()
    }
}

impl<'a> FbsTime<'a> {
    pub fn build<'b>(fbb: &mut FlatBufferBuilder<'b>, timestamp: u64) -> WIPOffset<FbsTime<'b>> {
        let mut builder = TimeBuilder::new(fbb);
        builder.add_timestamp(timestamp);
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

    pub fn build_filtered_block<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        block: &Block,
        transactions_index: &[usize],
    ) -> WIPOffset<SyncMessage<'b>> {
        let filtered_block = FilteredBlock::build(fbb, &block, transactions_index);
        let mut builder = SyncMessageBuilder::new(fbb);
        builder.add_payload_type(SyncPayload::FilteredBlock);
        builder.add_payload(filtered_block.as_union_value());
        builder.finish()
    }
}

impl<'a> FilteredBlock<'a> {
    pub fn build<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        block: &Block,
        transactions_index: &[usize],
    ) -> WIPOffset<FilteredBlock<'b>> {
        if transactions_index.is_empty() {
            // create an empty FilteredBlock
            let header = FbsHeader::build(fbb, &block.header());

            let mut builder = FilteredBlockBuilder::new(fbb);
            builder.add_header(header);
            builder.finish()
        } else {
            let transactions = transactions_index
                .iter()
                .map(|ti| FbsTransaction::build(fbb, &block.transactions()[*ti]))
                .collect::<Vec<_>>();

            let proof = build_merkle_proof(
                &block
                    .transactions()
                    .iter()
                    .map(Transaction::hash)
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>(),
                transactions_index,
            );

            let proof = proof.map(|p| {
                let lemmas =
                    fbb.create_vector(&p.lemmas().iter().map(Into::into).collect::<Vec<FbsH256>>());
                let indices = fbb.create_vector(p.indices());
                let mut builder = MerkleProofBuilder::new(fbb);
                builder.add_lemmas(lemmas);
                builder.add_indices(indices);
                builder.finish()
            });

            let header = FbsHeader::build(fbb, &block.header());
            let fbs_transactions = fbb.create_vector(&transactions);

            let mut builder = FilteredBlockBuilder::new(fbb);
            builder.add_header(header);
            builder.add_transactions(fbs_transactions);
            if let Some(p) = proof {
                builder.add_proof(p);
            }
            builder.finish()
        }
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
                .transactions()
                .len()
                .saturating_sub(prefilled_transactions_len),
        );
        let mut prefilled_transactions = Vec::with_capacity(prefilled_transactions_len);

        let (key0, key1) = short_transaction_id_keys(block.header().nonce(), nonce);
        for (transaction_index, transaction) in block.transactions().iter().enumerate() {
            if prefilled_transactions_indexes.contains(&transaction_index)
                || transaction.is_cellbase()
            {
                let fbs_transaction = FbsTransaction::build(fbb, transaction);
                let mut builder = IndexTransactionBuilder::new(fbb);
                builder.add_index(transaction_index as u32);
                builder.add_transaction(fbs_transaction);
                prefilled_transactions.push(builder.finish());
            } else {
                short_ids.push(FbsBytes::build(
                    fbb,
                    &short_transaction_id(key0, key1, &transaction.witness_hash()),
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
            .proposals()
            .iter()
            .map(Into::into)
            .collect::<Vec<FbsProposalShortId>>();
        let proposals = fbb.create_vector(&vec);

        let mut builder = CompactBlockBuilder::new(fbb);
        builder.add_header(header);
        builder.add_nonce(nonce);
        builder.add_short_ids(short_ids);
        builder.add_prefilled_transactions(prefilled_transactions);
        builder.add_uncles(uncles);
        builder.add_proposals(proposals);
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
        cycles: Cycle,
    ) -> WIPOffset<RelayMessage<'b>> {
        let fbs_transaction = FbsRelayTransaction::build(fbb, transaction, cycles);
        let mut builder = RelayMessageBuilder::new(fbb);
        builder.add_payload_type(RelayPayload::RelayTransaction);
        builder.add_payload(fbs_transaction.as_union_value());
        builder.finish()
    }

    pub fn build_transaction_hash<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        tx_hash: &H256,
    ) -> WIPOffset<RelayMessage<'b>> {
        let fbs_tx_hash = FbsRelayTransactionHash::build(fbb, tx_hash);
        let mut builder = RelayMessageBuilder::new(fbb);
        builder.add_payload_type(RelayPayload::RelayTransactionHash);
        builder.add_payload(fbs_tx_hash.as_union_value());
        builder.finish()
    }

    pub fn build_get_transaction<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        tx_hash: &H256,
    ) -> WIPOffset<RelayMessage<'b>> {
        let fbs_get_tx = FbsGetRelayTransaction::build(fbb, tx_hash);
        let mut builder = RelayMessageBuilder::new(fbb);
        builder.add_payload_type(RelayPayload::GetRelayTransaction);
        builder.add_payload(fbs_get_tx.as_union_value());
        builder.finish()
    }

    pub fn build_get_block_transactions<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        block_hash: &H256,
        indexes: &[u32],
    ) -> WIPOffset<RelayMessage<'b>> {
        let get_block_transactions = {
            let fbs_block_hash = block_hash.into();
            let indexes = fbb.create_vector(indexes);
            let mut builder = GetBlockTransactionsBuilder::new(fbb);
            builder.add_block_hash(&fbs_block_hash);
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
        block_hash: &H256,
        transactions: &[Transaction],
    ) -> WIPOffset<RelayMessage<'b>> {
        let block_transactions = {
            let fbs_block_hash = block_hash.into();
            let vec = transactions
                .iter()
                .map(|transaction| FbsTransaction::build(fbb, transaction))
                .collect::<Vec<_>>();
            let transactions = fbb.create_vector(&vec);

            let mut builder = BlockTransactionsBuilder::new(fbb);
            builder.add_block_hash(&fbs_block_hash);
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
        proposals: &[ProposalShortId],
    ) -> WIPOffset<RelayMessage<'b>> {
        let get_block_proposal = {
            let vec = proposals
                .iter()
                .map(Into::into)
                .collect::<Vec<FbsProposalShortId>>();
            let proposals = fbb.create_vector(&vec);
            let mut builder = GetBlockProposalBuilder::new(fbb);
            builder.add_block_number(block_number);
            builder.add_proposals(proposals);
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

impl<'a> TimeMessage<'a> {
    pub fn build_time<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        timestamp: u64,
    ) -> WIPOffset<TimeMessage<'b>> {
        let fbs_time = FbsTime::build(fbb, timestamp);
        let mut builder = TimeMessageBuilder::new(fbb);
        builder.add_payload(fbs_time);
        builder.finish()
    }
}

impl<'a> FbsAlert<'a> {
    pub fn build<'b>(fbb: &mut FlatBufferBuilder<'b>, alert: &Alert) -> WIPOffset<FbsAlert<'b>> {
        let min_version = alert
            .min_version
            .as_ref()
            .map(|min_ver| FbsBytes::build(fbb, min_ver.as_bytes()));
        let max_version = alert
            .max_version
            .as_ref()
            .map(|max_ver| FbsBytes::build(fbb, max_ver.as_bytes()));
        let signatures = {
            let signatures: Vec<_> = alert
                .signatures
                .iter()
                .map(|sig| FbsBytes::build(fbb, sig))
                .collect();
            fbb.create_vector(&signatures)
        };
        let message = FbsBytes::build(fbb, alert.message.as_bytes());
        let mut builder = AlertBuilder::new(fbb);
        builder.add_id(alert.id);
        builder.add_cancel(alert.cancel);
        if let Some(min_version) = min_version {
            builder.add_min_version(min_version);
        }
        if let Some(max_version) = max_version {
            builder.add_max_version(max_version);
        }
        builder.add_priority(alert.priority);
        builder.add_signatures(signatures);
        builder.add_notice_until(alert.notice_until);
        builder.add_message(message);
        builder.finish()
    }
}

impl<'a> AlertMessage<'a> {
    pub fn build_alert<'b>(
        fbb: &mut FlatBufferBuilder<'b>,
        alert: &Alert,
    ) -> WIPOffset<AlertMessage<'b>> {
        let fbs_alert = FbsAlert::build(fbb, alert);
        let mut builder = AlertMessageBuilder::new(fbb);
        builder.add_payload(fbs_alert);
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
    use std::convert::TryInto;

    #[test]
    fn build_and_convert_header() {
        let header = HeaderBuilder::default().build();
        let builder = &mut FlatBufferBuilder::new();
        let b = FbsHeader::build(builder, &header);
        builder.finish(b, None);

        let fbs_header = get_root::<FbsHeader>(builder.finished_data());
        assert_eq!(header, fbs_header.try_into().unwrap());
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
        assert_eq!(block, fbs_block.try_into().unwrap());
    }

    #[test]
    fn build_compcat_block_prefilled_transactions_indexes_overflow() {
        let block = BlockBuilder::default()
            .header(HeaderBuilder::default().build())
            .transaction(TransactionBuilder::default().build())
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
