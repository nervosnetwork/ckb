use ckb_chain_spec::consensus::TWO_IN_TWO_OUT_CYCLES;
use ckb_jsonrpc_types::{FeeRateDef, JsonBytes, ScriptHashType};
use ckb_types::core::{Cycle, FeeRate};
use ckb_types::H256;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// default min fee rate, 1000 shannons per kilobyte
const DEFAULT_MIN_FEE_RATE: FeeRate = FeeRate::from_u64(1000);
// default max tx verify cycles
const DEFAULT_MAX_TX_VERIFY_CYCLES: Cycle = TWO_IN_TWO_OUT_CYCLES * 20;
// default max ancestors count
const DEFAULT_MAX_ANCESTORS_COUNT: usize = 25;

/// Transaction pool configuration
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TxPoolConfig {
    /// Keep the transaction pool below <max_mem_size> mb
    pub max_mem_size: usize,
    /// Keep the transaction pool below <max_cycles> cycles
    pub max_cycles: Cycle,
    /// tx verify cache capacity
    pub max_verify_cache_size: usize,
    /// Conflict cache has been deprecated, and has no effect.
    pub max_conflict_cache_size: usize,
    /// committed transactions hash cache capacity
    pub max_committed_txs_hash_cache_size: usize,
    /// txs with lower fee rate than this will not be relayed or be mined
    #[serde(with = "FeeRateDef")]
    pub min_fee_rate: FeeRate,
    /// tx pool rejects txs that cycles greater than max_tx_verify_cycles
    pub max_tx_verify_cycles: Cycle,
    /// max ancestors size limit for a single tx
    pub max_ancestors_count: usize,
    /// The file to persist the tx pool on the disk when tx pool have been shutdown.
    ///
    /// By default, it is a file inside the data directory.
    #[serde(default)]
    pub persisted_data: PathBuf,
}

impl Default for TxPoolConfig {
    fn default() -> Self {
        TxPoolConfig {
            max_mem_size: 20_000_000, // 20mb
            max_cycles: 200_000_000_000,
            max_verify_cache_size: 100_000,
            max_conflict_cache_size: 1_000,
            max_committed_txs_hash_cache_size: 100_000,
            min_fee_rate: DEFAULT_MIN_FEE_RATE,
            max_tx_verify_cycles: DEFAULT_MAX_TX_VERIFY_CYCLES,
            max_ancestors_count: DEFAULT_MAX_ANCESTORS_COUNT,
            persisted_data: Default::default(),
        }
    }
}

/// Block assembler config options.
///
/// The block assembler section tells CKB how to claim the miner rewards.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlockAssemblerConfig {
    /// The miner lock script code hash.
    pub code_hash: H256,
    /// The miner lock script hash type.
    pub hash_type: ScriptHashType,
    /// The miner lock script args.
    pub args: JsonBytes,
    /// An arbitrary message to be added into the cellbase transaction.
    pub message: JsonBytes,
}

impl TxPoolConfig {
    /// Canonicalizes paths in the config options.
    ///
    /// If `self.persisted_data` is not set, set it to `data_dir / tx_pool.dat`.
    ///
    /// If `self.path` is relative, convert them to absolute path using
    /// `root_dir` as current working directory.
    pub fn adjust<P: AsRef<Path>>(&mut self, root_dir: &Path, data_dir: P) {
        if self.persisted_data.to_str().is_none() || self.persisted_data.to_str() == Some("") {
            self.persisted_data = data_dir.as_ref().to_path_buf().join("tx_pool.dat");
        } else if self.persisted_data.is_relative() {
            self.persisted_data = root_dir.to_path_buf().join(&self.persisted_data)
        }
    }
}
