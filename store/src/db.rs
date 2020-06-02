use crate::cache::StoreCache;
use crate::store::ChainStore;
use crate::transaction::StoreTransaction;
use crate::StoreSnapshot;
use ckb_app_config::StoreConfig;
use ckb_chain_spec::consensus::Consensus;
use ckb_db::{
    iter::{DBIter, DBIterator, IteratorMode},
    Col, DBPinnableSlice, RocksDB,
};
use ckb_error::Error;
use ckb_types::{core::BlockExt, packed, prelude::*};
use std::sync::Arc;

pub struct ChainDB {
    db: RocksDB,
    cache: Arc<StoreCache>,
}

impl<'a> ChainStore<'a> for ChainDB {
    type Vector = DBPinnableSlice<'a>;

    fn cache(&'a self) -> Option<&'a StoreCache> {
        Some(&self.cache)
    }

    fn get(&'a self, col: Col, key: &[u8]) -> Option<Self::Vector> {
        self.db
            .get_pinned(col, key)
            .expect("db operation should be ok")
    }

    fn get_iter(&self, col: Col, mode: IteratorMode) -> DBIter {
        self.db.iter(col, mode).expect("db operation should be ok")
    }
}

impl ChainDB {
    pub fn new(db: RocksDB, config: StoreConfig) -> Self {
        let cache = StoreCache::from_config(config);
        ChainDB {
            db,
            cache: Arc::new(cache),
        }
    }

    pub fn property_value(&self, col: Col, name: &str) -> Result<Option<String>, Error> {
        self.db.property_value(col, name)
    }

    pub fn property_int_value(&self, col: Col, name: &str) -> Result<Option<u64>, Error> {
        self.db.property_int_value(col, name)
    }

    pub fn begin_transaction(&self) -> StoreTransaction {
        StoreTransaction {
            inner: self.db.transaction(),
            cache: Arc::clone(&self.cache),
        }
    }

    pub fn get_snapshot(&self) -> StoreSnapshot {
        StoreSnapshot {
            inner: self.db.get_snapshot(),
            cache: Arc::clone(&self.cache),
        }
    }

