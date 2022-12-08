use ckb_app_config::StoreConfig;
use ckb_db::{Result, RocksDB};
use ckb_db_migration::{Migration, ProgressBar, ProgressStyle};
use ckb_error::InternalErrorKind;
use ckb_store::{ChainDB, ChainStore};
use ckb_types::utilities::merkle_mountain_range::ChainRootMMR;
use std::sync::Arc;

pub struct AddChainRootMMR;

const VERSION: &str = "20221208151540";

impl Migration for AddChainRootMMR {
    fn migrate(
        &self,
        db: RocksDB,
        pb: Arc<dyn Fn(u64) -> ProgressBar + Send + Sync>,
    ) -> Result<RocksDB> {
        let chain_db = ChainDB::new(db, StoreConfig::default());
        let tip = chain_db
            .get_tip_header()
            .ok_or_else(|| InternalErrorKind::MMR.other("tip block is not found"))?;
        let tip_number = tip.number();

        let pb = ::std::sync::Arc::clone(&pb);
        let pbi = pb(tip_number + 1);
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
        let mut mmr_size = 0;

        loop {
            let db_txn = chain_db.begin_transaction();
            let mut mmr = ChainRootMMR::new(mmr_size, &db_txn);

            for _ in 0..10000 {
                if block_number > tip_number {
                    break;
                }

                let block_hash = chain_db.get_block_hash(block_number).ok_or_else(|| {
                    let err = format!(
                        "tip is {} but hash for block#{} is not found",
                        tip_number, block_number
                    );
                    InternalErrorKind::Database.other(err)
                })?;
                let block_header = chain_db.get_block_header(&block_hash).ok_or_else(|| {
                    let err = format!(
                        "tip is {} but hash for block#{} ({:#x}) is not found",
                        tip_number, block_number, block_hash
                    );
                    InternalErrorKind::Database.other(err)
                })?;
                mmr.push(block_header.digest())
                    .map_err(|e| InternalErrorKind::MMR.other(e))?;
                pbi.inc(1);

                block_number += 1;
            }

            mmr_size = mmr.mmr_size();
            mmr.commit().map_err(|e| InternalErrorKind::MMR.other(e))?;
            db_txn.commit()?;

            if block_number > tip_number {
                break;
            }
        }

        pbi.finish_with_message("done!");

        Ok(chain_db.into_inner())
    }

    fn version(&self) -> &str {
        VERSION
    }
}
