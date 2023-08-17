use ckb_app_config::StoreConfig;
use ckb_db::{Result, RocksDB};
use ckb_db_migration::{Migration, ProgressBar, ProgressStyle};
use ckb_error::InternalErrorKind;
use ckb_store::{ChainDB, ChainStore};
use ckb_types::{
    core::BlockNumber,
    utilities::merkle_mountain_range::{hash_out_point_and_status, CellStatus},
};
use std::sync::Arc;

pub struct AddCellsRootMMR;

const VERSION: &str = "20230801101332";

impl Migration for AddCellsRootMMR {
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

        loop {
            let db_txn = chain_db.begin_transaction();
            let mut cells_root_mmr = db_txn.cells_root_mmr(block_number);

            for _ in 0..10000 {
                let block_hash = db_txn.get_block_hash(block_number).unwrap();
                let transactions = db_txn.get_block_body(&block_hash);
                for tx in transactions.iter() {
                    for input in tx.inputs().into_iter() {
                        let out_point = input.previous_output();
                        // cellbase and genesis block's tx may not have previous output
                        if let Some(mut cell_status) = db_txn.get_cells_root_mmr_status(&out_point)
                        {
                            cells_root_mmr
                                .update(
                                    cell_status.mmr_position,
                                    hash_out_point_and_status(
                                        &out_point,
                                        cell_status.created_by,
                                        block_number,
                                    ),
                                )
                                .map_err(|e| InternalErrorKind::MMR.other(e))?;
                            cell_status.mark_as_consumed(block_number);
                            db_txn.insert_cells_root_mmr_status(&out_point, &cell_status)?;
                        }
                    }

                    for out_point in tx.output_pts().into_iter() {
                        let hash =
                            hash_out_point_and_status(&out_point, block_number, BlockNumber::MAX);
                        let mmr_position = cells_root_mmr
                            .push(hash)
                            .map_err(|e| InternalErrorKind::MMR.other(e))?;
                        let cell_status = CellStatus::new(mmr_position, block_number);
                        db_txn.insert_cells_root_mmr_status(&out_point, &cell_status)?;
                    }
                }
                cells_root_mmr
                    .commit()
                    .map_err(|e| InternalErrorKind::MMR.other(e))?;
                db_txn.insert_cells_root_mmr_size(block_number, cells_root_mmr.mmr_size())?;

                pbi.inc(1);
                block_number += 1;
            }
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
