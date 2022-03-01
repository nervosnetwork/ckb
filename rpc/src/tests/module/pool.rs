use std::{thread::sleep, time::Duration};

use ckb_store::ChainStore;
use ckb_test_chain_utils::always_success_cell;
use ckb_test_chain_utils::ckb_testnet_consensus;
use ckb_types::{
    core::{self, Capacity, TransactionBuilder},
    packed::{self, CellDep, CellInput, CellOutputBuilder, OutPoint},
    prelude::*,
};
use serde_json::json;

use crate::{
    module::pool::WellKnownScriptsOnlyValidator,
    tests::{always_success_transaction, setup, RpcTestRequest},
};

#[test]
fn test_default_outputs_validator() {
    let consensus = ckb_testnet_consensus();
    let validator = WellKnownScriptsOnlyValidator::new(&consensus, &[], &[]);

    {
        let type_hash = consensus
            .secp256k1_blake160_sighash_all_type_hash()
            .unwrap();
        // valid output lock
        let tx = build_tx(&type_hash, core::ScriptHashType::Type, vec![1; 20]);
        assert!(validator.validate(&tx).is_ok());

        // invalid args len
        let tx = build_tx(&type_hash, core::ScriptHashType::Type, vec![1; 19]);
        assert!(validator.validate(&tx).is_err());

        // invalid hash type
        let tx = build_tx(&type_hash, core::ScriptHashType::Data, vec![1; 20]);
        assert!(validator.validate(&tx).is_err());

        // invalid code hash
        let tx = build_tx(
            &consensus.dao_type_hash().unwrap(),
            core::ScriptHashType::Type,
            vec![1; 20],
        );
        assert!(validator.validate(&tx).is_err());
    }

    {
        let type_hash = consensus
            .secp256k1_blake160_multisig_all_type_hash()
            .unwrap();
        // valid output lock
        let tx = build_tx(&type_hash, core::ScriptHashType::Type, vec![1; 20]);
        assert!(validator.validate(&tx).is_ok());

        // valid output lock
        let since: u64 = (0b1100_0000 << 56) | 42; // relative timestamp 42 seconds
        let mut args = vec![1; 20];
        args.extend_from_slice(&since.to_le_bytes());
        let tx = build_tx(&type_hash, core::ScriptHashType::Type, args);
        assert!(validator.validate(&tx).is_ok());

        // invalid args len
        let tx = build_tx(&type_hash, core::ScriptHashType::Type, vec![1; 19]);
        assert!(validator.validate(&tx).is_err());

        // invalid hash type
        let tx = build_tx(&type_hash, core::ScriptHashType::Data, vec![1; 20]);
        assert!(validator.validate(&tx).is_err());

        // invalid since args format
        let tx = build_tx(&type_hash, core::ScriptHashType::Type, vec![1; 28]);
        assert!(validator.validate(&tx).is_err());
    }

    {
        let lock_type_hash = consensus
            .secp256k1_blake160_multisig_all_type_hash()
            .unwrap();
        let type_type_hash = consensus.dao_type_hash().unwrap();
        // valid output lock
        let tx = build_tx_with_type(
            &lock_type_hash,
            core::ScriptHashType::Type,
            vec![1; 20],
            &type_type_hash,
            core::ScriptHashType::Type,
        );
        assert!(validator.validate(&tx).is_ok());

        // valid output lock
        let since: u64 = (0b0010_0000 << 56) | 42; // absolute epoch
        let mut args = vec![1; 20];
        args.extend_from_slice(&since.to_le_bytes());
        let tx = build_tx_with_type(
            &lock_type_hash,
            core::ScriptHashType::Type,
            args,
            &type_type_hash,
            core::ScriptHashType::Type,
        );
        assert!(validator.validate(&tx).is_ok());

        // invalid since arg lock
        let since: u64 = (0b1100_0000 << 56) | 42; // relative timestamp 42 seconds
        let mut args = vec![1; 20];
        args.extend_from_slice(&since.to_le_bytes());
        let tx = build_tx_with_type(
            &lock_type_hash,
            core::ScriptHashType::Type,
            args,
            &type_type_hash,
            core::ScriptHashType::Type,
        );
        assert!(validator.validate(&tx).is_err());

        // invalid since args type
        let tx = build_tx_with_type(
            &lock_type_hash,
            core::ScriptHashType::Type,
            vec![1; 20],
            &type_type_hash,
            core::ScriptHashType::Data,
        );
        assert!(validator.validate(&tx).is_err());

        // invalid code hash
        let tx = build_tx_with_type(
            &lock_type_hash,
            core::ScriptHashType::Type,
            vec![1; 20],
            &lock_type_hash,
            core::ScriptHashType::Type,
        );
        assert!(validator.validate(&tx).is_err());
    }
}

