use crate::tests::{RpcTestRequest, RpcTestSuite, setup};
use ckb_test_chain_utils::always_success_consensus;

// JSON-RPC standard error code for invalid params.
const INVALID_PARAMS_CODE: i64 = -32602;

fn setup_net_rpc() -> RpcTestSuite {
    setup(always_success_consensus())
}

// A relative `set_ban` whose `ban_time` overflows when added to the current
// timestamp must be rejected with a JSON-RPC error instead of panicking the
// request handler.
#[test]
fn test_set_ban_with_overflowing_relative_ban_time_returns_error() {
    let suite = setup_net_rpc();
    let response = suite.rpc(&RpcTestRequest {
        id: 42,
        jsonrpc: "2.0".to_string(),
        method: "set_ban".to_string(),
        params: vec![
            "127.0.0.1".into(),
            "insert".into(),
            // u64::MAX milliseconds, relative to now -> overflow.
            "0xffffffffffffffff".into(),
            false.into(),
            "set_ban overflow test".into(),
        ],
    });

    assert!(
        response.result.is_null(),
        "overflowing set_ban must not succeed, got result {:?}",
        response.result
    );
    assert_eq!(
        response.error["code"].as_i64(),
        Some(INVALID_PARAMS_CODE),
        "expected InvalidParams error, got {:?}",
        response.error
    );
}

// An absolute `set_ban` with the same large value is a valid timestamp and must
// still succeed, so the overflow guard only affects the relative branch.
#[test]
fn test_set_ban_with_absolute_ban_time_still_succeeds() {
    let suite = setup_net_rpc();
    let response = suite.rpc(&RpcTestRequest {
        id: 42,
        jsonrpc: "2.0".to_string(),
        method: "set_ban".to_string(),
        params: vec![
            "127.0.0.1".into(),
            "insert".into(),
            "0xffffffffffffffff".into(),
            true.into(),
            "set_ban absolute test".into(),
        ],
    });

    assert!(
        response.error.is_null(),
        "absolute set_ban must succeed, got error {:?}",
        response.error
    );
}

// A normal relative ban (the default duration) must keep working.
#[test]
fn test_set_ban_with_relative_ban_time_succeeds() {
    let suite = setup_net_rpc();
    let response = suite.rpc(&RpcTestRequest {
        id: 42,
        jsonrpc: "2.0".to_string(),
        method: "set_ban".to_string(),
        params: vec![
            "127.0.0.1".into(),
            "insert".into(),
            // 1 hour in milliseconds.
            "0x36ee80".into(),
            false.into(),
            "set_ban relative test".into(),
        ],
    });

    assert!(
        response.error.is_null(),
        "relative set_ban must succeed, got error {:?}",
        response.error
    );
}
