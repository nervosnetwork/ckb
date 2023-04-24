use crate::tests::util::{start_chain, start_chain_with_tx_pool_config};
use ckb_app_config::TxPoolConfig;
use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_dao_utils::genesis_dao_data;
use ckb_test_chain_utils::{is_even_lib, load_is_even};
use ckb_types::core::tx_pool::TxStatus;
use ckb_types::prelude::*;
use ckb_types::{
    bytes::Bytes,
    core::{
        capacity_bytes,
        hardfork::{HardForks, CKB2021, CKB2023},
        BlockBuilder, Capacity, EpochNumberWithFraction, ScriptHashType, TransactionBuilder,
        TransactionView,
    },
    packed::{self, CellDep, CellInput, CellOutputBuilder, OutPoint, Script},
    utilities::DIFF_TWO,
};

const TX_FEE: Capacity = capacity_bytes!(10);

pub(crate) fn create_load_is_even_script_tx() -> TransactionView {
    let (ref load_is_even_cell, ref load_is_even_data, ref load_is_even_script) = load_is_even();
    let (ref is_even_lib_cell, ref is_even_lib_data, _) = is_even_lib();
    TransactionBuilder::default()
        .witness(load_is_even_script.clone().into_witness())
        .input(CellInput::new(OutPoint::null(), 0))
        .output(load_is_even_cell.clone())
        .output_data(load_is_even_data.pack())
        .output(is_even_lib_cell.clone())
        .output_data(is_even_lib_data.pack())
        .build()
}

pub(crate) fn create_call_load_is_even_tx(parent: &TransactionView, index: u32) -> TransactionView {
    let is_even_lib = OutPoint::new(create_load_is_even_script_tx().hash(), 1);
    let load_is_even = OutPoint::new(create_load_is_even_script_tx().hash(), 0);
    let input_cap: Capacity = parent
        .outputs()
        .get(0)
        .expect("get output index 0")
        .capacity()
        .unpack();

    TransactionBuilder::default()
        .output(
            CellOutputBuilder::default()
                .capacity(input_cap.safe_sub(TX_FEE).unwrap().pack())
                .build(),
        )
        .output_data(Bytes::new().pack())
        .input(CellInput::new(OutPoint::new(parent.hash(), index), 0))
        .cell_dep(CellDep::new_builder().out_point(is_even_lib).build())
        .cell_dep(CellDep::new_builder().out_point(load_is_even).build())
        .build()
}

#[test]
fn test_load_code() {
    let (_, _, load_is_even_script) = load_is_even();
    let (_, _, is_even_lib_script) = is_even_lib();
    let load_is_even_script_tx = create_load_is_even_script_tx();

    let args: packed::Bytes = {
        let number = 0x01u64; // a random odd value

        let data_hash = is_even_lib_script.code_hash().raw_data();
        let mut vec = Vec::with_capacity(40);
        vec.extend_from_slice(&number.to_le_bytes());
        vec.extend_from_slice(&data_hash);
        vec.pack()
    };

    let lock_script = Script::new_builder()
        .hash_type(ScriptHashType::Data.into())
        .code_hash(load_is_even_script.code_hash())
        .args(args)
        .build();

    let issue_tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), 0))
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(5_000).pack())
                .lock(lock_script)
                .build(),
        )
        .output_data(Bytes::new().pack())
        .build();

    let dao = genesis_dao_data(vec![&load_is_even_script_tx, &issue_tx]).unwrap();

    let genesis_block = BlockBuilder::default()
        .transaction(load_is_even_script_tx)
        .transaction(issue_tx.clone())
        .compact_target(DIFF_TWO.pack())
        .dao(dao)
        .build();

    let consensus = ConsensusBuilder::default()
        .cellbase_maturity(EpochNumberWithFraction::new(0, 0, 1))
        .genesis_block(genesis_block)
        .build();

    let (_chain_controller, shared, _parent) = start_chain(Some(consensus));

    let tx = create_call_load_is_even_tx(&issue_tx, 0);

    let tx_pool = shared.tx_pool_controller();
    let ret = tx_pool.submit_local_tx(tx.clone()).unwrap();
    assert!(ret.is_ok(), "ret {ret:?}");
    let tx_status = tx_pool.get_tx_status(tx.hash());
    assert_eq!(
        tx_status.unwrap().unwrap(),
        (TxStatus::Pending, Some(11174))
    );
}

