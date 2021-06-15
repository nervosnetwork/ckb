use ckb_chain_spec::consensus::TWO_IN_TWO_OUT_CYCLES;
use ckb_jsonrpc_types::{FeeRateDef, JsonBytes, ScriptHashType, ScriptHashTypeShadow, VmVersion};
use ckb_types::core::{Cycle, FeeRate};
use ckb_types::H256;
use serde::{Deserialize, Serialize};

// default min fee rate, 1000 shannons per kilobyte
const DEFAULT_MIN_FEE_RATE: FeeRate = FeeRate::from_u64(1000);
// default max tx verify cycles
const DEFAULT_MAX_TX_VERIFY_CYCLES: Cycle = TWO_IN_TWO_OUT_CYCLES * 20;
// default max ancestors count
const DEFAULT_MAX_ANCESTORS_COUNT: usize = 25;

/// Transaction pool configuration
#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
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
        }
    }
}

/// Block assembler config options.
///
/// The block assembler section tells CKB how to claim the miner rewards.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "BlockAssemblerConfigShadow")]
pub struct BlockAssemblerConfig {
    /// The miner lock script code hash.
    pub code_hash: H256,
    /// The miner lock script hash type.
    #[serde(flatten)]
    pub hash_type: ScriptHashType,
    /// The miner lock script args.
    pub args: JsonBytes,
    /// An arbitrary message to be added into the cellbase transaction.
    pub message: JsonBytes,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BlockAssemblerConfigShadow {
    code_hash: H256,
    #[serde(rename = "hash_type")]
    hash_type_shadow: ScriptHashTypeShadow,
    #[serde(rename = "vm_version")]
    vm_version_opt: Option<VmVersion>,
    args: JsonBytes,
    message: JsonBytes,
}

struct BlockAssemblerConfigValidationError(&'static str);

impl std::fmt::Display for BlockAssemblerConfigValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl std::convert::TryFrom<BlockAssemblerConfigShadow> for BlockAssemblerConfig {
    type Error = BlockAssemblerConfigValidationError;
    fn try_from(shadow: BlockAssemblerConfigShadow) -> Result<Self, Self::Error> {
        let BlockAssemblerConfigShadow {
            code_hash,
            hash_type_shadow,
            vm_version_opt,
            args,
            message,
        } = shadow;
        let hash_type = match hash_type_shadow {
            ScriptHashTypeShadow::Data => {
                let vm_version = vm_version_opt.unwrap_or_default();
                ScriptHashType::Data { vm_version }
            }
            ScriptHashTypeShadow::Type => {
                if vm_version_opt.is_some() {
                    return Err(BlockAssemblerConfigValidationError(
                        "vm version is not allowed for hash-type \"type\".",
                    ));
                }
                ScriptHashType::Type
            }
        };
        let script = BlockAssemblerConfig {
            code_hash,
            hash_type,
            args,
            message,
        };
        Ok(script)
    }
}
