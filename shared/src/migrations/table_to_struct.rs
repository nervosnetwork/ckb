use ckb_db::{Result, RocksDB};
use ckb_db_migration::{Migration, ProgressBar, ProgressStyle};
use ckb_store::{
    COLUMN_BLOCK_HEADER, COLUMN_EPOCH, COLUMN_META, COLUMN_TRANSACTION_INFO, COLUMN_UNCLES,
    META_CURRENT_EPOCH_KEY,
};
use std::sync::Arc;

pub struct ChangeMoleculeTableToStruct;

const VERSION: &str = "20200703124523";

impl Migration for ChangeMoleculeTableToStruct {
    fn migrate(
        &self,
        db: RocksDB,
        pb: Arc<dyn Fn(u64) -> ProgressBar + Send + Sync>,
    ) -> Result<RocksDB> {
        let pb = pb(9);
        let spinner_style = ProgressStyle::default_spinner()
            .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ ")
            .template("{prefix:.bold.dim} {spinner} {wide_msg}");
        pb.set_style(spinner_style);

        pb.set_message("migrating: block header");
        pb.inc(1);
        let txn = db.transaction();
        let header_view_migration = |key: &[u8], value: &[u8]| -> Result<()> {
            // (1 total size field + 2 fields) * 4 byte per field
            txn.put(COLUMN_BLOCK_HEADER, key, &value[12..])?;

            Ok(())
        };
        db.traverse(COLUMN_BLOCK_HEADER, header_view_migration)?;
        pb.set_message("finish: block header");
        pb.inc(1);

        pb.set_message("migrating: uncles");
        pb.inc(1);
        let uncles_migration = |key: &[u8], value: &[u8]| -> Result<()> {
            // (1 total size field + 2 fields) * 4 byte per field
            txn.put(COLUMN_UNCLES, key, &value[12..])?;
            Ok(())
        };
        db.traverse(COLUMN_UNCLES, uncles_migration)?;
        pb.set_message("finish: uncles");
        pb.inc(1);

        pb.set_message("migrating: transaction info");
        pb.inc(1);
        let transaction_info_migration = |key: &[u8], value: &[u8]| -> Result<()> {
            // (1 total size field + 3 fields) * 4 byte per field
            txn.put(COLUMN_TRANSACTION_INFO, key, &value[16..])?;
            Ok(())
        };
        db.traverse(COLUMN_TRANSACTION_INFO, transaction_info_migration)?;
        pb.set_message("finish: transaction info");
        pb.inc(1);

        pb.set_message("migrating: epoch");
        pb.inc(1);
        let epoch_ext_migration = |key: &[u8], value: &[u8]| -> Result<()> {
            // COLUMN_EPOCH stores epoch_number => last_block_hash_in_previous_epoch and last_block_hash_in_previous_epoch => epoch_ext
            // only migrates epoch_ext
            if key.len() == 32 {
                // (1 total size field + 8 fields) * 4 byte per field
                txn.put(COLUMN_EPOCH, key, &value[36..])?;
            }
            Ok(())
        };
        db.traverse(COLUMN_EPOCH, epoch_ext_migration)?;
        pb.set_message("finish: epoch");
        pb.inc(1);

        if let Some(current_epoch) = txn.get(COLUMN_META, META_CURRENT_EPOCH_KEY)? {
            txn.put(COLUMN_META, META_CURRENT_EPOCH_KEY, &current_epoch[36..])?;
        }

        txn.commit()?;

        pb.set_message("commit changes");
        pb.inc(1);
        pb.finish_with_message("waiting...");
        Ok(db)
    }

    fn version(&self) -> &str {
        VERSION
    }
}