#[test]
fn test_load_code_with_snapshot() {
    let (_, _, load_is_even_script) = load_is_even();
    let (_, _, is_even_lib_script) = is_even_lib();
    let load_is_even_script_tx = create_load_is_even_script_tx();

    let args: packed::Bytes = {
        let number = 0x01u64; // a random odd value

        let data_hash = is_even_lib_script.code_hash().raw_data();
        let mut vec = Vec::with_capacity(40);
        vec.extend_from_slice(&number.to_le_bytes());
        vec.extend_from_slice(&data_hash);
        vec.pack()
    };

    let lock_script = Script::new_builder()
        .hash_type(ScriptHashType::Data.into())
        .code_hash(load_is_even_script.code_hash())
        .args(args)
        .build();

    let issue_tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), 0))
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(5_000).pack())
                .lock(lock_script)
                .build(),
        )
        .output_data(Bytes::new().pack())
        .build();

    let dao = genesis_dao_data(vec![&load_is_even_script_tx, &issue_tx]).unwrap();

    let genesis_block = BlockBuilder::default()
        .transaction(load_is_even_script_tx)
        .transaction(issue_tx.clone())
        .compact_target(DIFF_TWO.pack())
        .dao(dao)
        .build();

    let consensus = ConsensusBuilder::default()
        .cellbase_maturity(EpochNumberWithFraction::new(0, 0, 1))
        .genesis_block(genesis_block)
        .build();
    {
        let tx_pool_config = TxPoolConfig {
            max_tx_verify_cycles: 10_000, // 10_000/ 11_740
            ..Default::default()
        };

        let (_chain_controller, shared, _parent) =
            start_chain_with_tx_pool_config(Some(consensus), tx_pool_config);

        let tx = create_call_load_is_even_tx(&issue_tx, 0);

        let tx_pool = shared.tx_pool_controller();
        let ret = tx_pool.submit_local_tx(tx.clone()).unwrap();
        assert!(ret.is_ok(), "ret {ret:?}");

        let mut counter = 0;
        loop {
            let tx_status = tx_pool.get_tx_status(tx.hash());
            if let Ok(Ok((status, _))) = tx_status {
                if status == TxStatus::Pending {
                    break;
                }
            }
            // wait tx_pool if got `None`
            counter += 1;
            if counter > 100 {
                panic!("resume verification seems too slow");
            } else {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    }
}

fn _test_load_code_with_snapshot_after_hardfork(script_type: ScriptHashType) {
    let (_, _, load_is_even_script) = load_is_even();
    let (_, _, is_even_lib_script) = is_even_lib();
    let load_is_even_script_tx = create_load_is_even_script_tx();

    let args: packed::Bytes = {
        let number = 0x01u64; // a random odd value

        let data_hash = is_even_lib_script.code_hash().raw_data();
        let mut vec = Vec::with_capacity(40);
        vec.extend_from_slice(&number.to_le_bytes());
        vec.extend_from_slice(&data_hash);
        vec.pack()
    };

    let lock_script = Script::new_builder()
        .hash_type(script_type.into())
        .code_hash(load_is_even_script.code_hash())
        .args(args)
        .build();

    let issue_tx = TransactionBuilder::default()
        .input(CellInput::new(OutPoint::null(), 0))
        .output(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(5_000).pack())
                .lock(lock_script)
                .build(),
        )
        .output_data(Bytes::new().pack())
        .build();

    let dao = genesis_dao_data(vec![&load_is_even_script_tx, &issue_tx]).unwrap();

    let genesis_block = BlockBuilder::default()
        .transaction(load_is_even_script_tx)
        .transaction(issue_tx.clone())
        .compact_target(DIFF_TWO.pack())
        .dao(dao)
        .build();

    let hardfork_switch = HardForks {
        ckb2021: CKB2021::new_mirana()
            .as_builder()
            .rfc_0032(0)
            .build()
            .unwrap(),
        ckb2023: CKB2023::new_mirana().as_builder().build().unwrap(),
    };
    let consensus = ConsensusBuilder::default()
        .cellbase_maturity(EpochNumberWithFraction::new(0, 0, 1))
        .genesis_block(genesis_block)
        .hardfork_switch(hardfork_switch)
        .build();

    {
        let tx_pool_config = TxPoolConfig {
            max_tx_verify_cycles: 10_000, // 10_000/ 11_740
            ..Default::default()
        };

        let (_chain_controller, shared, _parent) =
            start_chain_with_tx_pool_config(Some(consensus), tx_pool_config);

        let tx = create_call_load_is_even_tx(&issue_tx, 0);

        let tx_pool = shared.tx_pool_controller();
        let ret = tx_pool.submit_local_tx(tx.clone()).unwrap();
        assert!(ret.is_ok(), "ret {ret:?}");

        let mut counter = 0;
        loop {
            let tx_status = tx_pool.get_tx_status(tx.hash());
            if let Ok(Ok((status, _))) = tx_status {
                if status == TxStatus::Pending {
                    break;
                }
            }
            // wait tx_pool if got `None`
            counter += 1;
            if counter > 100 {
                panic!("resume verification seems too slow");
            } else {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    }
}

#[test]
fn test_load_code_with_snapshot_after_hardfork() {
    _test_load_code_with_snapshot_after_hardfork(ScriptHashType::Data);
    _test_load_code_with_snapshot_after_hardfork(ScriptHashType::Data1);
}
