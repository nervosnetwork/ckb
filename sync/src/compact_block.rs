use bigint::{H256, H48};
use byteorder::{ByteOrder, LittleEndian, WriteBytesExt};
use core::block::Block;
use core::header::Header;
use core::transaction::Transaction;
use hash::sha3_256;
use nervos_protocol;
use protobuf::RepeatedField;
use rand::{thread_rng, Rng};
use siphasher::sip::SipHasher;
use std::collections::HashSet;
use std::hash::Hasher;

pub type ShortTransactionID = H48;

#[derive(Debug, PartialEq)]
pub struct CompactBlock {
    pub header: Header,
    pub nonce: u64,
    pub short_ids: Vec<ShortTransactionID>,
    pub prefilled_transactions: Vec<PrefilledTransaction>,
}

#[derive(Debug, PartialEq)]
pub struct PrefilledTransaction {
    pub index: usize,
    pub transaction: Transaction,
}

impl CompactBlock {
    pub fn header(&self) -> &Header {
        &self.header
    }
}

impl PrefilledTransaction {
    pub fn transaction(&self) -> &Transaction {
        &self.transaction
    }
}

pub fn build_compact_block<S: ::std::hash::BuildHasher>(
    block: &Block,
    prefilled_transactions_indexes: &HashSet<usize, S>,
) -> CompactBlock {
    let nonce: u64 = thread_rng().gen();

    let prefilled_transactions_len = prefilled_transactions_indexes.len();
    let mut short_ids: Vec<ShortTransactionID> =
        Vec::with_capacity(block.transactions.len() - prefilled_transactions_len);
    let mut prefilled_transactions: Vec<PrefilledTransaction> =
        Vec::with_capacity(prefilled_transactions_len);

    let (key0, key1) = short_transaction_id_keys(nonce, &block.header);
    for (transaction_index, transaction) in block.transactions.iter().enumerate() {
        // Since cellbase transaction is very unlikely to be included in mem pool,
        // we will always include it in prefilled transaction
        if prefilled_transactions_indexes.contains(&transaction_index) || transaction.is_cellbase()
        {
            prefilled_transactions.push(PrefilledTransaction {
                index: transaction_index,
                transaction: transaction.clone(),
            })
        } else {
            short_ids.push(short_transaction_id(key0, key1, &transaction.hash()));
        }
    }

    CompactBlock {
        header: block.header.clone(),
        nonce,
        short_ids,
        prefilled_transactions,
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

impl<'a> From<&'a nervos_protocol::CompactBlock> for CompactBlock {
    fn from(b: &'a nervos_protocol::CompactBlock) -> Self {
        CompactBlock {
            header: b.get_block_header().into(),
            nonce: b.get_nonce(),
            short_ids: b
                .get_short_ids()
                .iter()
                .map(|hash| ShortTransactionID::from_slice(&hash[..]))
                .collect(),
            prefilled_transactions: b
                .get_prefilled_transactions()
                .iter()
                .map(|t| t.into())
                .collect(),
        }
    }
}

impl From<CompactBlock> for nervos_protocol::CompactBlock {
    fn from(b: CompactBlock) -> Self {
        let mut block = nervos_protocol::CompactBlock::new();
        block.set_block_header(b.header().into());
        block.set_nonce(b.nonce);
        block.set_short_ids(RepeatedField::from_vec(
            b.short_ids
                .iter()
                .map(|short_id| short_id.to_vec())
                .collect(),
        ));
        block.set_prefilled_transactions(RepeatedField::from_vec(
            b.prefilled_transactions.iter().map(Into::into).collect(),
        ));
        block
    }
}

impl<'a> From<&'a nervos_protocol::PrefilledTransaction> for PrefilledTransaction {
    fn from(pt: &'a nervos_protocol::PrefilledTransaction) -> Self {
        PrefilledTransaction {
            index: pt.get_index() as usize,
            transaction: pt.get_transaction().into(),
        }
    }
}

impl<'a> From<&'a PrefilledTransaction> for nervos_protocol::PrefilledTransaction {
    fn from(pt: &'a PrefilledTransaction) -> Self {
        let mut prefilled_transaction = nervos_protocol::PrefilledTransaction::new();
        prefilled_transaction.set_index(pt.index as u32);
        prefilled_transaction.set_transaction(pt.transaction().into());
        prefilled_transaction
    }
}
