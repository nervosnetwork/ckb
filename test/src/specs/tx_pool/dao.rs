use crate::utils::{assert_send_transaction_fail, is_committed};
use crate::{Net, Node, Spec};
use ckb_core::script::{Script, ScriptHashType};
use ckb_core::transaction::{
    CellDep, CellInput, CellOutput, OutPoint, Transaction, TransactionBuilder,
};
use ckb_core::{BlockNumber, Bytes, Capacity};
use ckb_resource::CODE_HASH_DAO;
use ckb_test_chain_utils::always_success_cell;
use numext_fixed_hash::H256;

const SYSTEM_CELL_ALWAYS_SUCCESS_INDEX: u32 = 1;
const SYSTEM_CELL_DAO_INDEX: u32 = 3;
const WITHDRAW_WINDOW_LEFT: u64 = 10;
const WITHDRAW_HEADER_INDEX: u8 = 0;

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
                    let tx_hash = node0.get_tip_block().transactions()[0].hash().to_owned();
                    OutPoint::new(tx_hash, 0)
                };
                TransactionBuilder::from_transaction(deposit_dao_transaction(node0))
                    .inputs_clear()
                    .input(CellInput::new(out_point_without_block_hash, 0))
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
                    .iter()
                    .cloned()
                    .map(|mut cell_output| {
                        cell_output.type_ = Some(deposit_dao_script());
                        cell_output
                    })
                    .collect();
                TransactionBuilder::from_transaction(transaction)
                    .outputs_clear()
                    .outputs(outputs)
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
            TransactionBuilder::from_transaction(transaction)
                .inputs_clear()
                .inputs(inputs)
                .build()
        };
        node0.generate_blocks(20);
        assert_send_transaction_fail(
            node0,
            &transaction,
            "InvalidTx(ScriptFailure(ValidationFailure(-14)))",
        );
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
                .iter()
                .cloned()
                .map(|mut cell_output| {
                    cell_output.capacity = cell_output.capacity.safe_add(Capacity::one()).unwrap();
                    cell_output
                })
                .collect();
            TransactionBuilder::from_transaction(transaction)
                .outputs_clear()
                .outputs(outputs)
                .build()
        };
        node0.generate_blocks(20);
        // Withdraw DAO with empty witnesses. Return DAO script ERROR_INCORRECT_CAPACITY
        assert_send_transaction_fail(
            node0,
            &transaction,
            "InvalidTx(ScriptFailure(ValidationFailure(-15)))",
        );
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
            let transaction = TransactionBuilder::from_transaction(withdraw_dao_transaction(
                node0,
                deposited.0.clone(),
                deposited.1.clone(),
            ))
            .witnesses_clear()
            .build();
            node0.generate_blocks(20);
            assert_send_transaction_fail(
                node0,
                &transaction,
                "InvalidTx(ScriptFailure(ValidationFailure(-4)))",
            );
        }

        // Withdraw DAO with not-enough witnesses. Return DAO script ERROR_WRONG_NUMBER_OF_ARGUMENTS
        {
            let transaction = TransactionBuilder::from_transaction(withdraw_dao_transaction(
                node0,
                deposited.0.clone(),
                deposited.1.clone(),
            ))
            .witnesses_clear()
            .witness(vec![Bytes::from(vec![0, 0, 0, 0, 0, 0, 0, 0])])
            .build();
            node0.generate_blocks(20);
            assert_send_transaction_fail(
                node0,
                &transaction,
                "InvalidTx(ScriptFailure(ValidationFailure(-2)))",
            );
        }

        // Withdraw DAO with witness has bad format. Return DAO script ERROR_ENCODING.
        {
            let withdraw_header_index = vec![0];
            let witness = vec![Bytes::new(), Bytes::from(withdraw_header_index)];
            let transaction = TransactionBuilder::from_transaction(withdraw_dao_transaction(
                node0,
                deposited.0.clone(),
                deposited.1.clone(),
            ))
            .witnesses_clear()
            .witness(witness)
            .build();
            node0.generate_blocks(20);
            assert_send_transaction_fail(
                node0,
                &transaction,
                "InvalidTx(ScriptFailure(ValidationFailure(-11)))",
            );
        }

        // Withdraw DAO with witness point to out-of-index dependency. DAO script `ckb_load_header` failed
        {
            let withdraw_header_index = vec![0, 0, 0, 0, 0, 0, 0, 9];
            let witness = vec![Bytes::new(), Bytes::from(withdraw_header_index)];
            let transaction = TransactionBuilder::from_transaction(withdraw_dao_transaction(
                node0,
                deposited.0.clone(),
                deposited.1.clone(),
            ))
            .witnesses_clear()
            .witness(witness)
            .build();
            node0.generate_blocks(20);
            assert_send_transaction_fail(
                node0,
                &transaction,
                "InvalidTx(ScriptFailure(ValidationFailure(1)))",
            );
        }
    }
}