    pub fn init(&self, consensus: &Consensus) -> Result<(), Error> {
        let genesis = consensus.genesis_block();
        let epoch = consensus.genesis_epoch_ext();
        let db_txn = self.begin_transaction();
        let genesis_hash = genesis.hash();
        let ext = BlockExt {
            received_at: genesis.timestamp(),
            total_difficulty: genesis.difficulty(),
            total_uncles_count: 0,
            verified: Some(true),
            txs_fees: vec![],
        };

        let transactions = genesis.transactions();
        let cells = transactions
            .iter()
            .enumerate()
            .map(move |(tx_index, tx)| {
                let tx_hash = tx.hash();
                let block_hash = genesis.header().hash();
                let block_number = genesis.header().number();
                let block_epoch = genesis.header().epoch();

                tx.outputs_with_data_iter()
                    .enumerate()
                    .map(move |(index, (cell_output, data))| {
                        let out_point = packed::OutPoint::new_builder()
                            .tx_hash(tx_hash.clone())
                            .index(index.pack())
                            .build();
                        let data_hash = packed::CellOutput::calc_data_hash(&data);

                        let entry = packed::CellEntryBuilder::default()
                            .output(cell_output)
                            .block_hash(block_hash.clone())
                            .block_number(block_number.pack())
                            .block_epoch(block_epoch.pack())
                            .index(tx_index.pack())
                            .data_size((data.len() as u64).pack())
                            .build();

                        let data_entry = packed::CellDataEntryBuilder::default()
                            .output_data(data.pack())
                            .output_data_hash(data_hash)
                            .build();

                        (out_point, entry, data_entry)
                    })
            })
            .flatten();

        db_txn.insert_cells(cells)?;

        // for tx in genesis.transactions().iter() {
        //     let outputs_len = tx.outputs().len();
        //     let tx_meta = if tx.is_cellbase() {
        //         TransactionMeta::new_cellbase(
        //             block_number,
        //             epoch_with_fraction.number(),
        //             block_hash.clone(),
        //             outputs_len,
        //             false,
        //         )
        //     } else {
        //         TransactionMeta::new(
        //             block_number,
        //             epoch_with_fraction.number(),
        //             block_hash.clone(),
        //             outputs_len,
        //             false,
        //         )
        //     };
        //     db_txn.update_cell_set(&tx.hash(), &tx_meta.pack())?;
        // }

        let last_block_hash_in_previous_epoch = epoch.last_block_hash_in_previous_epoch();

        db_txn.insert_block(genesis)?;
        db_txn.insert_block_ext(&genesis_hash, &ext)?;
        db_txn.insert_tip_header(&genesis.header())?;
        db_txn.insert_current_epoch_ext(epoch)?;
        db_txn.insert_block_epoch_index(&genesis_hash, &last_block_hash_in_previous_epoch)?;
        db_txn.insert_epoch_ext(&last_block_hash_in_previous_epoch, &epoch)?;
        db_txn.attach_block(genesis)?;
        db_txn.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::COLUMNS;
    use super::*;
    use ckb_chain_spec::consensus::ConsensusBuilder;
    use ckb_db::RocksDB;
    use ckb_types::packed;

    fn setup_db(columns: u32) -> RocksDB {
        RocksDB::open_tmp(columns)
    }

    #[test]
    fn save_and_get_block() {
        let db = setup_db(COLUMNS);
        let store = ChainDB::new(db, Default::default());
        let consensus = ConsensusBuilder::default().build();
        let block = consensus.genesis_block();

        let hash = block.hash();
        let txn = store.begin_transaction();
        txn.insert_block(&block).unwrap();
        txn.commit().unwrap();
        assert_eq!(block, &store.get_block(&hash).unwrap());
    }

    #[test]
    fn save_and_get_block_with_transactions() {
        let db = setup_db(COLUMNS);
        let store = ChainDB::new(db, Default::default());
        let block = packed::Block::new_builder()
            .transactions(
                (0..3)
                    .map(|_| packed::Transaction::new_builder().build())
                    .collect::<Vec<_>>()
                    .pack(),
            )
            .build()
            .into_view();

        let hash = block.hash();
        let txn = store.begin_transaction();
        txn.insert_block(&block).unwrap();
        txn.commit().unwrap();
        assert_eq!(block, store.get_block(&hash).unwrap());
    }

    #[test]
    fn save_and_get_block_ext() {
        let db = setup_db(COLUMNS);
        let store = ChainDB::new(db, Default::default());
        let consensus = ConsensusBuilder::default().build();
        let block = consensus.genesis_block();

        let ext = BlockExt {
            received_at: block.timestamp(),
            total_difficulty: block.difficulty(),
            total_uncles_count: block.data().uncles().len() as u64,
            verified: Some(true),
            txs_fees: vec![],
        };

        let hash = block.hash();
        let txn = store.begin_transaction();
        txn.insert_block_ext(&hash, &ext).unwrap();
        txn.commit().unwrap();
        assert_eq!(ext, store.get_block_ext(&hash).unwrap());
    }

    #[test]
    fn index_store() {
        let db = RocksDB::open_tmp(COLUMNS);
        let store = ChainDB::new(db, Default::default());
        let consensus = ConsensusBuilder::default().build();
        let block = consensus.genesis_block();
        let hash = block.hash();
        store.init(&consensus).unwrap();
        assert_eq!(hash, store.get_block_hash(0).unwrap());

        assert_eq!(
            block.difficulty(),
            store.get_block_ext(&hash).unwrap().total_difficulty
        );

        assert_eq!(block.number(), store.get_block_number(&hash).unwrap());

        assert_eq!(block.header(), store.get_tip_header().unwrap());
    }
}
