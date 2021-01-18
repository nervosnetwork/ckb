use ckb_app_config::StoreConfig;
use ckb_db::{Result, RocksDB};
use ckb_db_migration::{Migration, ProgressBar, ProgressStyle};
use ckb_db_schema::COLUMN_NUMBER_HASH;
use ckb_migration_template::multi_thread_migration;
use ckb_store::{ChainDB, ChainStore};
use ckb_types::{packed, prelude::*};
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
                    let block = chain_db
                        .get_block_hash(number)
                        .and_then(|hash| chain_db.get_block(&hash))
                        .expect("DB data integrity");

                    let txs_len: packed::Uint32 = (block.transactions().len() as u32).pack();
                    wb.put(
                        COLUMN_NUMBER_HASH,
                        packed::NumberHash::new_builder()
                            .number(block.number().pack())
                            .block_hash(block.header().hash())
                            .build()
                            .as_slice(),
                        txs_len.as_slice(),
                    )
                    .unwrap();

                    if wb.len() > BATCH {
                        chain_db.write(&wb).unwrap();
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
