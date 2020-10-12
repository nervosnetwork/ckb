use ckb_app_config::StoreConfig;
use ckb_db::RocksDB;
use ckb_db_migration::{Migration, ProgressBar, ProgressStyle};
use ckb_error::Error;
use ckb_store::ChainStore;
use ckb_store::{attach_block_cell, ChainDB, COLUMN_CELL};
use std::sync::Arc;
use std::thread;

const RESTORE_CELL_VERSION: &str = "20200707214700";
const BATCH: usize = 500_000_000;

pub struct CellMigration;

impl Migration for CellMigration {
    fn migrate(
        &self,
        mut db: RocksDB,
        pb: Arc<dyn Fn(u64) -> ProgressBar + Send + Sync>,
    ) -> Result<RocksDB, Error> {
        clean_cell_column(&mut db)?;

        let chain_db = ChainDB::new(db, StoreConfig::default());
        let tip = chain_db.get_tip_header().unwrap();
        let tip_number = tip.number();

        let tb_num = std::cmp::max(2, num_cpus::get() as u64);
        let tb_num = std::cmp::min(tb_num, 4); // max 4 avoid resource busy

        let chunk_size = tip_number / tb_num;
        let remainder = tip_number % tb_num;

        let tbj: Vec<_> = (0..tb_num).map(|i| {
            let chain_db = chain_db.clone();
            let pb = Arc::clone(&pb);
            thread::spawn(move || {
                let last = i == (tb_num - 1);
                let size = if last {
                    chunk_size + remainder
                } else {
                    chunk_size
                };
                let pbi = pb(size);
                pbi.set_style(
                    ProgressStyle::default_bar()
                        .template(
                            "{prefix:.bold.dim} {spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})",
                        )
                        .progress_chars("#>-"),
                );
                pbi.enable_steady_tick(10000);

                let mut count = 0;
                let mut txn = chain_db.begin_transaction();

                let end = if last {
                    tip_number
                } else {
                    (i + 1) * chunk_size
                };
                for number in i * chunk_size..end {
                    let block = chain_db
                        .get_block_hash(number)
                        .and_then(|hash| chain_db.get_block(&hash))
                        .unwrap();
                    attach_block_cell(&txn, &block).unwrap();

                    // Header 208 bytes
                    let estimate = block.data().serialized_size_without_uncle_proposals() - 208;
                    count += estimate;
                    if count > BATCH {
                        txn.commit().unwrap();
                        txn = chain_db.begin_transaction();
                        count = 0;
                    }
                    pbi.inc(1);
                }

                if count != 0 {
                    txn.commit().unwrap();
                }
                pbi.finish_with_message("finish");
            })
        }).collect();

        for j in tbj {
            j.join().unwrap();
        }
        Ok(chain_db.into_inner())
    }

    fn version(&self) -> &str {
        RESTORE_CELL_VERSION
    }
}

// https://github.com/facebook/rocksdb/issues/1295
fn clean_cell_column(db: &mut RocksDB) -> Result<(), Error> {
    db.drop_cf(COLUMN_CELL)?;
    db.create_cf(COLUMN_CELL)?;
    Ok(())
}
