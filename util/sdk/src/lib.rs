mod basic;
mod chain;
mod error;
mod index;
mod rpc;
mod transaction;
mod util;

pub mod wallet;

pub use basic::{Address, AddressFormat, NetworkType};
pub use chain::{
    build_witness_with_key, serialize_signature, GenesisInfo, TransferTransactionBuilder,
    MIN_SECP_CELL_CAPACITY, ONE_CKB,
};
pub use error::Error;
pub use index::{
    CellIndex, HashType, IndexDatabase, IndexError, Key as IndexKey, KeyMetrics as IndexKeyMetrics,
    KeyType as IndexKeyType, LiveCellInfo, TxInfo,
};
pub use rpc::HttpRpcClient;

pub use transaction::{
    from_local_cell_out_point, to_local_cell_out_point, CellAliasManager, CellInputManager,
    CellManager, ScriptManager, TransactionManager, VerifyResult,
};
pub use util::with_rocksdb;

// 200MB extra disk space
pub const LMDB_EXTRA_MAP_SIZE: u64 = 200 * 1024 * 1024;

const ROCKSDB_COL_CELL: &str = "cell";
const ROCKSDB_COL_CELL_ALIAS: &str = "cell-alias";
const ROCKSDB_COL_CELL_INPUT: &str = "cell-input";
const ROCKSDB_COL_SCRIPT: &str = "script";
const ROCKSDB_COL_TX: &str = "tx";
