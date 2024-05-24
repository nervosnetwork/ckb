use ckb_app_config::StoreConfig;
use ckb_db_migration::{Migration, ProgressBar, ProgressStyle};
use ckb_db_schema::COLUMN_EPOCH;
use ckb_error::InternalErrorKind;
use ckb_store::{ChainDB, ChainStore};
use ckb_types::{
    core::hardfork::HardForks,
    packed,
    prelude::{Entity, FromSliceShouldBeOk, Pack, Reader},
};

const VERSION: &str = "20231101000000";
pub struct BlockExt2019ToZero {
    hardforks: HardForks,
}

impl BlockExt2019ToZero {
    pub fn new(hardforks: HardForks) -> Self {
        BlockExt2019ToZero { hardforks }
    }
}

impl Migration for BlockExt2019ToZero {
    fn run_in_background(&self) -> bool {
        true
    }

    fn migrate(
        &self,
        db: ckb_db::RocksDB,
        pb: std::sync::Arc<dyn Fn(u64) -> ProgressBar + Send + Sync>,
    ) -> Result<ckb_db::RocksDB, ckb_error::Error> {
        let chain_db = ChainDB::new(db, StoreConfig::default());
        let limit_epoch = self.hardforks.ckb2021.rfc_0032();

        eprintln!(
            "begin to run block_ext 2019 to zero migrate...: {}",
            limit_epoch
        );

        if limit_epoch == 0 {
            return Ok(chain_db.into_inner());
        }

        let hard_fork_epoch_number: packed::Uint64 = limit_epoch.pack();
        let tip_header = chain_db.get_tip_header().expect("db must have tip header");
        let tip_epoch_number = tip_header.epoch().pack();

        let header = if tip_epoch_number < hard_fork_epoch_number {
            Some(tip_header)
        } else if let Some(epoch_hash) =
            chain_db.get(COLUMN_EPOCH::NAME, hard_fork_epoch_number.as_slice())
        {
            let epoch_ext = chain_db
                .get_epoch_ext(
                    &packed::Byte32Reader::from_slice_should_be_ok(epoch_hash.as_ref()).to_entity(),
                )
                .expect("db must have epoch ext");
            let header = chain_db
                .get_block_header(&epoch_ext.last_block_hash_in_previous_epoch())
                .expect("db must have header");
            Some(header)
        } else {
            None
        };

        if let Some(mut header) = header {
            let pb = ::std::sync::Arc::clone(&pb);
            let pbi = pb(header.number() + 1);
            pbi.set_style(
                        ProgressStyle::default_bar()
                            .template(
                                "{prefix:.bold.dim} {spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}",
                            )
                            .progress_chars("#>-"),
                    );
            pbi.set_position(0);
            pbi.enable_steady_tick(5000);

            loop {
                let db_txn = chain_db.begin_transaction();
                if self.stop_background() {
                    return Err(InternalErrorKind::Database.other("intrupted").into());
                }
                for _ in 0..10000 {
                    let hash = header.hash();
                    let num_hash = header.num_hash();
                    let mut old_block_ext = db_txn.get_block_ext(&hash).unwrap();
                    old_block_ext.cycles = None;
                    db_txn.insert_block_ext(num_hash, &old_block_ext)?;

                    if header.is_genesis() {
                        break;
                    }

                    header = db_txn
                        .get_block_header(&header.parent_hash())
                        .expect("db must have header");

                    pbi.inc(1);
                }
                db_txn.commit()?;

                if header.is_genesis() {
                    break;
                }
            }
        }

        Ok(chain_db.into_inner())
    }
    fn version(&self) -> &str {
        VERSION
    }
}
