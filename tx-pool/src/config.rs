use crate::fee_rate::FeeRate;
use ckb_jsonrpc_types::{JsonBytes, ScriptHashType};
use ckb_types::core::Cycle;
use ckb_types::H256;
use serde_derive::{Deserialize, Serialize};

// default min fee rate, 1000 shannons per kilobyte
const DEFAULT_MIN_FEE_RATE: FeeRate = FeeRate::from_u64(1000);

/// Transaction pool configuration
#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub struct TxPoolConfig {
    // Keep the transaction pool below <max_mem_size> mb
    pub max_mem_size: usize,
    // Keep the transaction pool below <max_cycles> cycles
    pub max_cycles: Cycle,
    // tx verify cache capacity
    pub max_verify_cache_size: usize,
    // conflict tx cache capacity
    pub max_conflict_cache_size: usize,
    // committed transactions hash cache capacity
    pub max_committed_txs_hash_cache_size: usize,
    // txs with lower fee rate than this will not be relayed or be mined
    pub min_fee_rate: FeeRate,
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
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlockAssemblerConfig {
    pub code_hash: H256,
    pub hash_type: ScriptHashType,
    pub args: JsonBytes,
    pub message: JsonBytes,
}