#[test]
#[ignore]
fn test_send_transaction_exceeded_maximum_ancestors_count() {
    let suite = setup();

    let store = suite.shared.store();
    let tip = store.get_tip_header().unwrap();
    let tip_block = store.get_block(&tip.hash()).unwrap();
    let mut parent_tx_hash = tip_block.transactions().get(0).unwrap().hash();

    // generate 30 child-spends-parent txs
    for i in 0..130 {
        let input = CellInput::new(OutPoint::new(parent_tx_hash.clone(), 0), 0);
        let output = CellOutputBuilder::default()
            .capacity(Capacity::bytes(1000 - i).unwrap().pack())
            .lock(always_success_cell().2.clone())
            .build();
        let cell_dep = CellDep::new_builder()
            .out_point(OutPoint::new(always_success_transaction().hash(), 0))
            .build();
        let tx = TransactionBuilder::default()
            .input(input)
            .output(output)
            .output_data(Default::default())
            .cell_dep(cell_dep)
            .build();
        let new_tx: ckb_jsonrpc_types::Transaction = tx.data().into();
        suite.rpc(&RpcTestRequest {
            id: 42,
            jsonrpc: "2.0".to_string(),
            method: "send_transaction".to_string(),
            params: vec![json!(new_tx), json!("passthrough")],
        });
        parent_tx_hash = tx.hash();
    }

    suite.wait_block_template_array_ge("proposals", 130);

    // 130 txs will be added to proposal list
    while store.get_tip_header().unwrap().number() != (tip.number() + 2) {
        sleep(Duration::from_millis(400));
        suite.rpc(&RpcTestRequest {
            id: 42,
            jsonrpc: "2.0".to_string(),
            method: "generate_block".to_string(),
            params: vec![],
        });
    }

    // the default value of pool config `max_ancestors_count` is 125, only 125 txs will be added to committed list of the block template
    suite.wait_block_template_array_ge("transactions", 1);

    let response = suite.rpc(&RpcTestRequest {
        id: 42,
        jsonrpc: "2.0".to_string(),
        method: "get_block_template".to_string(),
        params: vec![],
    });

    assert_eq!(
        125,
        response.result["transactions"].as_array().unwrap().len()
    );
}

fn build_tx(
    code_hash: &packed::Byte32,
    hash_type: core::ScriptHashType,
    args: Vec<u8>,
) -> core::TransactionView {
    let lock = packed::ScriptBuilder::default()
        .code_hash(code_hash.clone())
        .hash_type(hash_type.into())
        .args(args.pack())
        .build();
    core::TransactionBuilder::default()
        .output(packed::CellOutput::new_builder().lock(lock).build())
        .build()
}

fn build_tx_with_type(
    lock_code_hash: &packed::Byte32,
    lock_hash_type: core::ScriptHashType,
    lock_args: Vec<u8>,
    type_code_hash: &packed::Byte32,
    type_hash_type: core::ScriptHashType,
) -> core::TransactionView {
    let lock = packed::ScriptBuilder::default()
        .code_hash(lock_code_hash.clone())
        .hash_type(lock_hash_type.into())
        .args(lock_args.pack())
        .build();
    let type_ = packed::ScriptBuilder::default()
        .code_hash(type_code_hash.clone())
        .hash_type(type_hash_type.into())
        .build();
    core::TransactionBuilder::default()
        .output(
            packed::CellOutput::new_builder()
                .lock(lock)
                .type_(Some(type_).pack())
                .build(),
        )
        .build()
}
