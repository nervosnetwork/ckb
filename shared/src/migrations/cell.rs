use ckb_app_config::StoreConfig;
use ckb_db::RocksDB;
use ckb_db_migration::{Migration, ProgressBar, ProgressStyle};
use ckb_error::Error;
use ckb_store::ChainStore;
use ckb_store::{ChainDB, StoreWriteBatch, COLUMN_CELL};
use ckb_types::{
    core::{BlockView, TransactionView},
    packed,
    prelude::*,
};
use std::sync::{Arc, Barrier};
use std::thread;

const RESTORE_CELL_VERSION: &str = "20200707214700";
const BATCH: usize = 1_000;
const MAX_DELETE_BATCH_SIZE: usize = 32 * 1024;
const MAX_THREAD: u64 = 6;
const MIN_THREAD: u64 = 2;

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

        let tb_num = std::cmp::max(MIN_THREAD, num_cpus::get() as u64);
        let tb_num = std::cmp::min(tb_num, MAX_THREAD);
        let chunk_size = tip_number / tb_num;
        let remainder = tip_number % tb_num;
        let barrier = Arc::new(Barrier::new(tb_num as usize));

        let handles: Vec<_> = (0..tb_num).map(|i| {
            let chain_db = chain_db.clone();
            let pb = Arc::clone(&pb);
            let c = Arc::clone(&barrier);

            let last = i == (tb_num - 1);
            let size = if last {
                chunk_size + remainder
            } else {
                chunk_size
            };
            let end = if last {
                tip_number + 1
            } else {
                (i + 1) * chunk_size
            };

            let pbi = pb(size * 2);
            pbi.set_style(
                ProgressStyle::default_bar()
                    .template(
                        "{prefix:.bold.dim} {spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}",
                    )
                    .progress_chars("#>-"),
            );
            pbi.set_position(0);
            pbi.enable_steady_tick(5000);
            thread::spawn(move || {
                let mut wb = chain_db.new_write_batch();

                let mut hashes = Vec::new();
                for number in i * chunk_size..end {
                    let block = chain_db
                        .get_block_hash(number)
                        .and_then(|hash| chain_db.get_block(&hash)).unwrap();

                    if block.transactions().len() > 1 {
                        hashes.push(block.hash());
                    }
                    insert_block_cell(&mut wb, &block);

                    if wb.len() > BATCH {
                        chain_db.write(&wb).unwrap();
                        wb.clear().unwrap();
                    }
                    pbi.inc(1);
                }

                if !wb.is_empty() {
                    chain_db.write(&wb).unwrap();
                    wb.clear().unwrap();
                }

                // wait all cell insert
                c.wait();

                pbi.set_length(size + hashes.len() as u64);

                for hash in hashes {
                    let txs = chain_db.get_block_body(&hash);

                    delete_consumed_cell(&mut wb, &txs);
                    if wb.size_in_bytes() > MAX_DELETE_BATCH_SIZE {
                        chain_db.write(&wb).unwrap();
                        wb.clear().unwrap();
                    }
                    pbi.inc(1);
                }

                if !wb.is_empty() {
                    chain_db.write(&wb).unwrap();
                }
                pbi.finish_with_message("done!");
            })
        }).collect();

        // Wait for other threads to finish.
        for handle in handles {
            handle.join().unwrap();
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

fn insert_block_cell(batch: &mut StoreWriteBatch, block: &BlockView) {
    let transactions = block.transactions();

    // add new live cells
    let new_cells = transactions
        .iter()
        .enumerate()
        .map(move |(tx_index, tx)| {
            let tx_hash = tx.hash();
            let block_hash = block.header().hash();
            let block_number = block.header().number();
            let block_epoch = block.header().epoch();

            tx.outputs_with_data_iter()
                .enumerate()
                .map(move |(index, (cell_output, data))| {
                    let out_point = packed::OutPoint::new_builder()
                        .tx_hash(tx_hash.clone())
                        .index(index.pack())
                        .build();

                    let entry = packed::CellEntryBuilder::default()
                        .output(cell_output)
                        .block_hash(block_hash.clone())
                        .block_number(block_number.pack())
                        .block_epoch(block_epoch.pack())
                        .index(tx_index.pack())
                        .data_size((data.len() as u64).pack())
                        .build();

                    let data_entry = if !data.is_empty() {
                        let data_hash = packed::CellOutput::calc_data_hash(&data);
                        Some(
                            packed::CellDataEntryBuilder::default()
                                .output_data(data.pack())
                                .output_data_hash(data_hash)
                                .build(),
                        )
                    } else {
                        None
                    };

                    (out_point, entry, data_entry)
                })
        })
        .flatten();
    batch.insert_cells(new_cells).unwrap();
}

fn delete_consumed_cell(batch: &mut StoreWriteBatch, transactions: &[TransactionView]) {
    // mark inputs dead
    // skip cellbase
    let deads = transactions
        .iter()
        .skip(1)
        .map(|tx| tx.input_pts_iter())
        .flatten();
    batch.delete_cells(deads).unwrap();
}
