mod cellbase_maturity;
mod collision;
mod dead_cell_deps;
mod depend_tx_in_same_block;
mod descendant;
mod different_txs_with_same_input;
mod limit;
#[cfg(target_os = "linux")]
mod pool_persisted;
mod pool_reconcile;
mod pool_resurrect;
mod proposal_expire_rule;
mod reference_header_maturity;
mod reorg_proposals;
mod send_arrow_txs;
mod send_defected_binary;
mod send_large_cycles_tx;
mod send_low_fee_rate_tx;
mod send_multisig_secp_tx;
mod send_secp_tx;
mod txs_relay_order;
mod utils;
mod valid_since;

pub use cellbase_maturity::*;
pub use collision::*;
pub use dead_cell_deps::*;
pub use depend_tx_in_same_block::*;
pub use descendant::*;
pub use different_txs_with_same_input::*;
pub use limit::*;
#[cfg(target_os = "linux")]
pub use pool_persisted::*;
pub use pool_reconcile::*;
pub use pool_resurrect::*;
pub use proposal_expire_rule::*;
pub use reference_header_maturity::*;
pub use reorg_proposals::*;
pub use send_arrow_txs::*;
pub use send_defected_binary::*;
pub use send_large_cycles_tx::*;
pub use send_low_fee_rate_tx::*;
pub use send_multisig_secp_tx::*;
pub use send_secp_tx::*;
pub use txs_relay_order::*;
pub use valid_since::*;

use ckb_app_config::BlockAssemblerConfig;
use ckb_chain_spec::{build_genesis_type_id_script, OUTPUT_INDEX_SECP256K1_BLAKE160_SIGHASH_ALL};
use ckb_jsonrpc_types::JsonBytes;
use ckb_resource::CODE_HASH_SECP256K1_BLAKE160_SIGHASH_ALL;
use ckb_types::{bytes::Bytes, core::ScriptHashType, prelude::*, H256};

fn type_lock_script_code_hash() -> H256 {
    build_genesis_type_id_script(OUTPUT_INDEX_SECP256K1_BLAKE160_SIGHASH_ALL)
        .calc_script_hash()
        .unpack()
}

fn new_block_assembler_config(lock_arg: Bytes, hash_type: ScriptHashType) -> BlockAssemblerConfig {
    let code_hash = if hash_type == ScriptHashType::Data {
        CODE_HASH_SECP256K1_BLAKE160_SIGHASH_ALL.clone()
    } else {
        type_lock_script_code_hash()
    };
    BlockAssemblerConfig {
        code_hash,
        hash_type: hash_type.into(),
        args: JsonBytes::from_bytes(lock_arg),
        message: Default::default(),
    }
}
