use ckb_store::ChainStore;
use ckb_test_chain_utils::{always_success_cell, always_success_consensus, ckb_testnet_consensus};
use ckb_types::{
    core::{self, Capacity, TransactionBuilder},
    packed::{self, CellDep, CellInput, CellOutputBuilder, OutPoint},
    prelude::*,
};
use serde_json::json;

use crate::{
    module::pool::WellKnownScriptsOnlyValidator,
    tests::{RpcTestRequest, always_success_transaction, setup},
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
            &consensus.dao_type_hash(),
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
        let type_type_hash = consensus.dao_type_hash();
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
fn timed_out_submission_still_completes_after_verification_resumes() {
    use std::time::{Duration, Instant};

    let suite = crate::tests::setup::setup_rpc_test_suite(20, None);
    let tip = suite.shared.store().get_tip_header().unwrap();
    let block = suite.shared.store().get_block(&tip.hash()).unwrap();
    let transaction = TransactionBuilder::default()
        .input(CellInput::new(
            OutPoint::new(block.transactions()[0].hash(), 0),
            0,
        ))
        .output(
            CellOutputBuilder::default()
                .capacity(Capacity::bytes(100).unwrap())
                .lock(always_success_cell().2.clone())
                .build(),
        )
        .output_data(packed::Bytes::default())
        .cell_dep(
            CellDep::new_builder()
                .out_point(OutPoint::new(always_success_transaction().hash(), 0))
                .build(),
        )
        .build();
    let request = RpcTestRequest {
        id: 42,
        jsonrpc: "2.0".to_owned(),
        method: "send_transaction".to_owned(),
        params: vec![
            json!(ckb_jsonrpc_types::Transaction::from(transaction.data())),
            json!("passthrough"),
        ],
    };
    let controller = suite.shared.tx_pool_controller();
    let preview = controller.test_accept_tx(transaction.clone()).unwrap();
    assert!(
        preview.is_ok(),
        "submission fixture must be valid: {preview:?}"
    );
    controller.suspend_chunk_process().unwrap();
    // The existing HTTP deadline must run while submission awaits the pool.
    // Bound the client as well so a blocking-handler regression cannot hang.
    let response = suite
        .rpc_client
        .post(&suite.rpc_uri)
        .timeout(Duration::from_secs(35))
        .json(&request)
        .send();
    controller.continue_chunk_process().unwrap();
    assert_eq!(
        response.unwrap().status(),
        reqwest::StatusCode::REQUEST_TIMEOUT
    );

    // A timeout abandons the reply, not the service-owned transaction. Observe
    // accepted membership after Resume instead of assuming a completion delay.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (status, _) = controller
            .get_tx_status(transaction.hash())
            .unwrap()
            .unwrap();
        if status == core::tx_pool::TxStatus::Pending {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "timed-out submission did not complete: {status:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let duplicate = suite.rpc(&request);
    assert_eq!(duplicate.error["code"], -1107);
}

#[test]
fn timed_out_submission_during_active_verification_completes_once_and_reclaims_capacity() {
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::{Duration, Instant};

    let suite = crate::tests::setup::setup_rpc_test_suite(20, None);
    let tip = suite.shared.store().get_tip_header().unwrap();
    let block = suite.shared.store().get_block(&tip.hash()).unwrap();
    let transaction = TransactionBuilder::default()
        .input(CellInput::new(
            OutPoint::new(block.transactions()[0].hash(), 0),
            0,
        ))
        .output(
            CellOutputBuilder::default()
                .capacity(Capacity::bytes(100).unwrap())
                .lock(always_success_cell().2.clone())
                .build(),
        )
        .output_data(packed::Bytes::default())
        .cell_dep(
            CellDep::new_builder()
                .out_point(OutPoint::new(always_success_transaction().hash(), 0))
                .build(),
        )
        .build();
    let request = RpcTestRequest {
        id: 43,
        jsonrpc: "2.0".to_owned(),
        method: "send_transaction".to_owned(),
        params: vec![
            json!(ckb_jsonrpc_types::Transaction::from(transaction.data())),
            json!("passthrough"),
        ],
    };
    let controller = suite.shared.tx_pool_controller();
    let mut new_tx_rx = suite.shared.async_handle().block_on(
        suite
            .shared
            .notify_controller()
            .subscribe_new_transaction("http_408_active_verify_regression".to_owned()),
    );

    let target_hash = transaction.hash();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Arc::new(Mutex::new(release_rx));
    controller
        .set_verify_observer(Some(Arc::new(move |hash| {
            if hash == &target_hash {
                let thread_name = std::thread::current()
                    .name()
                    .unwrap_or("unnamed")
                    .to_owned();
                let _ = entered_tx.send(thread_name);
                let _ = release_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(45));
            }
        })))
        .expect("install verify observer");

    let client = suite.rpc_client.clone();
    let uri = suite.rpc_uri.clone();
    let req_clone = request.clone();
    let (resp_tx, resp_rx) = mpsc::channel();
    let caller = std::thread::spawn(move || {
        let response = client
            .post(uri)
            .timeout(Duration::from_secs(40))
            .json(&req_clone)
            .send()
            .unwrap();
        let _ = resp_tx.send(response.status());
    });

    // 1. Observe that active verification has started off the HTTP thread.
    let worker_name = entered_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("target transaction must reach active verification");
    assert!(
        worker_name.starts_with("TxPoolCpu-"),
        "local verification must run on a dedicated compute worker, got {worker_name}"
    );
    assert_eq!(controller.inflight_requests(), 1);

    // 2. HTTP deadline must return 408 independently before verification is released.
    let status = resp_rx
        .recv_timeout(Duration::from_secs(32))
        .expect("HTTP timeout must return 408 before verification finishes");
    assert_eq!(status, reqwest::StatusCode::REQUEST_TIMEOUT);
    caller.join().unwrap();
    assert_eq!(
        controller.inflight_requests(),
        1,
        "admitted work must retain its ordinary capacity permit until completion"
    );

    // 3. Release verification and confirm single admission, single notification,
    //    and complete capacity reclamation.
    release_tx.send(()).unwrap();
    controller
        .set_verify_observer(None)
        .expect("clear verify observer");

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (tx_status, _) = controller
            .get_tx_status(transaction.hash())
            .unwrap()
            .unwrap();
        if tx_status == core::tx_pool::TxStatus::Pending && controller.inflight_requests() == 0 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "timed-out submission did not settle or reclaim capacity: status={tx_status:?}, inflight={}",
            controller.inflight_requests()
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    let mut target_notifications = 0usize;
    let notify_deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < notify_deadline {
        let entry = suite.shared.async_handle().block_on(async {
            tokio::time::timeout(Duration::from_millis(100), new_tx_rx.recv()).await
        });
        match entry {
            Ok(Some(entry)) if entry.transaction.hash() == transaction.hash() => {
                target_notifications += 1;
                break;
            }
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => {}
        }
    }
    // Confirm no duplicate notification arrives.
    let extra = suite.shared.async_handle().block_on(async {
        tokio::time::timeout(Duration::from_millis(200), new_tx_rx.recv()).await
    });
    if let Ok(Some(entry)) = extra
        && entry.transaction.hash() == transaction.hash()
    {
        target_notifications += 1;
    }
    assert_eq!(
        target_notifications, 1,
        "admitted transaction must notify exactly once"
    );

    assert_eq!(controller.get_tx_pool_info().unwrap().pending_size, 1);
    assert_eq!(controller.inflight_requests(), 0);
    let duplicate = suite.rpc(&request);
    assert_eq!(duplicate.error["code"], -1107);
}

#[test]
#[ignore]
fn test_send_transaction_exceeded_maximum_ancestors_count() {
    const MAX_ANCESTORS_COUNT: u64 = 1000;

    let suite = setup(always_success_consensus());

    let store = suite.shared.store();
    let tip = store.get_tip_header().unwrap();
    let tip_block = store.get_block(&tip.hash()).unwrap();
    let mut parent_tx_hash = tip_block.transactions().first().unwrap().hash();

    // generate 2000 child-spends-parent txs
    for i in 0..(MAX_ANCESTORS_COUNT + 1) {
        let input = CellInput::new(OutPoint::new(parent_tx_hash.clone(), 0), 0);
        let output = CellOutputBuilder::default()
            .capacity(
                Capacity::bytes(1000)
                    .unwrap()
                    .safe_sub(Capacity::shannons(i * 41 * 1000))
                    .unwrap(),
            )
            .lock(always_success_cell().2.clone())
            .build();
        let cell_dep = CellDep::new_builder()
            .out_point(OutPoint::new(always_success_transaction().hash(), 0))
            .build();
        let tx = TransactionBuilder::default()
            .input(input)
            .output(output)
            .output_data(packed::Bytes::default())
            .cell_dep(cell_dep)
            .build();
        let new_tx: ckb_jsonrpc_types::Transaction = tx.data().into();
        let response = suite.rpc(&RpcTestRequest {
            id: 42,
            jsonrpc: "2.0".to_string(),
            method: "send_transaction".to_string(),
            params: vec![json!(new_tx), json!("passthrough")],
        });
        if i != MAX_ANCESTORS_COUNT {
            assert_eq!(response.error.to_string(), "null".to_string());
        } else {
            assert!(
                response
                    .error
                    .to_string()
                    .contains("ExceededMaximumAncestorsCount")
            );
        }
        parent_tx_hash = tx.hash();
    }
}

fn build_tx(
    code_hash: &packed::Byte32,
    hash_type: core::ScriptHashType,
    args: Vec<u8>,
) -> core::TransactionView {
    let lock = packed::ScriptBuilder::default()
        .code_hash(code_hash.clone())
        .hash_type(hash_type)
        .args(args)
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
        .hash_type(lock_hash_type)
        .args(lock_args)
        .build();
    let type_ = packed::ScriptBuilder::default()
        .code_hash(type_code_hash.clone())
        .hash_type(type_hash_type)
        .build();
    core::TransactionBuilder::default()
        .output(
            packed::CellOutput::new_builder()
                .lock(lock)
                .type_(Some(type_))
                .build(),
        )
        .build()
}
