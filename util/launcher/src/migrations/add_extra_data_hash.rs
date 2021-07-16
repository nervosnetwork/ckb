use ckb_db::{Direction, IteratorMode, Result, RocksDB};
use ckb_db_migration::{Migration, ProgressBar, ProgressStyle};
use ckb_db_schema::{COLUMN_CELL_DATA, COLUMN_CELL_DATA_HASH};
use ckb_types::{packed, prelude::*};
use std::sync::Arc;

pub struct AddExtraDataHash;

const VERSION: &str = "20210609195049";

const LIMIT: usize = 100_000;

impl AddExtraDataHash {
    fn mode<'a>(&self, key: &'a [u8]) -> IteratorMode<'a> {
        if key == [0] {
            IteratorMode::Start
        } else {
            IteratorMode::From(key, Direction::Forward)
        }
    }
}

impl Migration for AddExtraDataHash {
    fn migrate(
        &self,
        db: RocksDB,
        pb: Arc<dyn Fn(u64) -> ProgressBar + Send + Sync>,
    ) -> Result<RocksDB> {
        let pb = pb(1);
        let spinner_style = ProgressStyle::default_spinner()
            .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ ")
            .template("{prefix:.bold.dim} {spinner} {wide_msg}");
        pb.set_style(spinner_style);
        let mut next_key = vec![0];
        while !next_key.is_empty() {
            let mut wb = db.new_write_batch();
            let mut cell_data_migration = |key: &[u8], value: &[u8]| -> Result<()> {
                let data_hash = if !value.as_ref().is_empty() {
                    let reader = packed::CellDataEntryReader::from_slice_should_be_ok(value);
                    reader.output_data_hash().as_slice()
                } else {
                    &[]
                };
                wb.put(COLUMN_CELL_DATA_HASH, key, data_hash)?;
                Ok(())
            };

            let mode = self.mode(&next_key);

            let (_count, nk) =
                db.traverse(COLUMN_CELL_DATA, &mut cell_data_migration, mode, LIMIT)?;
            next_key = nk;

            if !wb.is_empty() {
                db.write(&wb)?;
                wb.clear()?;
            }
        }
        pb.inc(1);
        pb.finish_with_message("waiting...");
        Ok(db)
    }

    fn version(&self) -> &str {
        VERSION
    }
}
