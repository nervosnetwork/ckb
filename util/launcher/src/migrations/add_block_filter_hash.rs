use ckb_app_config::StoreConfig;
use ckb_db::RocksDB;
use ckb_db_migration::{Migration, ProgressBar, ProgressStyle};
use ckb_db_schema::COLUMN_BLOCK_FILTER_HASH;
use ckb_error::Error;
use ckb_hash::blake2b_256;
use ckb_store::{ChainDB, ChainStore};
use ckb_types::prelude::Entity;
use std::sync::Arc;

pub struct AddBlockFilterHash;

const VERSION: &str = "20230206163640";

impl Migration for AddBlockFilterHash {
    fn migrate(
        &self,
        db: RocksDB,
        pb: Arc<dyn Fn(u64) -> ProgressBar + Send + Sync>,
    ) -> Result<RocksDB, Error> {
        let chain_db = ChainDB::new(db, StoreConfig::default());
        if let Some(block_hash) = chain_db.get_latest_built_filter_data_block_hash() {
            let latest_built_filter_data_block_number = if chain_db.is_main_chain(&block_hash) {
                chain_db
                    .get_block_number(&block_hash)
                    .expect("index stored")
            } else {
                // find the fork block number
                let mut header = chain_db
                    .get_block_header(&block_hash)
                    .expect("header stored");
                while !chain_db.is_main_chain(&header.parent_hash()) {
                    header = chain_db
                        .get_block_header(&header.parent_hash())
                        .expect("parent header stored");
                }
                header.number()
            };

            let pb = ::std::sync::Arc::clone(&pb);
            let pbi = pb(latest_built_filter_data_block_number + 1);
            pbi.set_style(
                    ProgressStyle::default_bar()
                        .template(
                            "{prefix:.bold.dim} {spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}",
                        )
                        .progress_chars("#>-"),
                );
            pbi.set_position(0);
            pbi.enable_steady_tick(5000);

            let mut block_number = 0;
            let mut parent_block_filter_hash = [0u8; 32];
            loop {
                let db_txn = chain_db.db().transaction();
                for _ in 0..10000 {
                    if block_number > latest_built_filter_data_block_number {
                        break;
                    }
                    let block_hash = chain_db.get_block_hash(block_number).expect("index stored");
                    let filter_data = chain_db
                        .get_block_filter(&block_hash)
                        .expect("filter data stored");
                    parent_block_filter_hash = blake2b_256(
                        [
                            parent_block_filter_hash.as_slice(),
                            filter_data.calc_raw_data_hash().as_slice(),
                        ]
                        .concat(),
                    );
                    db_txn
                        .put(
                            COLUMN_BLOCK_FILTER_HASH,
                            block_hash.as_slice(),
                            parent_block_filter_hash.as_slice(),
                        )
                        .expect("db transaction put should be ok");
                    pbi.inc(1);
                    block_number += 1;
                }
                db_txn.commit()?;

                if block_number > latest_built_filter_data_block_number {
                    break;
                }
            }
        }
        Ok(chain_db.into_inner())
    }

    fn version(&self) -> &str {
        VERSION
    }

    fn expensive(&self) -> bool {
        true
    }
}
