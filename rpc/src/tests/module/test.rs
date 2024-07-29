#![allow(clippy::inconsistent_digit_grouping)]

use ckb_chain_spec::consensus::build_genesis_epoch_ext;
use ckb_store::ChainStore;
use ckb_test_chain_utils::always_success_consensus;
use ckb_types::{
    core::{Capacity, EpochNumberWithFraction},
    utilities::DIFF_TWO,
};

use crate::tests::{setup, RpcTestRequest, RpcTestSuite};

const GENESIS_EPOCH_LENGTH: u64 = 30;

#[test]
fn test_generate_epochs() {
    let suite = setup_rpc();
    assert_eq!(
        get_current_epoch(&suite),
        EpochNumberWithFraction::new(0, 20, GENESIS_EPOCH_LENGTH)
    );

    // generate 1 epoch
    suite.rpc(&RpcTestRequest {
        id: 42,
        jsonrpc: "2.0".to_string(),
        method: "generate_epochs".to_string(),
        params: vec!["0x1".into()],
    });
    assert_eq!(
        get_current_epoch(&suite),
        EpochNumberWithFraction::new(1, 20, GENESIS_EPOCH_LENGTH)
    );

    // generate 1(0/1) epoch
    suite.rpc(&RpcTestRequest {
        id: 42,
        jsonrpc: "2.0".to_string(),
        method: "generate_epochs".to_string(),
        params: vec!["0x10000000001".into()],
    });
    assert_eq!(
        "0x10000000001".to_string(),
        Into::<ckb_jsonrpc_types::Uint64>::into(EpochNumberWithFraction::new(1, 0, 1)).to_string(),
    );
    assert_eq!(
        get_current_epoch(&suite),
        EpochNumberWithFraction::new(2, 20, GENESIS_EPOCH_LENGTH)
    );

    // generate 1/2 epoch
    suite.rpc(&RpcTestRequest {
        id: 42,
        jsonrpc: "2.0".to_string(),
        method: "generate_epochs".to_string(),
        params: vec!["0x20001000000".into()],
    });
    assert_eq!(
        get_current_epoch(&suite),
        EpochNumberWithFraction::new(3, 5, GENESIS_EPOCH_LENGTH)
    );

    // generate 3/2 epoch
    suite.rpc(&RpcTestRequest {
        id: 42,
        jsonrpc: "2.0".to_string(),
        method: "generate_epochs".to_string(),
        params: vec!["0x20003000000".into()],
    });
    assert_eq!(
        get_current_epoch(&suite),
        EpochNumberWithFraction::new(4, 20, GENESIS_EPOCH_LENGTH)
    );

    // generate 0/2 epoch
    suite.rpc(&RpcTestRequest {
        id: 42,
        jsonrpc: "2.0".to_string(),
        method: "generate_epochs".to_string(),
        params: vec!["0x20000000000".into()],
    });
    assert_eq!(
        get_current_epoch(&suite),
        EpochNumberWithFraction::new(4, 20, GENESIS_EPOCH_LENGTH)
    );
}

#[test]
fn test_rpc_tcp() {
    use tokio::runtime::Runtime;

    let suite = setup_rpc();
    let rt = Runtime::new().unwrap();
    let res = rt.block_on(async move {
        suite
            .tcp(&RpcTestRequest {
                id: 42,
                jsonrpc: "2.0".to_string(),
                method: "generate_epochs".to_string(),
                params: vec!["0x20000000000".into()],
            })
            .await
    });
    assert!(res.is_ok());
    assert_eq!(res.unwrap().result, "0x1e0014000000");
}

#[test]
fn test_rpc_batch_request_limit() {
    let suite = setup_rpc();
    let single_request = RpcTestRequest {
        id: 42,
        jsonrpc: "2.0".to_string(),
        method: "generate_epochs".to_string(),
        params: vec!["0x20000000000".into()],
    };

    let mut batch_request = vec![];
    for _i in 0..1001 {
        batch_request.push(single_request.clone());
    }

    // exceed limit with 1001
    let res = suite.rpc_batch(&batch_request);
    assert!(res.is_err());
    eprintln!("res: {:?}", res);

    // batch request will success with 1000
    batch_request.remove(0);
    let res = suite.rpc_batch(&batch_request);
    assert!(res.is_ok());
}

// setup a chain for integration test rpc
fn setup_rpc() -> RpcTestSuite {
    const INITIAL_PRIMARY_EPOCH_REWARD: Capacity = Capacity::shannons(1_917_808_21917808);
    const DEFAULT_EPOCH_DURATION_TARGET: u64 = 240;
    const DEFAULT_ORPHAN_RATE_TARGET: (u32, u32) = (1, 40);
    let epoch_ext = build_genesis_epoch_ext(
        INITIAL_PRIMARY_EPOCH_REWARD,
        DIFF_TWO,
        GENESIS_EPOCH_LENGTH,
        DEFAULT_EPOCH_DURATION_TARGET,
        DEFAULT_ORPHAN_RATE_TARGET,
    );
    let mut consensus = always_success_consensus();
    consensus.genesis_epoch_ext = epoch_ext;
    consensus.epoch_duration_target = 240;
    consensus.permanent_difficulty_in_dummy = true;

    setup(consensus)
}

fn get_current_epoch(suite: &RpcTestSuite) -> EpochNumberWithFraction {
    let store = suite.shared.store();
    let tip_block_number = store.get_tip_header().unwrap().number();
    store
        .get_current_epoch_ext()
        .unwrap()
        .number_with_fraction(tip_block_number)
}
