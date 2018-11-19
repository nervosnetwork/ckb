use bigint::{H256, H48};
use byteorder::{ByteOrder, LittleEndian, WriteBytesExt};
use ckb_protocol::{
    self, build_header_args, build_transaction_args, build_uncle_block_args,
    CompactBlock as FbsCompactBlock, CompactBlockArgs, FlatbuffersVectorIterator,
    Header as FbsHeader, PrefilledTransaction as FbsPrefilledTransaction, PrefilledTransactionArgs,
    RelayMessage, RelayMessageArgs, RelayPayload, Transaction as FbsTransaction,
    UncleBlock as FbsUncleBlock,
};
use core::block::IndexedBlock;
use core::header::Header;
use core::transaction::{ProposalShortId, Transaction};
use core::uncle::UncleBlock;
use flatbuffers::FlatBufferBuilder;
use hash::sha3_256;
use rand::{thread_rng, Rng};
use siphasher::sip::SipHasher;
use std::collections::HashSet;
use std::hash::Hasher;

pub type ShortTransactionID = H48;

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct CompactBlock {
    pub header: Header,
    pub uncles: Vec<UncleBlock>,
    pub nonce: u64,
    pub short_ids: Vec<ShortTransactionID>,
    pub prefilled_transactions: Vec<PrefilledTransaction>,
    pub proposal_transactions: Vec<ProposalShortId>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct PrefilledTransaction {
    pub index: usize,
    pub transaction: Transaction,
}

impl CompactBlock {
    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn to_vec(&self) -> Vec<u8> {
        let builder = &mut FlatBufferBuilder::new();
        {
            let compact_block_args = build_compact_block_args(builder, self);
            let payload =
                Some(FbsCompactBlock::create(builder, &compact_block_args).as_union_value());

            let payload_type = RelayPayload::CompactBlock;
            let message = RelayMessage::create(
                builder,
                &RelayMessageArgs {
                    payload_type,
                    payload,
                },
            );
            builder.finish(message, None);
        }
        builder.finished_data().to_vec()
    }
}

impl PrefilledTransaction {
    pub fn transaction(&self) -> &Transaction {
        &self.transaction
    }
}

pub struct CompactBlockBuilder<'a, S: ::std::hash::BuildHasher + 'a> {
    block: &'a IndexedBlock,
    prefilled_transactions_indexes: &'a HashSet<usize, S>,
}

impl<'a, S: ::std::hash::BuildHasher> CompactBlockBuilder<'a, S> {
    pub fn new(
        block: &'a IndexedBlock,
        prefilled_transactions_indexes: &'a HashSet<usize, S>,
    ) -> Self {
        CompactBlockBuilder {
            block,
            prefilled_transactions_indexes,
        }
    }

    pub fn build(&self) -> CompactBlock {
        let nonce: u64 = thread_rng().gen();

        let prefilled_transactions_len = self.prefilled_transactions_indexes.len();
        let mut short_ids: Vec<ShortTransactionID> =
            Vec::with_capacity(self.block.commit_transactions.len() - prefilled_transactions_len);
        let mut prefilled_transactions: Vec<PrefilledTransaction> =
            Vec::with_capacity(prefilled_transactions_len);

        let (key0, key1) = short_transaction_id_keys(nonce, &self.block.header);
        for (transaction_index, transaction) in self.block.commit_transactions.iter().enumerate() {
            if self
                .prefilled_transactions_indexes
                .contains(&transaction_index)
                || transaction.is_cellbase()
            {
                prefilled_transactions.push(PrefilledTransaction {
                    index: transaction_index,
                    transaction: transaction.clone().into(),
                })
            } else {
                short_ids.push(short_transaction_id(key0, key1, &transaction.hash()));
            }
        }

        CompactBlock {
            header: self.block.header.clone().into(),
            uncles: self.block.uncles.clone(),
            nonce,
            short_ids,
            prefilled_transactions,
            proposal_transactions: self.block.proposal_transactions.clone(),
        }
    }
}

pub fn short_transaction_id_keys(nonce: u64, header: &Header) -> (u64, u64) {
    // sha3-256(header.nonce + random nonce) in little-endian
    let mut bytes = vec![];
    bytes.write_u64::<LittleEndian>(header.seal.nonce).unwrap();
    bytes.write_u64::<LittleEndian>(nonce).unwrap();
    let block_header_with_nonce_hash = sha3_256(bytes);

    let key0 = LittleEndian::read_u64(&block_header_with_nonce_hash[0..8]);
    let key1 = LittleEndian::read_u64(&block_header_with_nonce_hash[8..16]);

    (key0, key1)
}

pub fn short_transaction_id(key0: u64, key1: u64, transaction_hash: &H256) -> ShortTransactionID {
    let mut hasher = SipHasher::new_with_keys(key0, key1);
    hasher.write(transaction_hash);
    let siphash_transaction_hash = hasher.finish();

    let mut siphash_transaction_hash_bytes = [0u8; 8];
    LittleEndian::write_u64(
        &mut siphash_transaction_hash_bytes,
        siphash_transaction_hash,
    );

    siphash_transaction_hash_bytes[0..6].into()
}

impl<'a> From<ckb_protocol::CompactBlock<'a>> for CompactBlock {
    fn from(b: ckb_protocol::CompactBlock<'a>) -> Self {
        CompactBlock {
            header: b.header().unwrap().into(),
            nonce: b.nonce(),
            short_ids: b
                .short_ids()
                .unwrap()
                .chunks(6)
                .map(ShortTransactionID::from)
                .collect(),
            prefilled_transactions: FlatbuffersVectorIterator::new(
                b.prefilled_transactions().unwrap(),
            ).map(Into::into)
            .collect(),

            uncles: FlatbuffersVectorIterator::new(b.uncles().unwrap())
                .map(Into::into)
                .collect(),

            proposal_transactions: b
                .proposal_transactions()
                .unwrap()
                .chunks(10)
                .filter_map(|s| ProposalShortId::from_slice(s))
                .collect(),
        }
    }
}

impl<'a> From<ckb_protocol::PrefilledTransaction<'a>> for PrefilledTransaction {
    fn from(pt: ckb_protocol::PrefilledTransaction<'a>) -> Self {
        PrefilledTransaction {
            index: pt.index() as usize,
            transaction: pt.transaction().unwrap().into(),
        }
    }
}

pub fn build_compact_block_args<'a>(
    builder: &mut FlatBufferBuilder<'a>,
    block: &CompactBlock,
) -> CompactBlockArgs<'a> {
    let header_args = build_header_args(builder, &block.header);
    let header = Some(FbsHeader::create(builder, &header_args));

    let vec = block
        .uncles
        .iter()
        .map(|uncle| {
            let uncle_args = build_uncle_block_args(builder, uncle);
            FbsUncleBlock::create(builder, &uncle_args)
        }).collect::<Vec<_>>();
    let uncles = Some(builder.create_vector(&vec));

    let vec = block
        .proposal_transactions
        .iter()
        .flat_map(|id| id.iter().cloned())
        .collect::<Vec<_>>();
    let proposal_transactions = Some(builder.create_vector(&vec));

    let vec = block
        .short_ids
        .iter()
        .flat_map(|id| id.iter().cloned())
        .collect::<Vec<_>>();
    let short_ids = Some(builder.create_vector(&vec));

    let vec = block
        .prefilled_transactions
        .iter()
        .map(|pt| {
            let transaction_args = build_transaction_args(builder, &pt.transaction);
            let transaction = Some(FbsTransaction::create(builder, &transaction_args));
            FbsPrefilledTransaction::create(
                builder,
                &PrefilledTransactionArgs {
                    index: pt.index as u32,
                    transaction,
                },
            )
        }).collect::<Vec<_>>();
    let prefilled_transactions = Some(builder.create_vector(&vec));

    CompactBlockArgs {
        header,
        nonce: block.nonce,
        short_ids,
        prefilled_transactions,
        uncles,
        proposal_transactions,
    }
}
