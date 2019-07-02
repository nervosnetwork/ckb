use crate::{
    COLUMN_BLOCK_BODY, COLUMN_BLOCK_EPOCH, COLUMN_BLOCK_EXT, COLUMN_BLOCK_HEADER,
    COLUMN_BLOCK_PROPOSAL_IDS, COLUMN_BLOCK_UNCLE, COLUMN_CELL_META, COLUMN_CELL_SET, COLUMN_EPOCH,
    COLUMN_INDEX, COLUMN_META, COLUMN_TRANSACTION_INFO, COLUMN_UNCLES,
};
use ckb_chain_spec::consensus::Consensus;
use ckb_core::block::{Block, BlockBuilder};
use ckb_core::cell::{BlockInfo, CellMeta};
use ckb_core::extras::{BlockExt, EpochExt, TransactionInfo};
use ckb_core::header::{BlockNumber, Header};
use ckb_core::transaction::{CellKey, CellOutPoint, CellOutput, ProposalShortId, Transaction};
use ckb_core::transaction_meta::TransactionMeta;
use ckb_core::uncle::UncleBlock;
use ckb_core::{Capacity, EpochNumber};
use ckb_db::{Col, DbBatch, Error, KeyValueDB};
use ckb_protos::{self as protos, CanBuild};
use lru_cache::LruCache;
use numext_fixed_hash::H256;
use serde_derive::{Deserialize, Serialize};
use std::convert::TryInto;
use std::ops::Range;
use std::sync::Mutex;

