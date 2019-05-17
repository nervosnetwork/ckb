use crate::flat_block_body::{
    deserialize_block_body, deserialize_transaction, serialize_block_body,
    serialize_block_body_size, TransactionAddressInner, TransactionAddressStored,
};
use crate::{
    COLUMN_BLOCK_BODY, COLUMN_BLOCK_EPOCH, COLUMN_BLOCK_HEADER, COLUMN_BLOCK_PROPOSAL_IDS,
    COLUMN_BLOCK_TRANSACTION_ADDRESSES, COLUMN_BLOCK_UNCLE, COLUMN_CELL_META, COLUMN_EPOCH,
    COLUMN_EXT, COLUMN_INDEX, COLUMN_META, COLUMN_TRANSACTION_ADDR,
};
use bincode::{deserialize, serialize};
use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::cell::{BlockInfo, CellMeta};
use ckb_core::extras::{
    BlockExt, DaoStats, EpochExt, TransactionAddress, DEFAULT_ACCUMULATED_RATE,
};
use ckb_core::header::{BlockNumber, Header};
use ckb_core::transaction::{CellOutPoint, CellOutput, ProposalShortId, Transaction};
use ckb_core::uncle::UncleBlock;
use ckb_core::{Capacity, EpochNumber};
use ckb_db::{Col, DbBatch, Error, KeyValueDB};
use lru_cache::LruCache;
use numext_fixed_hash::H256;
use serde_derive::{Deserialize, Serialize};
use std::ops::Range;
use std::sync::Mutex;

const META_TIP_HEADER_KEY: &[u8] = b"TIP_HEADER";
const META_CURRENT_EPOCH_KEY: &[u8] = b"CURRENT_EPOCH";

fn cell_store_key(tx_hash: &H256, index: u32) -> [u8; 36] {
    let mut key: [u8; 36] = [0; 36];
    key[..32].copy_from_slice(tx_hash.as_bytes());
    key[32..36].copy_from_slice(&index.to_be_bytes());
    key
}

#[derive(Clone, Serialize, Deserialize, Eq, PartialEq, Hash, Debug)]
pub struct StoreConfig {
    pub header_cache_size: usize,
    pub cell_output_cache_size: usize,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            header_cache_size: 4096,
            cell_output_cache_size: 128,
        }
    }
}

pub struct ChainKVStore<T> {
    db: T,
    header_cache: Mutex<LruCache<H256, Header>>,
    cell_output_cache: Mutex<LruCache<(H256, u32), CellOutput>>,
}

impl<T: KeyValueDB> ChainKVStore<T> {
    pub fn new(db: T) -> Self {
        Self::with_config(db, StoreConfig::default())
    }

    pub fn with_config(db: T, config: StoreConfig) -> Self {
        ChainKVStore {
            db,
            header_cache: Mutex::new(LruCache::new(config.header_cache_size)),
            cell_output_cache: Mutex::new(LruCache::new(config.cell_output_cache_size)),
        }
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

    fn init(&self, consensus: &Consensus) -> Result<(), Error>;
    /// Get block header hash by block number
    fn get_block_hash(&self, number: BlockNumber) -> Option<H256>;
    /// Get block number by block header hash
    fn get_block_number(&self, hash: &H256) -> Option<BlockNumber>;
    /// Get the tip(highest) header
    fn get_tip_header(&self) -> Option<Header>;
    /// Get commit transaction and block hash by it's hash
    fn get_transaction(&self, h: &H256) -> Option<(Transaction, H256)>;
    fn get_transaction_address(&self, hash: &H256) -> Option<TransactionAddress>;
    fn get_cell_meta(&self, tx_hash: &H256, index: u32) -> Option<CellMeta>;
    fn get_cell_output(&self, tx_hash: &H256, index: u32) -> Option<CellOutput>;
    // Get current epoch ext
    fn get_current_epoch_ext(&self) -> Option<EpochExt>;
    // Get epoch ext by epoch index
    fn get_epoch_ext(&self, hash: &H256) -> Option<EpochExt>;
    // Get epoch index by epoch number
    fn get_epoch_index(&self, number: EpochNumber) -> Option<H256>;
    // Get epoch index by block hash
    fn get_block_epoch_index(&self, h256: &H256) -> Option<H256>;
}

pub trait StoreBatch {
    fn insert_block(&mut self, block: &Block) -> Result<(), Error>;
    fn insert_block_ext(&mut self, block_hash: &H256, ext: &BlockExt) -> Result<(), Error>;
    fn insert_tip_header(&mut self, header: &Header) -> Result<(), Error>;
    fn insert_current_epoch_ext(&mut self, epoch: &EpochExt) -> Result<(), Error>;
    fn insert_block_epoch_index(
        &mut self,
        block_hash: &H256,
        epoch_hash: &H256,
    ) -> Result<(), Error>;
    fn insert_epoch_ext(&mut self, hash: &H256, epoch: &EpochExt) -> Result<(), Error>;

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

