use crate::flat_serializer::{serialize as flat_serialize, serialized_addresses, Address};
use crate::{
    COLUMN_BLOCK_BODY, COLUMN_BLOCK_HEADER, COLUMN_BLOCK_PROPOSAL_IDS,
    COLUMN_BLOCK_TRANSACTION_ADDRESSES, COLUMN_BLOCK_UNCLE, COLUMN_CELL_META, COLUMN_EXT,
    COLUMN_INDEX, COLUMN_META, COLUMN_TRANSACTION_ADDR,
};
use bincode::{deserialize, serialize};
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::cell::CellMeta;
use ckb_core::extras::{BlockExt, TransactionAddress};
use ckb_core::header::{BlockNumber, Header, HeaderBuilder};
use ckb_core::transaction::{
    CellOutput, OutPoint, ProposalShortId, Transaction, TransactionBuilder,
};
use ckb_core::uncle::UncleBlock;
use ckb_db::{Col, DbBatch, Error, KeyValueDB};
use numext_fixed_hash::H256;
use serde::Serialize;
use std::ops::Range;

const META_TIP_HEADER_KEY: &[u8] = b"TIP_HEADER";

fn cell_store_key(tx_hash: &H256, index: u32) -> Vec<u8> {
    let mut key: [u8; 36] = [0; 36];
    key[..32].copy_from_slice(tx_hash.as_bytes());
    key[32..36].copy_from_slice(&index.to_be_bytes());
    key.to_vec()
}

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

/// Store interface by chain
pub trait ChainStore: Sync + Send {
    /// Batch handle
    type Batch: StoreBatch;
    /// New a store batch handle
    fn new_batch(&self) -> Result<Self::Batch, Error>;

    /// Get block by block header hash
    fn get_block(&self, block_hash: &H256) -> Option<Block>;
    /// Get header by block header hash
    fn get_header(&self, block_hash: &H256) -> Option<Header>;
    /// Get block body by block header hash
    fn get_block_body(&self, block_hash: &H256) -> Option<Vec<Transaction>>;
    /// Get proposal short id by block header hash
    fn get_block_proposal_txs_ids(&self, h: &H256) -> Option<Vec<ProposalShortId>>;
    /// Get block uncles by block header hash
    fn get_block_uncles(&self, block_hash: &H256) -> Option<Vec<UncleBlock>>;
    /// Get block ext by block header hash
    fn get_block_ext(&self, block_hash: &H256) -> Option<BlockExt>;

    /// Init by genesis
    fn init(&self, genesis: &Block) -> Result<(), Error>;
    /// Get block header hash by block number
    fn get_block_hash(&self, number: BlockNumber) -> Option<H256>;
    /// Get block number by block header hash
    fn get_block_number(&self, hash: &H256) -> Option<BlockNumber>;
    /// Get the tip(highest) header
    fn get_tip_header(&self) -> Option<Header>;
    /// Get commit transaction and block hash by it's hash
    fn get_transaction(&self, h: &H256) -> Option<(Transaction, H256)>;
    /// Get commit transaction address by it's hash
    fn get_transaction_address(&self, hash: &H256) -> Option<TransactionAddress>;
    fn get_cell_meta(&self, tx_hash: &H256, index: u32) -> Option<CellMeta>;
    fn get_cell_output(&self, tx_hash: &H256, index: u32) -> Option<CellOutput>;
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
            let transactions = self
                .get_block_body(h)
                .expect("block transactions must be stored");
            let uncles = self
                .get_block_uncles(h)
                .expect("block uncles must be stored");
            let proposals = self
                .get_block_proposal_txs_ids(h)
                .expect("block proposal_ids must be stored");
            BlockBuilder::default()
                .header(header)
                .uncles(uncles)
                .transactions(transactions)
                .proposals(proposals)
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

    fn init(&self, genesis: &Block) -> Result<(), Error> {
        let mut batch = self.new_batch()?;
        let genesis_hash = genesis.header().hash();
        let ext = BlockExt {
            received_at: genesis.header().timestamp(),
            total_difficulty: genesis.header().difficulty().clone(),
            total_uncles_count: 0,
            txs_verified: Some(true),
        };

        let mut cells = Vec::with_capacity(genesis.transactions().len());

        for tx in genesis.transactions() {
            let ins = if tx.is_cellbase() {
                Vec::new()
            } else {
                tx.input_pts()
            };
            let outs = tx.output_pts();

            cells.push((ins, outs));
        }

        batch.insert_block(genesis)?;
        batch.insert_block_ext(&genesis_hash, &ext)?;
        batch.insert_tip_header(&genesis.header())?;
        batch.attach_block(genesis)?;
        batch.commit()
    }

    fn get_block_hash(&self, number: BlockNumber) -> Option<H256> {
        self.get(COLUMN_INDEX, &number.to_le_bytes())
            .map(|raw| H256::from_slice(&raw[..]).expect("db safe access"))
    }