// Send the given transaction and ensure it being committed
fn ensure_committed(node: &Node, transaction: &Transaction) -> (OutPoint, H256) {
    // Ensure the transaction's cellbase-maturity and since-maturity
    node.generate_blocks(20);

    let tx_hash = node.rpc_client().send_transaction(transaction.into());

    // Ensure the sent transaction is beyond the proposal-window
    node.generate_blocks(20);

    let tx_status = node
        .rpc_client()
        .get_transaction(tx_hash.clone())
        .expect("get sent transaction");
    assert!(
        is_committed(&tx_status),
        "ensure_committed failed {:#x}",
        tx_hash
    );

    let block_hash = tx_status.tx_status.block_hash.unwrap();
    (OutPoint::new(tx_hash, 0), block_hash)
}

fn tip_cellbase_input(node: &Node) -> (CellInput, H256, Capacity) {
    let tip_block = node.get_tip_block();
    let cellbase = tip_block.transactions()[0].clone();
    let block_hash = tip_block.header().hash().to_owned();
    let tx_hash = cellbase.hash().to_owned();
    let previous_out_point = OutPoint::new(tx_hash, 0);
    let capacity = cellbase.outputs_capacity().unwrap();
    (CellInput::new(previous_out_point, 0), block_hash, capacity)
}

// deps = [always-success-cell, dao-cell]
fn deposit_dao_deps(node: &Node) -> (Vec<CellDep>, Vec<H256>) {
    let genesis_block = node.get_block_by_number(0);
    let genesis_tx = &genesis_block.transactions()[0];

    // Reference to AlwaysSuccess lock_script, to unlock the cellbase
    let always_dep = CellDep::new_cell(OutPoint::new(
        genesis_tx.hash().to_owned(),
        SYSTEM_CELL_ALWAYS_SUCCESS_INDEX,
    ));
    // Reference to DAO type_script
    let dao_dep = CellDep::new_cell(OutPoint::new(
        genesis_tx.hash().to_owned(),
        SYSTEM_CELL_DAO_INDEX,
    ));

    (
        vec![always_dep, dao_dep],
        vec![genesis_block.header().hash().to_owned()],
    )
}

// deps = [cell-point-to-withdraw-header, always-success-cell, dao-cell]
fn withdraw_dao_deps(node: &Node, withdraw_header_hash: H256) -> (Vec<CellDep>, Vec<H256>) {
    let (deposit_cell_deps, mut deposit_header_deps) = deposit_dao_deps(node);
    deposit_header_deps.push(withdraw_header_hash);
    (deposit_cell_deps, deposit_header_deps)
}

fn deposit_dao_script() -> Script {
    Script::new(vec![], CODE_HASH_DAO, ScriptHashType::Data)
}

// Deposit `capacity` into DAO. The target output's type script == dao-script
fn deposit_dao_output(capacity: Capacity) -> (CellOutput, Bytes) {
    let always_success_script = always_success_cell().2.clone();
    let data = Bytes::from(vec![1; 10]);
    let cell_output = CellOutput::new(
        capacity,
        CellOutput::calculate_data_hash(&data),
        always_success_script,
        Some(deposit_dao_script()),
    );
    (cell_output, data)
}

// Withdraw `capacity` from DAO. the target output's type script is NONE
fn withdraw_dao_output(capacity: Capacity) -> (CellOutput, Bytes) {
    let always_success_script = always_success_cell().2.clone();
    let data = Bytes::from(vec![1; 10]);
    let cell_output = CellOutput::new(
        capacity,
        CellOutput::calculate_data_hash(&data),
        always_success_script,
        None,
    );
    (cell_output, data)
}

// Put the withdraw_header_index into the 1st witness
fn withdraw_dao_witness() -> Vec<Bytes> {
    let withdraw_header_index = vec![0, 0, 0, 0, 0, 0, 0, WITHDRAW_HEADER_INDEX];
    vec![Bytes::new(), Bytes::from(withdraw_header_index)]
}

fn absolute_minimal_since(node: &Node) -> BlockNumber {
    node.get_tip_block_number() + WITHDRAW_WINDOW_LEFT
}

// Construct a deposit dao transaction, which consumes the tip-cellbase as the input,
// generates the output with always-success-script as lock script, dao-script as type script
fn deposit_dao_transaction(node: &Node) -> Transaction {
    let (input, block_hash, input_capacity) = tip_cellbase_input(node);
    let (output, output_data) = deposit_dao_output(input_capacity);
    let (cell_deps, mut header_deps) = deposit_dao_deps(node);
    header_deps.push(block_hash);
    TransactionBuilder::default()
        .cell_deps(cell_deps)
        .header_deps(header_deps)
        .input(input)
        .output(output)
        .output_data(output_data)
        .build()
}

// Construct a withdraw dao transaction, which consumes the tip-cellbase and a given deposited cell
// as the inputs, generates the output with always-success-script as lock script, none type script
fn withdraw_dao_transaction(node: &Node, out_point: OutPoint, block_hash: H256) -> Transaction {
    let withdraw_header_hash: H256 = node.get_tip_block().header().hash().to_owned();
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
    TransactionBuilder::default()
        .cell_deps(cell_deps)
        .header_deps(header_deps)
        .input(deposited_input)
        .output(output)
        .output_data(output_data)
        .witness(withdraw_dao_witness())
        .build()
}
