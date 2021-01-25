use ckb_app_config::StoreConfig;
use ckb_db::{Direction, IteratorMode, Result, RocksDB};
use ckb_db_migration::{Migration, ProgressBar, ProgressStyle};
use ckb_db_schema::{COLUMN_BLOCK_BODY, COLUMN_INDEX, COLUMN_NUMBER_HASH};
use ckb_migration_template::multi_thread_migration;
use ckb_store::{ChainDB, ChainStore};
use ckb_types::{molecule::io::Write, packed, prelude::*};
use std::sync::Arc;

pub struct AddNumberHashMapping;

const VERSION: &str = "20200710181855";

impl Migration for AddNumberHashMapping {
    fn migrate(
        &self,
        db: RocksDB,
        pb: Arc<dyn Fn(u64) -> ProgressBar + Send + Sync>,
    ) -> Result<RocksDB> {
        multi_thread_migration! {
            {
                for number in i * chunk_size..end {
                    let block_number: packed::Uint64 = number.pack();
                    let raw_hash = chain_db.get(COLUMN_INDEX, block_number.as_slice()).expect("DB data integrity");
                    let txs_len = chain_db.get_iter(
                        COLUMN_BLOCK_BODY,
                        IteratorMode::From(&raw_hash, Direction::Forward),
                    )
                    .take_while(|(key, _)| key.starts_with(&raw_hash))
                    .count();

                    let raw_txs_len: packed::Uint32 = (txs_len as u32).pack();

                    let mut raw_key = Vec::with_capacity(40);
                    raw_key.write_all(block_number.as_slice()).expect("write_all block_number");
                    raw_key.write_all(&raw_hash).expect("write_all hash");
                    let key = packed::NumberHash::new_unchecked(raw_key.into());

                    wb.put(
                        COLUMN_NUMBER_HASH,
                        key.as_slice(),
                        raw_txs_len.as_slice(),
                    )
                    .expect("put number_hash");

                    if wb.len() > BATCH {
                        chain_db.write(&wb).expect("write db batch");
                        wb.clear().unwrap();
                    }
                    pbi.inc(1);
                }
            }
        }
    }

    fn version(&self) -> &str {
        VERSION
    }
}
