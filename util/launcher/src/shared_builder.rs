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
