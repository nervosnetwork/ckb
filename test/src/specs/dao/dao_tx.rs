use super::*;
use crate::utils::assert_send_transaction_fail;
use crate::{Net, Node, Spec};
use ckb_types::{
    bytes::Bytes,
    core::Capacity,
    packed::{self, CellInput, OutPoint},
    prelude::*,
};

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
        let dao_type_hash = node0
            .consensus()
            .dao_type_hash()
            .expect("No dao system cell");
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
                            .type_(Some(deposit_dao_script(dao_type_hash.clone())).pack())
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
        assert_send_transaction_fail(node0, &transaction, "CapacityOverflow");
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
            assert_send_transaction_fail(node0, &transaction, "Dao(InvalidOutPoint)");
        }

        // Withdraw DAO with not-enough witnesses. Return DAO script ERROR_WRONG_NUMBER_OF_ARGUMENTS
        {
            let withdraw_header_index: Bytes = 0u64.to_le_bytes().to_vec().into();
            let witness: packed::Bytes = withdraw_header_index.pack();
            let transaction =
                withdraw_dao_transaction(node0, deposited.0.clone(), deposited.1.clone())
                    .as_advanced_builder()
                    .set_witnesses(vec![witness])
                    .build();
            node0.generate_blocks(20);
            assert_send_transaction_fail(node0, &transaction, "Dao(InvalidOutPoint)");
        }

        // Withdraw DAO with witness has bad format. Return DAO script ERROR_ENCODING.
        {
            let witness: packed::Bytes = Bytes::new().pack();
            let transaction =
                withdraw_dao_transaction(node0, deposited.0.clone(), deposited.1.clone())
                    .as_advanced_builder()
                    .set_witnesses(vec![witness])
                    .build();
            node0.generate_blocks(20);
            assert_send_transaction_fail(node0, &transaction, "Dao(InvalidDaoFormat)");
        }

        // Withdraw DAO with witness point to out-of-index dependency. DAO script `ckb_load_header` failed
        {
            let withdraw_header_index: Bytes = 9u64.to_le_bytes().to_vec().into();
            let witness: packed::Bytes = withdraw_header_index.pack();
            let transaction =
                withdraw_dao_transaction(node0, deposited.0.clone(), deposited.1.clone())
                    .as_advanced_builder()
                    .set_witnesses(vec![witness])
                    .build();
            node0.generate_blocks(20);
            assert_send_transaction_fail(node0, &transaction, "Dao(InvalidOutPoint)");
        }
    }
}
