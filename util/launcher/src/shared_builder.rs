//! Shared factory
//!
//! which can be used in order to configure the properties of a new shared.

use ckb_app_config::ExitCode;
use ckb_app_config::{BlockAssemblerConfig, DBConfig, NotifyConfig, StoreConfig, TxPoolConfig};
use ckb_async_runtime::{new_background_runtime, Handle};
use ckb_chain_spec::consensus::Consensus;
use ckb_chain_spec::SpecError;
use ckb_channel::Receiver;
use ckb_db::RocksDB;
use ckb_db_schema::COLUMNS;
use ckb_error::{Error, InternalErrorKind};
use ckb_freezer::Freezer;
use ckb_logger::{error, info};
use ckb_migrate::migrate::Migrate;
use ckb_notify::{NotifyController, NotifyService, PoolTransactionEntry};
use ckb_proposal_table::ProposalTable;
use ckb_proposal_table::ProposalView;
use ckb_shared::Shared;
use ckb_snapshot::{Snapshot, SnapshotMgr};

use ckb_store::ChainDB;
use ckb_store::ChainStore;
use ckb_tx_pool::{
    error::Reject, service::TxVerificationResult, TokioRwLock, TxEntry, TxPool,
    TxPoolServiceBuilder,
};
use ckb_types::core::EpochExt;
use ckb_types::core::HeaderView;
use ckb_verification::cache::init_cache;
use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tempfile::TempDir;

pub fn open_or_create_db(
    bin_name: &str,
    root_dir: &Path,
    config: &DBConfig,
) -> Result<RocksDB, ExitCode> {
    let migrate = Migrate::new(&config.path);

    let read_only_db = migrate.open_read_only_db().map_err(|e| {
        eprintln!("migrate error {e}");
        ExitCode::Failure
    })?;

    if let Some(db) = read_only_db {
        match migrate.check(&db) {
            Ordering::Greater => {
                eprintln!(
                    "The database is created by a higher version CKB executable binary, \n\
                     so that the current CKB executable binary couldn't open this database.\n\
                     Please download the latest CKB executable binary."
                );
                Err(ExitCode::Failure)
            }
            Ordering::Equal => Ok(RocksDB::open(config, COLUMNS)),
            Ordering::Less => {
                if migrate.require_expensive(&db) {
                    eprintln!(
                        "For optimal performance, CKB wants to migrate the data into new format.\n\
                        You can use the old version CKB if you don't want to do the migration.\n\
                        We strongly recommended you to use the latest stable version of CKB, \
                        since the old versions may have unfixed vulnerabilities.\n\
                        Run `\"{}\" migrate -C \"{}\"` and confirm by typing \"YES\" to migrate the data.\n\
                        We strongly recommend that you backup the data directory before migration.",
                        bin_name,
                        root_dir.display()
                    );
                    Err(ExitCode::Failure)
                } else {
                    info!("process fast migrations ...");

                    let bulk_load_db_db = migrate.open_bulk_load_db().map_err(|e| {
                        eprintln!("migrate error {e}");
                        ExitCode::Failure
                    })?;

                    if let Some(db) = bulk_load_db_db {
                        migrate.migrate(db).map_err(|err| {
                            eprintln!("Run error: {err:?}");
                            ExitCode::Failure
                        })?;
                    }

                    Ok(RocksDB::open(config, COLUMNS))
                }
            }
        }
    } else {
        let db = RocksDB::open(config, COLUMNS);
        migrate.init_db_version(&db).map_err(|e| {
            eprintln!("migrate init_db_version error {e}");
            ExitCode::Failure
        })?;
        Ok(db)
    }
}

fn start_notify_service(notify_config: NotifyConfig, handle: Handle) -> NotifyController {
    NotifyService::new(notify_config, handle).start()
}

fn build_store(
    db: RocksDB,
    store_config: StoreConfig,
    ancient_path: Option<PathBuf>,
) -> Result<ChainDB, Error> {
    let store = if store_config.freezer_enable && ancient_path.is_some() {
        let freezer = Freezer::open(ancient_path.expect("exist checked"))?;
        ChainDB::new_with_freezer(db, freezer, store_config)
    } else {
        ChainDB::new(db, store_config)
    };
    Ok(store)
}

fn register_tx_pool_callback(tx_pool_builder: &mut TxPoolServiceBuilder, notify: NotifyController) {
    let notify_pending = notify.clone();

    let tx_relay_sender = tx_pool_builder.tx_relay_sender();
    let create_notify_entry = |entry: &TxEntry| PoolTransactionEntry {
        transaction: entry.rtx.transaction.clone(),
        cycles: entry.cycles,
        size: entry.size,
        fee: entry.fee,
        timestamp: entry.timestamp,
    };
    tx_pool_builder.register_pending(Box::new(move |tx_pool: &mut TxPool, entry: &TxEntry| {
        // update statics
        tx_pool.update_statics_for_add_tx(entry.size, entry.cycles);

        // notify
        let notify_tx_entry = create_notify_entry(entry);
        notify_pending.notify_new_transaction(notify_tx_entry);
    }));

    let notify_proposed = notify.clone();
    tx_pool_builder.register_proposed(Box::new(
        move |tx_pool: &mut TxPool, entry: &TxEntry, new: bool| {
            // update statics
            if new {
                tx_pool.update_statics_for_add_tx(entry.size, entry.cycles);
            }

            // notify
            let notify_tx_entry = create_notify_entry(entry);
            notify_proposed.notify_proposed_transaction(notify_tx_entry);
        },
    ));

    tx_pool_builder.register_committed(Box::new(move |tx_pool: &mut TxPool, entry: &TxEntry| {
        tx_pool.update_statics_for_remove_tx(entry.size, entry.cycles);
    }));

    let notify_reject = notify;
    tx_pool_builder.register_reject(Box::new(
        move |tx_pool: &mut TxPool, entry: &TxEntry, reject: Reject| {
            // update statics
            tx_pool.update_statics_for_remove_tx(entry.size, entry.cycles);

            let tx_hash = entry.transaction().hash();
            // record recent reject
            if matches!(reject, Reject::Resolve(..) | Reject::RBFRejected(..)) {
                if let Some(ref mut recent_reject) = tx_pool.recent_reject {
                    if let Err(e) = recent_reject.put(&tx_hash, reject.clone()) {
                        error!("record recent_reject failed {} {} {}", tx_hash, reject, e);
                    }
                }
            }

            if reject.is_allowed_relay() {
                if let Err(e) = tx_relay_sender.send(TxVerificationResult::Reject { tx_hash }) {
                    error!("tx-pool tx_relay_sender internal error {}", e);
                }
            }

            // notify
            let notify_tx_entry = create_notify_entry(entry);
            notify_reject.notify_reject_transaction(notify_tx_entry, reject);
        },
    ));
}
