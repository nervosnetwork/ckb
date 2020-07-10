use ckb_app_config::StoreConfig;
use ckb_chain_iter::ChainIterator;
use ckb_db::{Result, RocksDB};
use ckb_db_migration::Migration;
use ckb_store::{ChainDB, COLUMN_NUMBER_HASH};
use ckb_types::{packed, prelude::*};
use indicatif::{ProgressBar, ProgressStyle};

const BATCH: u64 = 10_000;

pub struct AddNumberHashMapping;

const VERSION: &str = "20200710181855";

impl Migration for AddNumberHashMapping {
    fn migrate(&self, db: RocksDB) -> Result<RocksDB> {
        let chain_db = ChainDB::new(db, StoreConfig::default());
        let iter = ChainIterator::new(&chain_db);
        let pb = ProgressBar::new(iter.len());
        pb.set_style(
            ProgressStyle::default_bar()
                .template(
                    "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})",
                )
                .progress_chars("#>-"),
        );
        let mut count = 0;
        let mut batch = chain_db.new_write_batch();
        for block in iter {
            let txs_len: packed::Uint32 = (block.transactions().len() as u32).pack();
            batch.put(
                COLUMN_NUMBER_HASH,
                packed::NumberHash::new_builder()
                    .number(block.number().pack())
                    .block_hash(block.header().hash())
                    .build()
                    .as_slice(),
                txs_len.as_slice(),
            )?;
            count += 1;
            if count == BATCH {
                chain_db.write(&batch)?;
                batch.clear()?;
                count = 0;
            }
            pb.inc(1);
        }
        if count != 0 {
            chain_db.write(&batch)?;
        }
        pb.finish_with_message("finish");
        Ok(chain_db.into_inner())
    }

    fn version(&self) -> &str {
        VERSION
    }
}
