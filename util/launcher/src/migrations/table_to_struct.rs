use ckb_db::{Direction, IteratorMode, Result, RocksDB};
use ckb_db_migration::{Migration, ProgressBar, ProgressStyle};
use ckb_db_schema::{
    COLUMN_BLOCK_HEADER, COLUMN_EPOCH, COLUMN_META, COLUMN_TRANSACTION_INFO, COLUMN_UNCLES,
    META_CURRENT_EPOCH_KEY,
};
use std::sync::Arc;

pub struct ChangeMoleculeTableToStruct;

const LIMIT: usize = 100_000;
const VERSION: &str = "20200703124523";

impl ChangeMoleculeTableToStruct {
    fn mode<'a>(&self, key: &'a [u8]) -> IteratorMode<'a> {
        if key == [0] {
            IteratorMode::Start
        } else {
            IteratorMode::From(key, Direction::Forward)
        }
    }

    fn migrate_header(&self, db: &RocksDB) -> Result<()> {
        const HEADER_SIZE: usize = 240;
        let mut next_key = vec![0];
        while !next_key.is_empty() {
            let mut wb = db.new_write_batch();
            let mut header_view_migration = |key: &[u8], value: &[u8]| -> Result<()> {
                // (1 total size field + 2 fields) * 4 byte per field
                if value.len() != HEADER_SIZE {
                    wb.put(COLUMN_BLOCK_HEADER, key, &value[12..])?;
                }

                Ok(())
            };

            let mode = self.mode(&next_key);

            let (_count, nk) =
                db.traverse(COLUMN_BLOCK_HEADER, &mut header_view_migration, mode, LIMIT)?;
            next_key = nk;

            if !wb.is_empty() {
                db.write(&wb)?;
                wb.clear()?;
            }
        }

        Ok(())
    }

    fn migrate_uncles(&self, db: &RocksDB) -> Result<()> {
        const HEADER_SIZE: usize = 240;
        let mut next_key = vec![0];
        while !next_key.is_empty() {
            let mut wb = db.new_write_batch();
            let mut uncles_migration = |key: &[u8], value: &[u8]| -> Result<()> {
                // (1 total size field + 2 fields) * 4 byte per field
                if value.len() != HEADER_SIZE {
                    wb.put(COLUMN_UNCLES, key, &value[12..])?;
                }
                Ok(())
            };

            let mode = self.mode(&next_key);
            let (_count, nk) = db.traverse(COLUMN_UNCLES, &mut uncles_migration, mode, LIMIT)?;
            next_key = nk;

            if !wb.is_empty() {
                db.write(&wb)?;
                wb.clear()?;
            }
        }
        Ok(())
    }

    fn migrate_transaction_info(&self, db: &RocksDB) -> Result<()> {
        const TRANSACTION_INFO_SIZE: usize = 52;
        let mut next_key = vec![0];
        while !next_key.is_empty() {
            let mut wb = db.new_write_batch();
            let mut transaction_info_migration = |key: &[u8], value: &[u8]| -> Result<()> {
                // (1 total size field + 3 fields) * 4 byte per field
                if value.len() != TRANSACTION_INFO_SIZE {
                    wb.put(COLUMN_TRANSACTION_INFO, key, &value[16..])?;
                }
                Ok(())
            };

            let mode = self.mode(&next_key);

            let (_count, nk) =
                db.traverse(COLUMN_UNCLES, &mut transaction_info_migration, mode, LIMIT)?;
            next_key = nk;

            if !wb.is_empty() {
                db.write(&wb)?;
                wb.clear()?;
            }
        }
        Ok(())
    }

    fn migrate_epoch_ext(&self, db: &RocksDB) -> Result<()> {
        const EPOCH_SIZE: usize = 108;
        let mut next_key = vec![0];
        while !next_key.is_empty() {
            let mut wb = db.new_write_batch();
            let mut epoch_ext_migration = |key: &[u8], value: &[u8]| -> Result<()> {
                // COLUMN_EPOCH stores epoch_number => last_block_hash_in_previous_epoch and last_block_hash_in_previous_epoch => epoch_ext
                // only migrates epoch_ext
                if key.len() == 32 && value.len() != EPOCH_SIZE {
                    // (1 total size field + 8 fields) * 4 byte per field
                    wb.put(COLUMN_EPOCH, key, &value[36..])?;
                }
                Ok(())
            };

            let mode = self.mode(&next_key);
            let (_count, nk) = db.traverse(COLUMN_EPOCH, &mut epoch_ext_migration, mode, LIMIT)?;
            next_key = nk;

            if !wb.is_empty() {
                db.write(&wb)?;
                wb.clear()?;
            }
        }
        Ok(())
    }
}

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
        self.migrate_header(&db)?;
        pb.set_message("finish: block header");
        pb.inc(1);

        pb.set_message("migrating: uncles");
        pb.inc(1);
        self.migrate_uncles(&db)?;
        pb.set_message("finish: uncles");
        pb.inc(1);

        pb.set_message("migrating: transaction info");
        pb.inc(1);
        self.migrate_transaction_info(&db)?;
        pb.set_message("finish: transaction info");
        pb.inc(1);

        pb.set_message("migrating: epoch");
        pb.inc(1);
        self.migrate_epoch_ext(&db)?;
        pb.set_message("finish: epoch");
        pb.inc(1);

        let mut wb = db.new_write_batch();
        if let Some(current_epoch) = db.get_pinned(COLUMN_META, META_CURRENT_EPOCH_KEY)? {
            if current_epoch.len() != 108 {
                wb.put(COLUMN_META, META_CURRENT_EPOCH_KEY, &current_epoch[36..])?;
            }
        }
        db.write(&wb)?;

        pb.set_message("commit changes");
        pb.inc(1);
        pb.finish_with_message("waiting...");
        Ok(db)
    }

    fn version(&self) -> &str {
        VERSION
    }
}