    fn get_block_number(&self, hash: &H256) -> Option<BlockNumber> {
        self.get(COLUMN_INDEX, hash.as_bytes())
            .map(|raw| deserialize(&raw[..]).unwrap())
    }

    fn get_tip_header(&self) -> Option<Header> {
        self.get(COLUMN_META, META_TIP_HEADER_KEY)
            .and_then(|raw| self.get_header(&H256::from_slice(&raw[..]).expect("db safe access")))
            .map(Into::into)
    }

    fn get_transaction(&self, h: &H256) -> Option<(Transaction, H256)> {
        self.get_transaction_address(h).and_then(|d| {
            self.partial_get(
                COLUMN_BLOCK_BODY,
                d.block_hash.as_bytes(),
                &(d.offset..(d.offset + d.length)),
            )
            .map(|ref serialized_transaction| {
                (
                    TransactionBuilder::new(serialized_transaction).build(),
                    d.block_hash,
                )
            })
        })
    }

    fn get_transaction_address(&self, h: &H256) -> Option<TransactionAddress> {
        self.get(COLUMN_TRANSACTION_ADDR, h.as_bytes())
            .map(|raw| deserialize(&raw[..]).unwrap())
    }

    fn get_cell_meta(&self, tx_hash: &H256, index: u32) -> Option<CellMeta> {
        self.get(COLUMN_CELL_META, &cell_store_key(tx_hash, index))
            .map(|raw| deserialize(&raw[..]).unwrap())
    }

    // TODO build index for cell_output, avoid load the whole tx
    fn get_cell_output(&self, tx_hash: &H256, index: u32) -> Option<CellOutput> {
        self.get_transaction(tx_hash)
            .and_then(|(tx, _)| tx.outputs().get(index as usize).map(ToOwned::to_owned))
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
        self.insert_serialize(COLUMN_BLOCK_PROPOSAL_IDS, hash.as_bytes(), b.proposals())?;
        let (block_data, block_addresses) =
            flat_serialize(b.transactions().iter()).expect("flat serialize should be ok");
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
        let addresses = serialized_addresses(block.transactions().iter())
            .expect("serialize addresses should be ok");
        for (id, tx) in block.transactions().iter().enumerate() {
            let address = TransactionAddress {
                block_hash: hash.clone(),
                offset: addresses[id].offset,
                length: addresses[id].length,
            };
            let tx_hash = tx.hash();
            self.insert_serialize(COLUMN_TRANSACTION_ADDR, tx_hash.as_bytes(), &address)?;
            let cellbase = id == 0;
            for (index, output) in tx.outputs().iter().enumerate() {
                let out_point = OutPoint {
                    tx_hash: tx_hash.clone(),
                    index: index as u32,
                };
                let store_key = cell_store_key(&tx_hash, index as u32);
                let cell_meta = CellMeta {
                    cell_output: None,
                    out_point,
                    block_number: Some(block.header().number()),
                    cellbase,
                    capacity: output.capacity,
                    data_hash: Some(output.data_hash()),
                };
                self.insert_serialize(COLUMN_CELL_META, &store_key, &cell_meta)?;
            }
        }

        let number = block.header().number().to_le_bytes();
        self.insert_raw(COLUMN_INDEX, &number, hash.as_bytes())?;
        self.insert_raw(COLUMN_INDEX, hash.as_bytes(), &number)
    }

    fn detach_block(&mut self, block: &Block) -> Result<(), Error> {
        for tx in block.transactions() {
            let tx_hash = tx.hash();
            self.delete(COLUMN_TRANSACTION_ADDR, tx_hash.as_bytes())?;
            for index in 0..tx.outputs().len() {
                let store_key = cell_store_key(&tx_hash, index as u32);
                self.delete(COLUMN_CELL_META, &store_key)?;
            }
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
            .transaction(TransactionBuilder::default().build())
            .transaction(TransactionBuilder::default().build())
            .transaction(TransactionBuilder::default().build())
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

    #[test]
    fn index_store() {
        let tmp_dir = tempfile::Builder::new()
            .prefix("index_init")
            .tempdir()
            .unwrap();
        let config = DBConfig {
            path: tmp_dir.as_ref().to_path_buf(),
            ..Default::default()
        };
        let db = RocksDB::open(&config, COLUMNS);
        let store = ChainKVStore::new(db);
        let consensus = Consensus::default();
        let block = consensus.genesis_block();
        let hash = block.header().hash();
        store.init(&block).unwrap();
        assert_eq!(&hash, &store.get_block_hash(0).unwrap());

        assert_eq!(
            block.header().difficulty(),
            &store.get_block_ext(&hash).unwrap().total_difficulty
        );

        assert_eq!(
            block.header().number(),
            store.get_block_number(&hash).unwrap()
        );

        assert_eq!(block.header(), &store.get_tip_header().unwrap());
    }
}
