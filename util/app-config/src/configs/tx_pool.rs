use ckb_jsonrpc_types::{FeeRateDef, JsonBytes, ScriptHashType};
use ckb_types::core::{Cycle, FeeRate};
use ckb_types::H256;
use serde::{Deserialize, Serialize};

// The default values are set in the legacy version.
/// Transaction pool configuration
#[derive(Copy, Clone, Debug, Serialize)]
pub struct TxPoolConfig {
    /// Keep the transaction pool below <max_mem_size> mb
    pub max_mem_size: usize,
    /// Keep the transaction pool below <max_cycles> cycles
    pub max_cycles: Cycle,
    /// txs with lower fee rate than this will not be relayed or be mined
    #[serde(with = "FeeRateDef")]
    pub min_fee_rate: FeeRate,
    /// tx pool rejects txs that cycles greater than max_tx_verify_cycles
    pub max_tx_verify_cycles: Cycle,
    /// max ancestors size limit for a single tx
    pub max_ancestors_count: usize,
}

/// Block assembler config options.
///
/// The block assembler section tells CKB how to claim the miner rewards.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockAssemblerConfig {
    /// The miner lock script code hash.
    pub code_hash: H256,
    /// The miner lock script args.
    pub args: JsonBytes,
    /// An arbitrary message to be added into the cellbase transaction.
    pub message: JsonBytes,
    /// The miner lock script hash type.
    pub hash_type: ScriptHashType,
}
