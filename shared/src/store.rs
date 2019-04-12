use crate::flat_serializer::{serialize as flat_serialize, serialized_addresses, Address};
use crate::{
    COLUMN_BLOCK_BODY, COLUMN_BLOCK_HEADER, COLUMN_BLOCK_PROPOSAL_IDS,
    COLUMN_BLOCK_TRANSACTION_ADDRESSES, COLUMN_BLOCK_UNCLE, COLUMN_EXT, COLUMN_INDEX, COLUMN_META,
    COLUMN_TRANSACTION_ADDR,
};
use bincode::{deserialize, serialize};
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::extras::{BlockExt, TransactionAddress};
use ckb_core::header::{Header, HeaderBuilder};
use ckb_core::transaction::{ProposalShortId, Transaction, TransactionBuilder};
use ckb_core::uncle::UncleBlock;
use ckb_db::{Col, DbBatch, Error, KeyValueDB};
use numext_fixed_hash::H256;
use serde::Serialize;
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

pub trait ChainStore: Sync + Send {
    type Batch: StoreBatch;
    fn new_batch(&self) -> Result<Self::Batch, Error>;

    fn get_block(&self, block_hash: &H256) -> Option<Block>;
    fn get_header(&self, block_hash: &H256) -> Option<Header>;
    fn get_block_body(&self, block_hash: &H256) -> Option<Vec<Transaction>>;
    fn get_block_proposal_txs_ids(&self, h: &H256) -> Option<Vec<ProposalShortId>>;
    fn get_block_uncles(&self, block_hash: &H256) -> Option<Vec<UncleBlock>>;
    fn get_block_ext(&self, block_hash: &H256) -> Option<BlockExt>;
}

pub trait StoreBatch {
    fn insert_block(&mut self, block: &Block) -> Result<(), Error>;
    fn insert_block_ext(&mut self, block_hash: &H256, ext: &BlockExt) -> Result<(), Error>;
    fn insert_tip_header(&mut self, header: &Header) -> Result<(), Error>;

    fn attach_block(&mut self, block: &Block) -> Result<(), Error>;
    fn detach_block(&mut self, block: &Block) -> Result<(), Error>;

    fn commit(self) -> Result<(), Error>;
}

impl<T: KeyValueDB> ChainStore for ChainKVStore<T> {
    type Batch = DefaultStoreBatch<T::Batch>;

    fn new_batch(&self) -> Result<Self::Batch, Error> {
        Ok(DefaultStoreBatch {
            inner: self.db.batch()?,
        })
    }

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
            .map(|raw| deserialize(&raw[..]).expect("deserialize uncle should be ok"))
    }

    fn get_block_proposal_txs_ids(&self, h: &H256) -> Option<Vec<ProposalShortId>> {
        self.get(COLUMN_BLOCK_PROPOSAL_IDS, h.as_bytes())
            .map(|raw| deserialize(&raw[..]).expect("deserialize proposal txs id should be ok"))
    }

    fn get_block_body(&self, h: &H256) -> Option<Vec<Transaction>> {
        self.get(COLUMN_BLOCK_TRANSACTION_ADDRESSES, h.as_bytes())
            .and_then(|serialized_addresses| {
                let addresses: Vec<Address> =
                    deserialize(&serialized_addresses).expect("deserialize address should be ok");
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
            .map(|txs| txs.into_iter().map(TransactionBuilder::build).collect())
    }

    fn get_block_ext(&self, block_hash: &H256) -> Option<BlockExt> {
        self.get(COLUMN_EXT, block_hash.as_bytes())
            .map(|raw| deserialize(&raw[..]).expect("deserialize block ext should be ok"))
    }
}

pub struct DefaultStoreBatch<B> {
    inner: B,
}

/// helper methods
impl<B: DbBatch> DefaultStoreBatch<B> {
    fn insert_raw(&mut self, col: Col, key: &[u8], value: &[u8]) -> Result<(), Error> {
        self.inner.insert(col, key, value)
    }

    fn insert_serialize<T: Serialize + ?Sized>(
        &mut self,
        col: Col,
        key: &[u8],
        item: &T,
    ) -> Result<(), Error> {
        self.inner.insert(
            col,
            key,
            &serialize(item).expect("serializing should be ok"),
        )
    }

    fn delete(&mut self, col: Col, key: &[u8]) -> Result<(), Error> {
        self.inner.delete(col, key)
    }
}

impl<B: DbBatch> StoreBatch for DefaultStoreBatch<B> {
    fn insert_block(&mut self, b: &Block) -> Result<(), Error> {
        let hash = b.header().hash();
        self.insert_serialize(COLUMN_BLOCK_HEADER, hash.as_bytes(), b.header())?;
        self.insert_serialize(COLUMN_BLOCK_UNCLE, hash.as_bytes(), b.uncles())?;
        self.insert_serialize(
            COLUMN_BLOCK_PROPOSAL_IDS,
            hash.as_bytes(),
            b.proposal_transactions(),
        )?;
        let (block_data, block_addresses) =
            flat_serialize(b.commit_transactions().iter()).expect("flat serialize should be ok");
        self.insert_raw(COLUMN_BLOCK_BODY, hash.as_bytes(), &block_data)?;
        self.insert_serialize(
            COLUMN_BLOCK_TRANSACTION_ADDRESSES,
            hash.as_bytes(),
            &block_addresses,
        )
    }

    fn insert_block_ext(&mut self, block_hash: &H256, ext: &BlockExt) -> Result<(), Error> {
        self.insert_serialize(COLUMN_EXT, block_hash.as_bytes(), ext)
    }

    fn attach_block(&mut self, block: &Block) -> Result<(), Error> {
        let hash = block.header().hash();
        let addresses = serialized_addresses(block.commit_transactions().iter())
            .expect("serialize addresses should be ok");
        for (id, tx) in block.commit_transactions().iter().enumerate() {
            let address = TransactionAddress {
                block_hash: hash.clone(),
                offset: addresses[id].offset,
                length: addresses[id].length,
            };
            self.insert_serialize(COLUMN_TRANSACTION_ADDR, tx.hash().as_bytes(), &address)?;
        }

        let number = block.header().number().to_le_bytes();
        self.insert_raw(COLUMN_INDEX, &number, hash.as_bytes())?;
        self.insert_raw(COLUMN_INDEX, hash.as_bytes(), &number)
    }

    fn detach_block(&mut self, block: &Block) -> Result<(), Error> {
        for tx in block.commit_transactions() {
            self.delete(COLUMN_TRANSACTION_ADDR, tx.hash().as_bytes())?;
        }
        self.delete(COLUMN_INDEX, &block.header().number().to_le_bytes())?;
        self.delete(COLUMN_INDEX, block.header().hash().as_bytes())
    }

    fn insert_tip_header(&mut self, h: &Header) -> Result<(), Error> {
        self.insert_raw(COLUMN_META, META_TIP_HEADER_KEY, h.hash().as_bytes())
    }

    fn commit(self) -> Result<(), Error> {
        self.inner.commit()
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
        let mut batch = store.new_batch().unwrap();
        batch.insert_block(&block).unwrap();
        batch.commit().unwrap();
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
        let mut batch = store.new_batch().unwrap();
        batch.insert_block(&block).unwrap();
        batch.commit().unwrap();
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
        let mut batch = store.new_batch().unwrap();
        batch.insert_block_ext(&hash, &ext).unwrap();
        batch.commit().unwrap();
        assert_eq!(ext, store.get_block_ext(&hash).unwrap());
    }
}
