use crate::store::ChainStore;
use crate::transaction::StoreTransaction;
use crate::COLUMN_CELL_SET;
use ckb_chain_spec::consensus::Consensus;
use ckb_core::extras::BlockExt;
use ckb_core::transaction_meta::TransactionMeta;
use ckb_db::{Col, DBPinnableSlice, Error, RocksDB};
use ckb_protos as protos;
use numext_fixed_hash::H256;
use std::convert::TryInto;

pub struct ChainDB {
    db: RocksDB,
}

impl<'a> ChainStore<'a> for ChainDB {
    type Vector = DBPinnableSlice<'a>;

    fn get(&'a self, col: Col, key: &[u8]) -> Option<Self::Vector> {
        self.db
            .get_pinned(col, key)
            .expect("db operation should be ok")
    }
}

impl ChainDB {
    pub fn new(db: RocksDB) -> Self {
        ChainDB { db }
    }

    pub fn traverse_cell_set<F>(&self, mut callback: F) -> Result<(), Error>
    where
        F: FnMut(H256, TransactionMeta) -> Result<(), Error>,
    {
        self.db
            .traverse(COLUMN_CELL_SET, |hash_slice, tx_meta_bytes| {
                let tx_hash =
                    H256::from_slice(hash_slice).expect("deserialize tx hash should be ok");
                let tx_meta: TransactionMeta =
                    protos::TransactionMeta::from_slice(tx_meta_bytes).try_into()?;
                callback(tx_hash, tx_meta)
            })
    }

    pub fn begin_transaction(&self) -> StoreTransaction {
        StoreTransaction {
            inner: self.db.transaction(),
        }
    }

    pub fn init(&self, consensus: &Consensus) -> Result<(), Error> {
        let genesis = consensus.genesis_block();
        let epoch = consensus.genesis_epoch_ext();
        let db_txn = self.begin_transaction();
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
                    genesis.header().hash().to_owned(),
                    tx.outputs().len(),
                    false,
                );
                Vec::new()
            } else {
                tx_meta = TransactionMeta::new(
                    genesis.header().number(),
                    genesis.header().epoch(),
                    genesis.header().hash().to_owned(),
                    tx.outputs().len(),
                    false,
                );
                tx.input_pts_iter().cloned().collect()
            };
            db_txn.update_cell_set(tx.hash(), &tx_meta)?;
            let outs = tx.output_pts();

            cells.push((ins, outs));
        }

        db_txn.insert_block(genesis)?;
        db_txn.insert_block_ext(&genesis_hash, &ext)?;

        db_txn.insert_tip_header(genesis.header())?;
        db_txn.insert_current_epoch_ext(epoch)?;
        db_txn
            .insert_block_epoch_index(&genesis_hash, epoch.last_block_hash_in_previous_epoch())?;
        db_txn.insert_epoch_ext(epoch.last_block_hash_in_previous_epoch(), &epoch)?;
        db_txn.attach_block(genesis)?;
        db_txn.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::COLUMNS;
    use super::*;
    use ckb_chain_spec::consensus::Consensus;
    use ckb_core::block::BlockBuilder;
    use ckb_core::transaction::TransactionBuilder;
    use ckb_db::RocksDB;

    fn setup_db(columns: u32) -> RocksDB {
        RocksDB::open_tmp(columns)
    }

    #[test]
    fn save_and_get_block() {
        let db = setup_db(COLUMNS);
        let store = ChainDB::new(db);
        let consensus = Consensus::default();
        let block = consensus.genesis_block();

        let hash = block.header().hash();
        let txn = store.begin_transaction();
        txn.insert_block(&block).unwrap();
        txn.commit().unwrap();
        assert_eq!(block, &store.get_block(&hash).unwrap());
    }

    #[test]
    fn save_and_get_block_with_transactions() {
        let db = setup_db(COLUMNS);
        let store = ChainDB::new(db);
        let block = BlockBuilder::default()
            .transaction(TransactionBuilder::default().build())
            .transaction(TransactionBuilder::default().build())
            .transaction(TransactionBuilder::default().build())
            .build();

        let hash = block.header().hash();
        let txn = store.begin_transaction();
        txn.insert_block(&block).unwrap();
        txn.commit().unwrap();
        assert_eq!(block, store.get_block(&hash).unwrap());
    }

    #[test]
    fn save_and_get_block_ext() {
        let db = setup_db(COLUMNS);
        let store = ChainDB::new(db);
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
        let txn = store.begin_transaction();
        txn.insert_block_ext(&hash, &ext).unwrap();
        txn.commit().unwrap();
        assert_eq!(ext, store.get_block_ext(&hash).unwrap());
    }

    #[test]
    fn index_store() {
        let db = RocksDB::open_tmp(COLUMNS);
        let store = ChainDB::new(db);
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
