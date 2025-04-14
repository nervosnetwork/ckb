//! Provide several functions used for testing.
mod chain;
mod median_time;
mod mock_chain;
mod mock_store;
mod mock_utils;

pub use chain::{
    always_success_cell, always_success_cellbase, always_success_consensus, ckb_testnet_consensus,
    is_even_lib, load_input_data_hash_cell, load_input_one_byte_cell, load_is_even,
    secp256k1_blake160_sighash_cell, secp256k1_data_cell, type_lock_script_code_hash,
};
pub use median_time::{MOCK_MEDIAN_TIME_COUNT, MockMedianTime};
pub use mock_chain::MockChain;
pub use mock_store::MockStore;
pub use mock_utils::{
    calculate_reward, create_always_success_out_point, create_always_success_tx, create_cellbase,
    create_load_input_data_hash_cell_out_point, create_load_input_data_hash_cell_tx,
    create_load_input_one_byte_cell_tx, create_load_input_one_byte_out_point,
    create_multi_outputs_transaction, create_transaction, create_transaction_with_out_point,
    dao_data,
};
