use bigint::{H256, H48};
use byteorder::{ByteOrder, LittleEndian, WriteBytesExt};
use ckb_protocol;
use core::block::IndexedBlock;
use core::header::Header;
use core::transaction::{ProposalShortId, Transaction};
use core::uncle::UncleBlock;
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

impl<'a> From<&'a ckb_protocol::CompactBlock> for CompactBlock {
    fn from(b: &'a ckb_protocol::CompactBlock) -> Self {
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
                .map(Into::into)
                .collect(),
            uncles: b.get_uncles().iter().map(Into::into).collect(),
            proposal_transactions: b
                .get_proposal_transactions()
                .iter()
                .filter_map(|id| ProposalShortId::from_slice(&id))
                .collect(),
        }
    }
}

impl From<CompactBlock> for ckb_protocol::CompactBlock {
    fn from(b: CompactBlock) -> Self {
        let mut block = ckb_protocol::CompactBlock::new();
        block.set_block_header(b.header().into());
        block.set_nonce(b.nonce);
        block.set_short_ids(
            b.short_ids
                .iter()
                .map(|short_id| short_id.to_vec())
                .collect(),
        );
        block.set_uncles(b.uncles.iter().map(Into::into).collect());
        block.set_prefilled_transactions(b.prefilled_transactions.iter().map(Into::into).collect());
        block.set_proposal_transactions(
            b.proposal_transactions.iter().map(|t| t.to_vec()).collect(),
        );
        block
    }
}

impl<'a> From<&'a ckb_protocol::PrefilledTransaction> for PrefilledTransaction {
    fn from(pt: &'a ckb_protocol::PrefilledTransaction) -> Self {
        PrefilledTransaction {
            index: pt.get_index() as usize,
            transaction: pt.get_transaction().into(),
        }
    }
}

impl<'a> From<&'a PrefilledTransaction> for ckb_protocol::PrefilledTransaction {
    fn from(pt: &'a PrefilledTransaction) -> Self {
        let mut prefilled_transaction = ckb_protocol::PrefilledTransaction::new();
        prefilled_transaction.set_index(pt.index as u32);
        prefilled_transaction.set_transaction(pt.transaction().into());
        prefilled_transaction
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bigint::U256;
    use core::block::IndexedBlock;
    use core::header::{RawHeader, Seal};
    use core::transaction::{IndexedTransaction, ProposalShortId};
    use protobuf;
    use protobuf::Message;

    fn dummy_block() -> IndexedBlock {
        let cellbase = dummy_cellbase();
        let uncles = vec![dummy_uncle(), dummy_uncle()];
        let header = Header {
            raw: RawHeader {
                number: 0,
                version: 0,
                parent_hash: H256::zero(),
                timestamp: 10,
                txs_commit: H256::zero(),
                txs_proposal: H256::zero(),
                difficulty: U256::zero(),
                cellbase_id: cellbase.hash(),
                uncles_hash: H256::zero(),
            },
            seal: Seal {
                nonce: 0,
                mix_hash: H256::zero(),
            },
        };

        IndexedBlock {
            header: header.into(),
            uncles,
            commit_transactions: vec![cellbase],
            proposal_transactions: vec![ProposalShortId::from_slice(&[1; 10]).unwrap()],
        }
    }

    fn dummy_cellbase() -> IndexedTransaction {
        use core::transaction::{CellInput, CellOutput, VERSION};

        let inputs = vec![CellInput::new_cellbase_input(0)];
        let outputs = vec![CellOutput::new(0, vec![], H256::from(0))];
        Transaction::new(VERSION, vec![], inputs, outputs).into()
    }

    fn dummy_uncle() -> UncleBlock {
        let cellbase = dummy_cellbase();
        let header = Header {
            raw: RawHeader {
                number: 0,
                version: 0,
                parent_hash: H256::zero(),
                timestamp: 10,
                txs_commit: H256::zero(),
                txs_proposal: H256::zero(),
                difficulty: U256::zero(),
                cellbase_id: cellbase.hash(),
                uncles_hash: H256::zero(),
            },
            seal: Seal {
                nonce: 0,
                mix_hash: H256::zero(),
            },
        };
        UncleBlock {
            header,
            cellbase: cellbase.into(),
            proposal_transactions: vec![ProposalShortId::from_slice(&[1; 10]).unwrap()],
        }
    }

    #[test]
    fn test_proto_convert() {
        let block = dummy_block();
        let cmpt_block = CompactBlockBuilder::new(&block, &HashSet::new()).build();
        let proto_cmpt_block: ckb_protocol::CompactBlock = cmpt_block.clone().into();

        let message = proto_cmpt_block.write_to_bytes().unwrap();
        let decoded_proto_cmpt_block =
            protobuf::parse_from_bytes::<ckb_protocol::CompactBlock>(&message).unwrap();
        assert_eq!(proto_cmpt_block, decoded_proto_cmpt_block);
        let decoded_cmpt_block: CompactBlock = (&decoded_proto_cmpt_block).into();
        assert_eq!(cmpt_block, decoded_cmpt_block);
    }
}
