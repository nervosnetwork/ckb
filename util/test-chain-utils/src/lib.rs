//! Provide several functions used for testing.
mod chain;
mod median_time;
mod mock_store;

pub use chain::{
    always_success_cell, always_success_cellbase, always_success_consensus, ckb_testnet_consensus,
    load_input_data_hash_cell, load_input_one_byte_cell, secp256k1_blake160_sighash_cell,
    secp256k1_data_cell, type_lock_script_code_hash,
};
pub use median_time::{MockMedianTime, MOCK_MEDIAN_TIME_COUNT};
pub use mock_store::MockStore;
