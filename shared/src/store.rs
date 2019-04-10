use crate::flat_serializer::{serialize as flat_serialize, serialized_addresses, Address};
use crate::{
    COLUMN_BLOCK_BODY, COLUMN_BLOCK_HEADER, COLUMN_BLOCK_PROPOSAL_IDS,
    COLUMN_BLOCK_TRANSACTION_ADDRESSES, COLUMN_BLOCK_UNCLE, COLUMN_EXT, COLUMN_INDEX, COLUMN_META,
    COLUMN_TRANSACTION_ADDR,
};
use bincode::{deserialize, serialize};
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::extras::{BlockExt, TransactionAddress};
use ckb_core::header::{BlockNumber, Header, HeaderBuilder};
use ckb_core::transaction::{ProposalShortId, Transaction, TransactionBuilder};
use ckb_core::uncle::UncleBlock;
use ckb_db::batch::{Batch, Col};
use ckb_db::kvdb::{DbBatch, KeyValueDB};
use failure::Error;
use numext_fixed_hash::H256;
use std::ops::Range;

const META_TIP_HEADER_KEY: &[u8] = b"TIP_HEADER";

pub struct ChainKVStore<T> {
    db: T,
}

impl<T: KeyValueDB> ChainKVStore<T> {
    pub fn new(db: T) -> Self {
        ChainKVStore { db }
    }

    pub fn get(&self, col: Col, key: &[u8]) -> Option<Vec<u8>> {
        self.db.read(col, key).expect("db operation should be ok")
    }

    pub fn partial_get(&self, col: Col, key: &[u8], range: &Range<usize>) -> Option<Vec<u8>> {
        self.db
            .partial_read(col, key, range)
            .expect("db operation should be ok")
    }
}

pub struct ChainStoreHeaderIterator<'a, T: ChainStore>
where
    T: 'a,
{
    store: &'a T,
    head: Option<Header>,
}

pub trait ChainStore: Sync + Send {
    type Batch: StoreBatch;

    fn get_block(&self, block_hash: &H256) -> Option<Block>;
    fn get_header(&self, block_hash: &H256) -> Option<Header>;
    fn get_block_body(&self, block_hash: &H256) -> Option<Vec<Transaction>>;
    fn get_block_proposal_txs_ids(&self, h: &H256) -> Option<Vec<ProposalShortId>>;
    fn get_block_uncles(&self, block_hash: &H256) -> Option<Vec<UncleBlock>>;
    fn get_block_ext(&self, block_hash: &H256) -> Option<BlockExt>;

    /// Visits block headers backward to genesis.
    fn headers_iter<'a>(&'a self, head: Header) -> ChainStoreHeaderIterator<'a, Self>
    where
        Self: 'a + Sized,
    {
        ChainStoreHeaderIterator {
            store: self,
            head: Some(head),
        }
    }

    fn new_batch(&self) -> Self::Batch;
}

pub trait StoreBatch {
    fn insert_block(&mut self, b: &Block);
    fn insert_block_ext(&mut self, block_hash: &H256, ext: &BlockExt);
    fn insert_block_hash(&mut self, number: BlockNumber, hash: &H256);
    fn delete_block_hash(&mut self, number: BlockNumber);
    fn insert_block_number(&mut self, hash: &H256, number: BlockNumber);
    fn delete_block_number(&mut self, hash: &H256);
    fn insert_tip_header(&mut self, h: &Header);
    fn insert_transaction_address(&mut self, block_hash: &H256, txs: &[Transaction]);
    fn delete_transaction_address(&mut self, txs: &[Transaction]);

    fn commit(self);
}

impl<'a, T: ChainStore> Iterator for ChainStoreHeaderIterator<'a, T> {
    type Item = Header;

    fn next(&mut self) -> Option<Self::Item> {
        let current_header = self.head.take();
        self.head = match current_header {
            Some(ref h) => {
                if h.number() > 0 {
                    self.store.get_header(&h.parent_hash())
                } else {
                    None
                }
            }
            None => None,
        };
        current_header
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self.head {
            Some(ref h) => (1, Some(h.number() as usize + 1)),
            None => (0, Some(0)),
        }
    }
}

