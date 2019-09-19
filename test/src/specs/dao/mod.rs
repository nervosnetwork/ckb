mod dao_tx;
mod satoshi_dao_occupied;

pub use dao_tx::{
    DepositDAO, WithdrawAndDepositDAOWithinSameTx, WithdrawDAO, WithdrawDAOWithInvalidWitness,
    WithdrawDAOWithNotMaturitySince, WithdrawDAOWithOverflowCapacity,
};

pub use satoshi_dao_occupied::{DAOWithSatoshiCellOccupied, SpendSatoshiCell};

use crate::utils::is_committed;
use crate::Node;
use ckb_resource::CODE_HASH_DAO;
use ckb_test_chain_utils::always_success_cell;
use ckb_types::{
    bytes::Bytes,
    core::{BlockNumber, Capacity, ScriptHashType, TransactionBuilder, TransactionView},
    packed::{Byte32, CellDep, CellInput, CellOutput, OutPoint, Script},
    prelude::*,
};

const SYSTEM_CELL_ALWAYS_SUCCESS_INDEX: u32 = 5;
const SYSTEM_CELL_DAO_INDEX: u32 = 2;
const WITHDRAW_WINDOW_LEFT: u64 = 10;
// The second witness
const WITHDRAW_HEADER_INDEX: u64 = 1;

// Send the given transaction and ensure it being committed
fn ensure_committed(node: &Node, transaction: &TransactionView) -> (OutPoint, Byte32) {
    // Ensure the transaction's cellbase-maturity and since-maturity
    node.generate_blocks(20);

    let tx_hash = node
        .rpc_client()
        .send_transaction(transaction.data().into());

    // Ensure the sent transaction is beyond the proposal-window
    node.generate_blocks(20);

    let tx_status = node
        .rpc_client()
        .get_transaction(tx_hash.clone())
        .expect("get sent transaction");
    assert!(
        is_committed(&tx_status),
        "ensure_committed failed {}",
        tx_hash
    );

    let block_hash = tx_status.tx_status.block_hash.unwrap();
    (OutPoint::new(tx_hash, 0), block_hash.pack())
}

fn tip_cellbase_input(node: &Node) -> (CellInput, Byte32, Capacity) {
    let tip_block = node.get_tip_block();
    let cellbase = tip_block.transactions()[0].clone();
    let block_hash = tip_block.hash();
    let tx_hash = cellbase.hash();
    let previous_out_point = OutPoint::new(tx_hash, 0);
    let capacity = cellbase.outputs_capacity().unwrap();
    (CellInput::new(previous_out_point, 0), block_hash, capacity)
}

// deps = [always-success-cell, dao-cell]
fn deposit_dao_deps(node: &Node) -> (Vec<CellDep>, Vec<Byte32>) {
    let genesis_block = node.get_block_by_number(0);
    let genesis_tx = &genesis_block.transactions()[0];

    // Reference to AlwaysSuccess lock_script, to unlock the cellbase
    let always_dep = CellDep::new_builder()
        .out_point(OutPoint::new(
            genesis_tx.hash(),
            SYSTEM_CELL_ALWAYS_SUCCESS_INDEX,
        ))
        .build();
    // Reference to DAO type_script
    let dao_dep = CellDep::new_builder()
        .out_point(OutPoint::new(genesis_tx.hash(), SYSTEM_CELL_DAO_INDEX))
        .build();

    (vec![always_dep, dao_dep], vec![genesis_block.hash()])
}

// cell deps = [always-success-cell, dao-cell]
// header deps = [genesis-header-hash, withdraw-header-hash]
fn withdraw_dao_deps(node: &Node, withdraw_header_hash: Byte32) -> (Vec<CellDep>, Vec<Byte32>) {
    let (cell_deps, mut header_deps) = deposit_dao_deps(node);
    header_deps.push(withdraw_header_hash);
    (cell_deps, header_deps)
}

fn deposit_dao_script() -> Script {
    Script::new_builder()
        .code_hash(CODE_HASH_DAO.pack())
        .hash_type(ScriptHashType::Data.pack())
        .build()
}

// Deposit `capacity` into DAO. The target output's type script == dao-script
fn deposit_dao_output(capacity: Capacity) -> (CellOutput, Bytes) {
    let always_success_script = always_success_cell().2.clone();
    let data = Bytes::from(vec![1; 10]);
    let cell_output = CellOutput::new_builder()
        .capacity(capacity.pack())
        .lock(always_success_script)
        .type_(Some(deposit_dao_script()).pack())
        .build();
    (cell_output, data)
}

// Withdraw `capacity` from DAO. the target output's type script is NONE
fn withdraw_dao_output(capacity: Capacity) -> (CellOutput, Bytes) {
    let always_success_script = always_success_cell().2.clone();
    let data = Bytes::from(vec![1; 10]);
    let cell_output = CellOutput::new_builder()
        .capacity(capacity.pack())
        .lock(always_success_script)
        .build();
    (cell_output, data)
}

fn absolute_minimal_since(node: &Node) -> BlockNumber {
    node.get_tip_block_number() + WITHDRAW_WINDOW_LEFT
}

// Construct a deposit dao transaction, which consumes the tip-cellbase as the input,
// generates the output with always-success-script as lock script, dao-script as type script
fn deposit_dao_transaction(node: &Node) -> TransactionView {
    let (input, block_hash, input_capacity) = tip_cellbase_input(node);
    let (output, output_data) = deposit_dao_output(input_capacity);
    let (cell_deps, mut header_deps) = deposit_dao_deps(node);
    header_deps.push(block_hash);
    TransactionBuilder::default()
        .cell_deps(cell_deps)
        .header_deps(header_deps.into_iter())
        .input(input)
        .output(output)
        .output_data(output_data.pack())
        .build()
}

// Construct a withdraw dao transaction, which consumes the tip-cellbase and a given deposited cell
// as the inputs, generates the output with always-success-script as lock script, none type script
fn withdraw_dao_transaction(
    node: &Node,
    out_point: OutPoint,
    block_hash: Byte32,
) -> TransactionView {
    let withdraw_header_hash = node.get_tip_block().hash();
    let deposited_input = {
        let minimal_since = absolute_minimal_since(node);
        CellInput::new(out_point.clone(), minimal_since)
    };
    let (output, output_data) = {
        let input_capacities = node
            .rpc_client()
            .calculate_dao_maximum_withdraw(out_point.into(), withdraw_header_hash.clone());
        withdraw_dao_output(input_capacities)
    };
    let (cell_deps, mut header_deps) = withdraw_dao_deps(node, withdraw_header_hash);
    header_deps.push(block_hash);
    // Put the withdraw_header_index into the 2nd witness
    let withdraw_dao_witness = vec![
        Bytes::new().pack(),
        Bytes::from(WITHDRAW_HEADER_INDEX.to_le_bytes().to_vec()).pack(),
    ]
    .pack();
    TransactionBuilder::default()
        .cell_deps(cell_deps)
        .header_deps(header_deps.into_iter())
        .input(deposited_input)
        .output(output)
        .output_data(output_data.pack())
        .witness(withdraw_dao_witness)
        .build()
}