    fn get_header(&self, hash: &H256) -> Option<Header> {
        let mut header_cache_unlocked = self
            .header_cache
            .lock()
            .expect("poisoned header cache lock");
        if let Some(header) = header_cache_unlocked.get_refresh(hash) {
            return Some(header.clone());
        }
        // release lock asap
        drop(header_cache_unlocked);

        self.get(COLUMN_BLOCK_HEADER, hash.as_bytes())
            .map(|ref raw| unsafe { Header::from_bytes_with_hash_unchecked(raw, hash.to_owned()) })
            .and_then(|header| {
                let mut header_cache_unlocked = self
                    .header_cache
                    .lock()
                    .expect("poisoned header cache lock");
                header_cache_unlocked.insert(hash.clone(), header.clone());
                Some(header)
            })
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
                let tx_addresses: Vec<TransactionAddressInner> = deserialize(&serialized_addresses)
                    .expect("flat deserialize address should be ok");
                self.get(COLUMN_BLOCK_BODY, h.as_bytes())
                    .map(|serialized_body| {
                        deserialize_block_body(&serialized_body, &tx_addresses[..])
                    })
            })
    }

    fn get_block_ext(&self, block_hash: &H256) -> Option<BlockExt> {
        self.get(COLUMN_EXT, block_hash.as_bytes())
            .map(|raw| deserialize(&raw[..]).expect("deserialize block ext should be ok"))
    }

    fn init(&self, consensus: &Consensus) -> Result<(), Error> {
        let genesis = consensus.genesis_block();
        let epoch = consensus.genesis_epoch_ext();
        let mut batch = self.new_batch()?;
        let genesis_hash = genesis.header().hash();
        let ext = BlockExt {
            received_at: genesis.header().timestamp(),
            total_difficulty: genesis.header().difficulty().clone(),
            total_uncles_count: 0,
            txs_verified: Some(true),
            dao_stats: DaoStats {
                accumulated_rate: DEFAULT_ACCUMULATED_RATE,
                accumulated_capacity: genesis
                    .transactions()
                    .get(0)
                    .map(|tx| {
                        tx.outputs()
                            .iter()
                            .skip(1)
                            .try_fold(Capacity::zero(), |capacity, output| {
                                capacity.safe_add(output.capacity)
                            })
                            .unwrap()
                    })
                    .unwrap_or_else(Capacity::zero)
                    .as_u64(),
            },
        };

        let mut cells = Vec::with_capacity(genesis.transactions().len());

        for tx in genesis.transactions() {
            let ins = if tx.is_cellbase() {
                Vec::new()
            } else {
                tx.input_pts_iter().cloned().collect()
            };
            let outs = tx.output_pts();

            cells.push((ins, outs));
        }

        batch.insert_block(genesis)?;
        batch.insert_block_ext(&genesis_hash, &ext)?;
        batch.insert_tip_header(&genesis.header())?;
        batch.insert_current_epoch_ext(epoch)?;
        batch.insert_block_epoch_index(&genesis_hash, epoch.last_block_hash_in_previous_epoch())?;
        batch.insert_epoch_ext(epoch.last_block_hash_in_previous_epoch(), &epoch)?;
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

    fn get_current_epoch_ext(&self) -> Option<EpochExt> {
        self.get(COLUMN_META, META_CURRENT_EPOCH_KEY)
            .map(|raw| deserialize(&raw[..]).expect("db safe access"))
    }

    fn get_epoch_ext(&self, hash: &H256) -> Option<EpochExt> {
        self.get(COLUMN_EPOCH, hash.as_bytes())
            .map(|raw| deserialize(&raw[..]).expect("db safe access"))
    }

    fn get_epoch_index(&self, number: EpochNumber) -> Option<H256> {
        self.get(COLUMN_EPOCH, &number.to_le_bytes())
            .map(|raw| H256::from_slice(&raw[..]).expect("db safe access"))
    }

    fn get_block_epoch_index(&self, block_hash: &H256) -> Option<H256> {
        self.get(COLUMN_BLOCK_EPOCH, block_hash.as_bytes())
            .map(|raw| H256::from_slice(&raw[..]).expect("db safe access"))
    }

    fn get_transaction(&self, h: &H256) -> Option<(Transaction, H256)> {
        self.get(COLUMN_TRANSACTION_ADDR, h.as_bytes())
            .map(|raw| deserialize(&raw[..]).expect("deserialize tx address should be ok"))
            .and_then(|addr: TransactionAddressStored| {
                self.partial_get(
                    COLUMN_BLOCK_BODY,
                    addr.block_hash.as_bytes(),
                    &(addr.inner.offset..(addr.inner.offset + addr.inner.length)),
                )
                .map(|ref serialized_transaction| {
                    (
                        deserialize_transaction(
                            serialized_transaction,
                            &addr.inner.outputs_addresses,
                        )
                        .expect("flat deserialize tx should be ok"),
                        addr.block_hash,
                    )
                })
            })
    }

    fn get_transaction_address(&self, h: &H256) -> Option<TransactionAddress> {
        self.get(COLUMN_TRANSACTION_ADDR, h.as_bytes())
            .map(|raw| deserialize(&raw[..]).expect("deserialize tx address should be ok"))
            .map(|stored: TransactionAddressStored| TransactionAddress {
                block_hash: stored.block_hash,
                offset: stored.inner.offset,
                length: stored.inner.length,
            })
    }

    fn get_cell_meta(&self, tx_hash: &H256, index: u32) -> Option<CellMeta> {
        self.get(COLUMN_CELL_META, &cell_store_key(tx_hash, index))
            .map(|raw| deserialize(&raw[..]).unwrap())
    }

    fn get_cell_output(&self, tx_hash: &H256, index: u32) -> Option<CellOutput> {
        let mut cell_output_cache_unlocked = self
            .cell_output_cache
            .lock()
            .expect("poisoned cell output cache lock");
        if let Some(cell_output) = cell_output_cache_unlocked.get_refresh(&(tx_hash.clone(), index))
        {
            return Some(cell_output.clone());
        }
        // release lock asap
        drop(cell_output_cache_unlocked);

        self.get(COLUMN_TRANSACTION_ADDR, tx_hash.as_bytes())
            .map(|raw| deserialize(&raw[..]).expect("deserialize tx address should be ok"))
            .and_then(|stored: TransactionAddressStored| {
                stored
                    .inner
                    .outputs_addresses
                    .get(index as usize)
                    .and_then(|addr| {
                        let output_offset = stored.inner.offset + addr.offset;
                        self.partial_get(
                            COLUMN_BLOCK_BODY,
                            stored.block_hash.as_bytes(),
                            &(output_offset..(output_offset + addr.length)),
                        )
                        .map(|ref serialized_cell_output| {
                            let cell_output: CellOutput = deserialize(serialized_cell_output)
                                .expect("flat deserialize cell output should be ok");
                            let mut cell_output_cache_unlocked = self
                                .cell_output_cache
                                .lock()
                                .expect("poisoned cell output cache lock");
                            cell_output_cache_unlocked
                                .insert((tx_hash.clone(), index), cell_output.clone());
                            cell_output.to_owned()
                        })
                    })
            })
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

    fn insert_serialize<T: serde::ser::Serialize + ?Sized>(
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
    fn insert_block(&mut self, block: &Block) -> Result<(), Error> {
        let hash = block.header().hash();
        self.insert_serialize(COLUMN_BLOCK_HEADER, hash.as_bytes(), block.header())?;
        self.insert_serialize(COLUMN_BLOCK_UNCLE, hash.as_bytes(), block.uncles())?;
        self.insert_serialize(
            COLUMN_BLOCK_PROPOSAL_IDS,
            hash.as_bytes(),
            block.proposals(),
        )?;
        let (block_data, block_addresses) = serialize_block_body(block.transactions())
            .expect("flat serialize block body should be ok");
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
        let (_, tx_addresses) = serialize_block_body_size(block.transactions())
            .expect("flat serialize tx addresses should be ok");
        for (id, (tx, addr)) in block
            .transactions()
            .iter()
            .zip(tx_addresses.into_iter())
            .enumerate()
        {
            let tx_hash = tx.hash();
            self.insert_serialize(
                COLUMN_TRANSACTION_ADDR,
                tx_hash.as_bytes(),
                &addr.into_stored(hash.to_owned()),
            )?;
            let cellbase = id == 0;
            for (index, output) in tx.outputs().iter().enumerate() {
                let out_point = CellOutPoint {
                    tx_hash: tx_hash.to_owned(),
                    index: index as u32,
                };
                let store_key = cell_store_key(&tx_hash, index as u32);
                let cell_meta = CellMeta {
                    cell_output: None,
                    out_point,
                    block_info: Some(BlockInfo {
                        number: block.header().number(),
                        epoch: block.header().epoch(),
                    }),
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

    fn insert_block_epoch_index(
        &mut self,
        block_hash: &H256,
        epoch_hash: &H256,
    ) -> Result<(), Error> {
        self.insert_raw(
            COLUMN_BLOCK_EPOCH,
            block_hash.as_bytes(),
            epoch_hash.as_bytes(),
        )
    }

    fn insert_epoch_ext(&mut self, hash: &H256, epoch: &EpochExt) -> Result<(), Error> {
        let epoch_index = hash.as_bytes();
        let epoch_number = epoch.number().to_le_bytes();
        self.insert_serialize(COLUMN_EPOCH, epoch_index, epoch)?;
        self.insert_raw(COLUMN_EPOCH, &epoch_number, epoch_index)
    }

    fn insert_current_epoch_ext(&mut self, epoch: &EpochExt) -> Result<(), Error> {
        self.insert_serialize(COLUMN_META, META_CURRENT_EPOCH_KEY, epoch)
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
    use ckb_core::transaction::TransactionBuilder;
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
            total_difficulty: block.header().difficulty().to_owned(),
            total_uncles_count: block.uncles().len() as u64,
            txs_verified: Some(true),
            dao_stats: DaoStats {
                accumulated_rate: DEFAULT_ACCUMULATED_RATE,
                accumulated_capacity: block.outputs_capacity().unwrap().as_u64(),
            },
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
        store.init(&consensus).unwrap();
        assert_eq!(hash, &store.get_block_hash(0).unwrap());

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
