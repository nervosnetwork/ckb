use crate::utils::{assert_send_transaction_fail, is_committed};
use crate::{Net, Node, Spec};
use ckb_resource::CODE_HASH_DAO;
use ckb_test_chain_utils::always_success_cell;
use ckb_types::{
    bytes::Bytes,
    core::{BlockNumber, Capacity, ScriptHashType, TransactionBuilder, TransactionView},
    packed::{self, Byte32, CellDep, CellInput, CellOutput, OutPoint, Script},
    prelude::*,
};

const SYSTEM_CELL_ALWAYS_SUCCESS_INDEX: u32 = 1;
const SYSTEM_CELL_DAO_INDEX: u32 = 3;
const WITHDRAW_WINDOW_LEFT: u64 = 10;
// The second witness
const WITHDRAW_HEADER_INDEX: u64 = 1;

pub struct DepositDAO;

impl Spec for DepositDAO {
    crate::name!("deposit_dao");

    fn run(&self, net: Net) {
        let node0 = &net.nodes[0];
        node0.generate_blocks(2);

        // Deposit DAO
        {
            let transaction = deposit_dao_transaction(node0);
            ensure_committed(node0, &transaction);
        }

        // Deposit DAO without specifying `block_hash` within input
        {
            // deposit dao transaction without `block_hash`
            let transaction = {
                let out_point_without_block_hash = {
                    let tx_hash = node0.get_tip_block().transactions()[0].hash();
                    OutPoint::new(tx_hash, 0)
                };
                deposit_dao_transaction(node0)
                    .as_advanced_builder()
                    .set_inputs(vec![CellInput::new(out_point_without_block_hash, 0)])
                    .build()
            };
            ensure_committed(node0, &transaction);
        }
    }
}

pub struct WithdrawDAO;

impl Spec for WithdrawDAO {
    crate::name!("withdraw_dao");

    fn run(&self, net: Net) {
        let node0 = &net.nodes[0];
        node0.generate_blocks(2);

        let deposited = {
            let transaction = deposit_dao_transaction(node0);
            ensure_committed(node0, &transaction)
        };
        let transaction = withdraw_dao_transaction(node0, deposited.0.clone(), deposited.1.clone());
        ensure_committed(node0, &transaction);
    }
}

pub struct WithdrawAndDepositDAOWithinSameTx;

impl Spec for WithdrawAndDepositDAOWithinSameTx {
    crate::name!("withdraw_and_deposit_dao_within_same_tx");

    fn run(&self, net: Net) {
        let node0 = &net.nodes[0];
        node0.generate_blocks(2);

        let mut deposited = {
            let transaction = deposit_dao_transaction(node0);
            ensure_committed(node0, &transaction)
        };
        for _ in 0..5 {
            let transaction = {
                let transaction =
                    withdraw_dao_transaction(node0, deposited.0.clone(), deposited.1.clone());
                let outputs: Vec<_> = transaction
                    .outputs()
                    .into_iter()
                    .map(|cell_output| {
                        cell_output
                            .as_builder()
                            .type_(Some(deposit_dao_script()).pack())
                            .build()
                    })
                    .collect();
                transaction
                    .as_advanced_builder()
                    .set_outputs(outputs)
                    .build()
            };
            // TODO compare the reward
            deposited = ensure_committed(node0, &transaction);
        }
    }
}

pub struct WithdrawDAOWithNotMaturitySince;

impl Spec for WithdrawDAOWithNotMaturitySince {
    crate::name!("withdraw_dao_with_not_maturity_since");

    fn run(&self, net: Net) {
        let node0 = &net.nodes[0];
        node0.generate_blocks(2);

        let not_maturity = |node: &Node, previous_output: OutPoint| {
            let not_maturity_since = node.get_tip_block_number();
            CellInput::new(previous_output, not_maturity_since)
        };

        let deposited = {
            let transaction = deposit_dao_transaction(node0);
            ensure_committed(node0, &transaction)
        };
        let transaction = {
            let transaction =
                withdraw_dao_transaction(node0, deposited.0.clone(), deposited.1.clone());
            let inputs: Vec<_> = transaction
                .input_pts_iter()
                .map(|out_point| not_maturity(node0, out_point.clone()))
                .collect();
            transaction.as_advanced_builder().set_inputs(inputs).build()
        };
        node0.generate_blocks(20);
        assert_send_transaction_fail(node0, &transaction, "Script(ValidationFailure(-14))");
    }
}

