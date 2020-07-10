use ckb_app_config::StoreConfig;
use ckb_chain_iter::ChainIterator;
use ckb_db::{Result, RocksDB};
use ckb_db_migration::{Migration, ProgressBar, ProgressStyle};
use ckb_db_schema::COLUMN_NUMBER_HASH;
use ckb_store::ChainDB;
use ckb_types::{packed, prelude::*};
use std::sync::Arc;

const BATCH: usize = 1_000;

pub struct AddNumberHashMapping;

const VERSION: &str = "20200710181855";

impl Migration for AddNumberHashMapping {
    fn migrate(
        &self,
        db: RocksDB,
        pb: Arc<dyn Fn(u64) -> ProgressBar + Send + Sync>,
    ) -> Result<RocksDB> {
        let chain_db = ChainDB::new(db, StoreConfig::default());
        let iter = ChainIterator::new(&chain_db);
        let pb = pb(iter.len());
        pb.set_style(
            ProgressStyle::default_bar()
                .template(
                    "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})",
                )
                .progress_chars("#>-"),
        );
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

            if batch.len() > BATCH {
                chain_db.write(&batch)?;
                batch.clear()?;
            }
            pb.inc(1);
        }
        if !batch.is_empty() {
            chain_db.write(&batch)?;
        }
        pb.finish_with_message("finish");
        Ok(chain_db.into_inner())
    }

    fn version(&self) -> &str {
        VERSION
    }
}