impl<T: KeyValueDB> ChainStore for ChainKVStore<T> {
    type Batch = DefaultStoreBatch<T::Batch>;

    // TODO error log
    fn get_block(&self, h: &H256) -> Option<Block> {
        self.get_header(h).map(|header| {
            let commit_transactions = self
                .get_block_body(h)
                .expect("block transactions must be stored");
            let uncles = self
                .get_block_uncles(h)
                .expect("block uncles must be stored");
            let proposal_transactions = self
                .get_block_proposal_txs_ids(h)
                .expect("block proposal_ids must be stored");
            BlockBuilder::default()
                .header(header)
                .uncles(uncles)
                .commit_transactions(commit_transactions)
                .proposal_transactions(proposal_transactions)
                .build()
        })
    }

    fn get_header(&self, h: &H256) -> Option<Header> {
        self.get(COLUMN_BLOCK_HEADER, h.as_bytes())
            .map(|ref raw| HeaderBuilder::new(raw).build())
    }

    fn get_block_uncles(&self, h: &H256) -> Option<Vec<UncleBlock>> {
        // TODO Q use builder
        self.get(COLUMN_BLOCK_UNCLE, h.as_bytes())
            .map(|raw| deserialize(&raw[..]).unwrap())
    }

    fn get_block_proposal_txs_ids(&self, h: &H256) -> Option<Vec<ProposalShortId>> {
        self.get(COLUMN_BLOCK_PROPOSAL_IDS, h.as_bytes())
            .map(|raw| deserialize(&raw[..]).unwrap())
    }

    fn get_block_body(&self, h: &H256) -> Option<Vec<Transaction>> {
        self.get(COLUMN_BLOCK_TRANSACTION_ADDRESSES, h.as_bytes())
            .and_then(|serialized_addresses| {
                let addresses: Vec<Address> = deserialize(&serialized_addresses).unwrap();
                self.get(COLUMN_BLOCK_BODY, h.as_bytes())
                    .map(|serialized_body| {
                        let txs: Vec<TransactionBuilder> = addresses
                            .iter()
                            .filter_map(|address| {
                                serialized_body
                                    .get(address.offset..(address.offset + address.length))
                                    .map(TransactionBuilder::new)
                            })
                            .collect();

                        txs
                    })
            })
            .map(|txs| txs.into_iter().map(|tx| tx.build()).collect())
    }

    fn get_block_ext(&self, block_hash: &H256) -> Option<BlockExt> {
        self.get(COLUMN_EXT, block_hash.as_bytes())
            .map(|raw| deserialize(&raw[..]).unwrap())
    }

    fn new_batch(&self) -> Self::Batch {
        DefaultStoreBatch {
            inner: self.db.db_batch().expect("new db batch should be ok"),
        }
    }
}

pub struct DefaultStoreBatch<B> {
    inner: B,
}

impl<B: DbBatch> StoreBatch for DefaultStoreBatch<B> {
    fn insert_block(&mut self, b: &Block) {
        let hash = b.header().hash().to_vec();
        self.inner.insert(
            COLUMN_BLOCK_HEADER,
            &hash,
            &serialize(b.header()).expect("serializing header should be ok"),
        );
        self.inner.insert(
            COLUMN_BLOCK_UNCLE,
            &hash,
            &serialize(b.uncles()).expect("serializing uncles should be ok"),
        );
        self.inner.insert(
            COLUMN_BLOCK_PROPOSAL_IDS,
            &hash,
            &serialize(b.proposal_transactions())
                .expect("serializing proposal_transactions should be ok"),
        );
        let (block_data, block_addresses) = flat_serialize(b.commit_transactions().iter()).unwrap();
        self.inner.insert(COLUMN_BLOCK_BODY, &hash, &block_data);
        self.inner.insert(
            COLUMN_BLOCK_TRANSACTION_ADDRESSES,
            &hash,
            &serialize(&block_addresses).expect("serializing addresses should be ok"),
        );
    }

    fn insert_block_ext(&mut self, block_hash: &H256, ext: &BlockExt) {
        self.inner.insert(
            COLUMN_EXT,
            &block_hash.to_vec(),
            &serialize(ext).expect("serializing block ext should be ok"),
        );
    }