pub struct WithdrawDAOWithOverflowCapacity;

impl Spec for WithdrawDAOWithOverflowCapacity {
    crate::name!("withdraw_dao_with_overflow_capacity");

    fn run(&self, net: Net) {
        let node0 = &net.nodes[0];
        node0.generate_blocks(2);

        let deposited = {
            let transaction = deposit_dao_transaction(node0);
            ensure_committed(node0, &transaction)
        };
        let transaction = {
            let transaction =
                withdraw_dao_transaction(node0, deposited.0.clone(), deposited.1.clone());
            let outputs: Vec<_> = transaction
                .outputs()
                .into_iter()
                .map(|cell_output| {
                    let old_capacity: Capacity = cell_output.capacity().unpack();
                    let new_capacity = old_capacity.safe_add(Capacity::one()).unwrap();
                    cell_output
                        .as_builder()
                        .capacity(new_capacity.pack())
                        .build()
                })
                .collect();
            transaction
                .as_advanced_builder()
                .set_outputs(outputs)
                .build()
        };
        node0.generate_blocks(20);
        // Withdraw DAO with empty witnesses. Return DAO script ERROR_INCORRECT_CAPACITY
        assert_send_transaction_fail(node0, &transaction, "Script(ValidationFailure(-15))");
    }
}

pub struct WithdrawDAOWithInvalidWitness;

impl Spec for WithdrawDAOWithInvalidWitness {
    crate::name!("withdraw_dao_with_invalid_witness");

    fn run(&self, net: Net) {
        let node0 = &net.nodes[0];
        node0.generate_blocks(2);

        let deposited = {
            let transaction = deposit_dao_transaction(node0);
            ensure_committed(node0, &transaction)
        };

        // Withdraw DAO with empty witnesses. Return DAO script ERROR_SYSCALL
        {
            let transaction =
                withdraw_dao_transaction(node0, deposited.0.clone(), deposited.1.clone())
                    .as_advanced_builder()
                    .set_witnesses(Vec::new())
                    .build();
            node0.generate_blocks(20);
            assert_send_transaction_fail(node0, &transaction, "Script(ValidationFailure(-4))");
        }

        // Withdraw DAO with not-enough witnesses. Return DAO script ERROR_WRONG_NUMBER_OF_ARGUMENTS
        {
            let withdraw_header_index: Bytes = 0u64.to_le_bytes().to_vec().into();
            let witness: packed::Witness = vec![withdraw_header_index.pack()].pack();
            let transaction =
                withdraw_dao_transaction(node0, deposited.0.clone(), deposited.1.clone())
                    .as_advanced_builder()
                    .set_witnesses(vec![witness])
                    .build();
            node0.generate_blocks(20);
            assert_send_transaction_fail(node0, &transaction, "Script(ValidationFailure(-2))");
        }

        // Withdraw DAO with witness has bad format. Return DAO script ERROR_ENCODING.
        {
            let witness: packed::Witness =
                vec![Bytes::new().pack(), Bytes::from(vec![0]).pack()].pack();
            let transaction =
                withdraw_dao_transaction(node0, deposited.0.clone(), deposited.1.clone())
                    .as_advanced_builder()
                    .set_witnesses(vec![witness])
                    .build();
            node0.generate_blocks(20);
            assert_send_transaction_fail(node0, &transaction, "Script(ValidationFailure(-11))");
        }

        // Withdraw DAO with witness point to out-of-index dependency. DAO script `ckb_load_header` failed
        {
            let withdraw_header_index: Bytes = 9u64.to_le_bytes().to_vec().into();
            let witness: packed::Witness =
                vec![Default::default(), withdraw_header_index.pack()].pack();
            let transaction =
                withdraw_dao_transaction(node0, deposited.0.clone(), deposited.1.clone())
                    .as_advanced_builder()
                    .set_witnesses(vec![witness])
                    .build();
            node0.generate_blocks(20);
            assert_send_transaction_fail(node0, &transaction, "Script(ValidationFailure(1))");
        }
    }
}

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