const META_TIP_HEADER_KEY: &[u8] = b"TIP_HEADER";
const META_CURRENT_EPOCH_KEY: &[u8] = b"CURRENT_EPOCH";

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

    fn process_get<F, Ret>(&self, col: Col, key: &[u8], process: F) -> Option<Ret>
    where
        F: FnOnce(&[u8]) -> Result<Option<Ret>, Error>,
    {
        self.db
            .process_read(col, key, process)
            .expect("db operation should be ok")
    }

    pub fn traverse<F>(&self, col: Col, callback: F) -> Result<(), Error>
    where
        F: FnMut(&[u8], &[u8]) -> Result<(), Error>,
    {
        self.db.traverse(col, callback)
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
    fn get_block_header(&self, block_hash: &H256) -> Option<Header>;
    /// Get block body by block header hash
    fn get_block_body(&self, block_hash: &H256) -> Option<Vec<Transaction>>;
    /// Get all transaction-hashes in block body by block header hash
    fn get_block_txs_hashes(&self, block_hash: &H256) -> Option<Vec<H256>>;
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
    fn get_transaction_info(&self, hash: &H256) -> Option<TransactionInfo>;
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
    fn traverse_cell_set<F>(&self, callback: F) -> Result<(), Error>
    where
        F: FnMut(H256, TransactionMeta) -> Result<(), Error>;
    fn is_uncle(&self, hash: &H256) -> bool;
    // Get cellbase by block hash
    fn get_cellbase(&self, hash: &H256) -> Option<Transaction>;
    // Get the ancestor of a base block by a block number
    fn get_ancestor(&self, base: &H256, number: BlockNumber) -> Option<Header>;
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

    fn update_cell_set(&mut self, tx_hash: &H256, meta: &TransactionMeta) -> Result<(), Error>;
    fn delete_cell_set(&mut self, tx_hash: &H256) -> Result<(), Error>;

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
        self.get_block_header(h).map(|header| {
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

    fn is_uncle(&self, hash: &H256) -> bool {
        self.get(COLUMN_UNCLES, hash.as_bytes()).is_some()
    }

    fn get_block_header(&self, hash: &H256) -> Option<Header> {
        let mut header_cache_unlocked = self
            .header_cache
            .lock()
            .expect("poisoned header cache lock");
        if let Some(header) = header_cache_unlocked.get_refresh(hash) {
            return Some(header.clone());
        }
        // release lock asap
        drop(header_cache_unlocked);

        self.process_get(COLUMN_BLOCK_HEADER, hash.as_bytes(), |slice| {
            let header: Header = protos::StoredHeader::from_slice(slice).try_into()?;
            Ok(Some(header))
        })
        .and_then(|header| {
            let mut header_cache_unlocked = self
                .header_cache
                .lock()
                .expect("poisoned header cache lock");
            header_cache_unlocked.insert(hash.clone(), header.clone());
            Some(header)
        })
    }

    fn get_block_uncles(&self, hash: &H256) -> Option<Vec<UncleBlock>> {
        self.process_get(COLUMN_BLOCK_UNCLE, hash.as_bytes(), |slice| {
            let uncles: Vec<UncleBlock> =
                protos::StoredUncleBlocks::from_slice(slice).try_into()?;
            Ok(Some(uncles))
        })
    }

    fn get_block_proposal_txs_ids(&self, hash: &H256) -> Option<Vec<ProposalShortId>> {
        self.process_get(COLUMN_BLOCK_PROPOSAL_IDS, hash.as_bytes(), |slice| {
            let short_ids: Vec<ProposalShortId> =
                protos::StoredProposalShortIds::from_slice(slice).try_into()?;
            Ok(Some(short_ids))
        })
    }

    fn get_block_body(&self, hash: &H256) -> Option<Vec<Transaction>> {
        self.process_get(COLUMN_BLOCK_BODY, hash.as_bytes(), |slice| {
            let transactions: Vec<Transaction> =
                protos::StoredBlockBody::from_slice(slice).try_into()?;
            Ok(Some(transactions))
        })
    }

    fn get_block_txs_hashes(&self, hash: &H256) -> Option<Vec<H256>> {
        self.process_get(COLUMN_BLOCK_BODY, hash.as_bytes(), |slice| {
            let tx_hashes = protos::StoredBlockBody::from_slice(slice).tx_hashes()?;
            Ok(Some(tx_hashes))
        })
    }

    fn get_block_ext(&self, block_hash: &H256) -> Option<BlockExt> {
        self.process_get(COLUMN_BLOCK_EXT, block_hash.as_bytes(), |slice| {
            let ext: BlockExt = protos::BlockExt::from_slice(slice).try_into()?;
            Ok(Some(ext))
        })
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
            verified: Some(true),
            txs_fees: vec![],
        };

        let mut cells = Vec::with_capacity(genesis.transactions().len());

        for tx in genesis.transactions() {
            let tx_meta;
            let ins = if tx.is_cellbase() {
                tx_meta = TransactionMeta::new_cellbase(
                    genesis.header().number(),
                    genesis.header().epoch(),
                    tx.outputs().len(),
                    false,
                );
                Vec::new()
            } else {
                tx_meta = TransactionMeta::new(
                    genesis.header().number(),
                    genesis.header().epoch(),
                    tx.outputs().len(),
                    false,
                );
                tx.input_pts_iter().cloned().collect()
            };
            batch.update_cell_set(tx.hash(), &tx_meta)?;
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
        self.get(COLUMN_INDEX, hash.as_bytes()).map(|raw| {
            let le_bytes: [u8; 8] = raw[..].try_into().expect("should not be failed");
            u64::from_le_bytes(le_bytes)
        })
    }

    fn get_tip_header(&self) -> Option<Header> {
        self.get(COLUMN_META, META_TIP_HEADER_KEY)
            .and_then(|raw| {
                self.get_block_header(&H256::from_slice(&raw[..]).expect("db safe access"))
            })
            .map(Into::into)
    }

    fn get_current_epoch_ext(&self) -> Option<EpochExt> {
        self.process_get(COLUMN_META, META_CURRENT_EPOCH_KEY, |slice| {
            let ext: EpochExt = protos::StoredEpochExt::from_slice(slice).try_into()?;
            Ok(Some(ext))
        })
    }

    fn get_epoch_ext(&self, hash: &H256) -> Option<EpochExt> {
        self.process_get(COLUMN_EPOCH, hash.as_bytes(), |slice| {
            let ext: EpochExt = protos::StoredEpochExt::from_slice(slice).try_into()?;
            Ok(Some(ext))
        })
    }

    fn get_epoch_index(&self, number: EpochNumber) -> Option<H256> {
        self.get(COLUMN_EPOCH, &number.to_le_bytes())
            .map(|raw| H256::from_slice(&raw[..]).expect("db safe access"))
    }

    fn get_block_epoch_index(&self, block_hash: &H256) -> Option<H256> {
        self.get(COLUMN_BLOCK_EPOCH, block_hash.as_bytes())
            .map(|raw| H256::from_slice(&raw[..]).expect("db safe access"))
    }

    fn get_transaction(&self, hash: &H256) -> Option<(Transaction, H256)> {
        self.get_transaction_info(&hash).and_then(|info| {
            self.process_get(COLUMN_BLOCK_BODY, info.block_hash.as_bytes(), |slice| {
                let tx_opt = protos::StoredBlockBody::from_slice(slice).transaction(info.index)?;
                Ok(tx_opt)
            })
            .map(|tx| (tx, info.block_hash))
        })
    }

    fn get_transaction_info(&self, hash: &H256) -> Option<TransactionInfo> {
        self.process_get(COLUMN_TRANSACTION_INFO, hash.as_bytes(), |slice| {
            let info: TransactionInfo =
                protos::StoredTransactionInfo::from_slice(slice).try_into()?;
            Ok(Some(info))
        })
    }

    fn get_cell_meta(&self, tx_hash: &H256, index: u32) -> Option<CellMeta> {
        self.process_get(
            COLUMN_CELL_META,
            CellKey::calculate(tx_hash, index).as_ref(),
            |slice| {
                let meta: (Capacity, H256) =
                    protos::StoredCellMeta::from_slice(slice).try_into()?;
                Ok(Some(meta))
            },
        )
        .and_then(|meta| {
            self.get_transaction_info(tx_hash)
                .map(|tx_info| (tx_info, meta))
        })
        .map(|(tx_info, meta)| {
            let out_point = CellOutPoint {
                tx_hash: tx_hash.to_owned(),
                index: index as u32,
            };
            let cellbase = tx_info.index == 0;
            let block_info = BlockInfo {
                number: tx_info.block_number,
                epoch: tx_info.block_epoch,
            };
            let (capacity, data_hash) = meta;
            CellMeta {
                cell_output: None,
                out_point,
                block_info: Some(block_info),
                cellbase,
                capacity,
                data_hash: Some(data_hash),
            }
        })
    }

    fn get_cellbase(&self, hash: &H256) -> Option<Transaction> {
        self.process_get(COLUMN_BLOCK_BODY, hash.as_bytes(), |slice| {
            let cellbase = protos::StoredBlockBody::from_slice(slice)
                .transaction(0)?
                .expect("cellbase address should exist");
            Ok(Some(cellbase))
        })
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

        self.get_transaction_info(&tx_hash)
            .and_then(|info| {
                self.process_get(COLUMN_BLOCK_BODY, info.block_hash.as_bytes(), |slice| {
                    let output_opt = protos::StoredBlockBody::from_slice(slice)
                        .output(info.index, index as usize)?;
                    Ok(output_opt)
                })
            })
            .map(|cell_output: CellOutput| {
                let mut cell_output_cache_unlocked = self
                    .cell_output_cache
                    .lock()
                    .expect("poisoned cell output cache lock");
                cell_output_cache_unlocked.insert((tx_hash.clone(), index), cell_output.clone());
                cell_output
            })
    }

    fn traverse_cell_set<F>(&self, mut callback: F) -> Result<(), Error>
    where
        F: FnMut(H256, TransactionMeta) -> Result<(), Error>,
    {
        self.traverse(COLUMN_CELL_SET, |hash_slice, tx_meta_bytes| {
            let tx_hash = H256::from_slice(hash_slice).expect("deserialize tx hash should be ok");
            let tx_meta: TransactionMeta =
                protos::TransactionMeta::from_slice(tx_meta_bytes).try_into()?;
            callback(tx_hash, tx_meta)
        })
    }

    fn get_ancestor(&self, base: &H256, number: BlockNumber) -> Option<Header> {
        if let Some(header) = self.get_block_header(base) {
            let mut n_number = header.number();
            let mut index_walk = header;
            if number > n_number {
                return None;
            }

            while n_number > number {
                if let Some(header) = self.get_block_header(&index_walk.parent_hash()) {
                    index_walk = header;
                    n_number -= 1;
                } else {
                    return None;
                }
            }
            return Some(index_walk);
        }
        None
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

    fn delete(&mut self, col: Col, key: &[u8]) -> Result<(), Error> {
        self.inner.delete(col, key)
    }
}

impl<B: DbBatch> StoreBatch for DefaultStoreBatch<B> {
    fn insert_block(&mut self, block: &Block) -> Result<(), Error> {
        let hash = block.header().hash().as_bytes();
        {
            let builder = protos::StoredHeader::full_build(block.header());
            self.insert_raw(COLUMN_BLOCK_HEADER, hash, builder.as_slice())?;
        }
        {
            let builder = protos::StoredUncleBlocks::full_build(block.uncles());
            self.insert_raw(COLUMN_BLOCK_UNCLE, hash, builder.as_slice())?;
        }
        {
            let builder = protos::StoredProposalShortIds::full_build(block.proposals());
            self.insert_raw(COLUMN_BLOCK_PROPOSAL_IDS, hash, builder.as_slice())?;
        }
        {
            let builder = protos::StoredBlockBody::full_build(block.transactions());
            self.insert_raw(COLUMN_BLOCK_BODY, hash, builder.as_slice())?;
        }
        Ok(())
    }

    fn insert_block_ext(&mut self, block_hash: &H256, ext: &BlockExt) -> Result<(), Error> {
        let builder = protos::BlockExt::full_build(ext);
        self.insert_raw(COLUMN_BLOCK_EXT, block_hash.as_bytes(), builder.as_slice())
    }

    fn attach_block(&mut self, block: &Block) -> Result<(), Error> {
        let header = block.header();
        let hash = header.hash();
        for (index, tx) in block.transactions().iter().enumerate() {
            let tx_hash = tx.hash();
            {
                let info = TransactionInfo {
                    block_hash: hash.to_owned(),
                    block_number: header.number(),
                    block_epoch: header.epoch(),
                    index,
                };
                let builder = protos::StoredTransactionInfo::full_build(&info);
                self.insert_raw(
                    COLUMN_TRANSACTION_INFO,
                    tx_hash.as_bytes(),
                    builder.as_slice(),
                )?;
            }
            for (cell_index, output) in tx.outputs().iter().enumerate() {
                let out_point = CellOutPoint {
                    tx_hash: tx_hash.to_owned(),
                    index: cell_index as u32,
                };
                let store_key = out_point.cell_key();
                let data = (output.capacity, output.data_hash());
                let builder = protos::StoredCellMeta::full_build(&data);
                self.insert_raw(COLUMN_CELL_META, store_key.as_ref(), builder.as_slice())?;
            }
        }

        let number = block.header().number().to_le_bytes();
        self.insert_raw(COLUMN_INDEX, &number, hash.as_bytes())?;
        for uncle in block.uncles() {
            self.insert_raw(COLUMN_UNCLES, &uncle.hash().as_bytes(), &[])?;
        }
        self.insert_raw(COLUMN_INDEX, hash.as_bytes(), &number)
    }

    fn detach_block(&mut self, block: &Block) -> Result<(), Error> {
        for tx in block.transactions() {
            let tx_hash = tx.hash();
            self.delete(COLUMN_TRANSACTION_INFO, tx_hash.as_bytes())?;
            for index in 0..tx.outputs().len() {
                let store_key = CellKey::calculate(&tx_hash, index as u32);
                self.delete(COLUMN_CELL_META, store_key.as_ref())?;
            }
        }

        for uncle in block.uncles() {
            self.delete(COLUMN_UNCLES, &uncle.hash().as_bytes())?;
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
        let builder = protos::StoredEpochExt::full_build(epoch);
        self.insert_raw(COLUMN_EPOCH, epoch_index, builder.as_slice())?;
        self.insert_raw(COLUMN_EPOCH, &epoch_number, epoch_index)
    }

    fn insert_current_epoch_ext(&mut self, epoch: &EpochExt) -> Result<(), Error> {
        let builder = protos::StoredEpochExt::full_build(epoch);
        self.insert_raw(COLUMN_META, META_CURRENT_EPOCH_KEY, builder.as_slice())
    }

    fn update_cell_set(&mut self, tx_hash: &H256, meta: &TransactionMeta) -> Result<(), Error> {
        let builder = protos::TransactionMeta::full_build(meta);
        self.insert_raw(COLUMN_CELL_SET, tx_hash.as_bytes(), builder.as_slice())
    }

    fn delete_cell_set(&mut self, tx_hash: &H256) -> Result<(), Error> {
        self.delete(COLUMN_CELL_SET, tx_hash.as_bytes())
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
            verified: Some(true),
            txs_fees: vec![],
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