    fn insert_block_hash(&mut self, number: BlockNumber, hash: &H256) {
        let key = serialize(&number).unwrap();
        self.inner.insert(COLUMN_INDEX, &key, &hash.to_vec());
    }

    fn insert_block_number(&mut self, hash: &H256, number: BlockNumber) {
        self.inner
            .insert(COLUMN_INDEX, &hash.to_vec(), &serialize(&number).unwrap());
    }

    fn insert_transaction_address(&mut self, block_hash: &H256, txs: &[Transaction]) {
        let addresses = serialized_addresses(txs.iter()).unwrap();
        for (id, tx) in txs.iter().enumerate() {
            let address = TransactionAddress {
                block_hash: block_hash.clone(),
                offset: addresses[id].offset,
                length: addresses[id].length,
            };
            self.inner.insert(
                COLUMN_TRANSACTION_ADDR,
                &tx.hash().to_vec(),
                &serialize(&address).unwrap(),
            );
        }
    }

    fn delete_transaction_address(&mut self, txs: &[Transaction]) {
        for tx in txs {
            self.inner
                .delete(COLUMN_TRANSACTION_ADDR, &tx.hash().to_vec());
        }
    }

    fn insert_tip_header(&mut self, h: &Header) {
        self.inner.insert(
            COLUMN_META,
            &META_TIP_HEADER_KEY.to_vec(),
            &h.hash().to_vec(),
        );
    }

    fn delete_block_hash(&mut self, number: BlockNumber) {
        let key = serialize(&number).unwrap();
        self.inner.delete(COLUMN_INDEX, &key);
    }

    fn delete_block_number(&mut self, hash: &H256) {
        self.inner.delete(COLUMN_INDEX, &hash.to_vec());
    }

    fn commit(self) {
        self.inner.commit();
    }
}

#[cfg(test)]
mod tests {
    use super::super::COLUMNS;
    use super::*;
    use crate::store::StoreBatch;
    use ckb_chain_spec::consensus::Consensus;
    use ckb_db::{DBConfig, RocksDB};
    use tempfile;

    fn setup_db(prefix: &str, columns: u32) -> RocksDB {
        let tmp_dir = tempfile::Builder::new().prefix(prefix).tempdir().unwrap();
        let config = DBConfig {
            path: tmp_dir.as_ref().to_path_buf(),
            ..Default::default()
        };

        RocksDB::open(&config, columns)
    }

    #[test]
    fn save_and_get_block() {
        let db = setup_db("save_and_get_block", COLUMNS);
        let store = ChainKVStore::new(db);
        let consensus = Consensus::default();
        let block = consensus.genesis_block();

        let hash = block.header().hash();
        let mut batch = store.new_batch();
        batch.insert_block(&block);
        batch.commit();
        assert_eq!(block, &store.get_block(&hash).unwrap());
    }

    #[test]
    fn save_and_get_block_with_transactions() {
        let db = setup_db("save_and_get_block_with_transactions", COLUMNS);
        let store = ChainKVStore::new(db);
        let block = BlockBuilder::default()
            .commit_transaction(TransactionBuilder::default().build())
            .commit_transaction(TransactionBuilder::default().build())
            .commit_transaction(TransactionBuilder::default().build())
            .build();

        let hash = block.header().hash();
        let mut batch = store.new_batch();
        batch.insert_block(&block);
        batch.commit();
        assert_eq!(block, store.get_block(&hash).unwrap());
    }

    #[test]
    fn save_and_get_block_ext() {
        let db = setup_db("save_and_get_block_ext", COLUMNS);
        let store = ChainKVStore::new(db);
        let consensus = Consensus::default();
        let block = consensus.genesis_block();

        let ext = BlockExt {
            received_at: block.header().timestamp(),
            total_difficulty: block.header().difficulty().clone(),
            total_uncles_count: block.uncles().len() as u64,
            txs_verified: Some(true),
        };

        let hash = block.header().hash();
        let mut batch = store.new_batch();
        batch.insert_block_ext(&hash, &ext);
        batch.commit();
        assert_eq!(ext, store.get_block_ext(&hash).unwrap());
    }
}
