use ckb_hash::blake2b_256;
use ckb_types::{core::tx_pool::Reject, packed::Byte32};

use crate::component::recent_reject::RecentReject;

#[test]
fn test_basic() {
    let tmp_dir = tempfile::Builder::new().tempdir().unwrap();
    let shard_num = 2;
    let limit = 100;
    let ttl = -1;

    let mut recent_reject = RecentReject::build(tmp_dir.path(), shard_num, limit, ttl).unwrap();

    for i in 0..80u64 {
        let key = Byte32::new(blake2b_256(i.to_le_bytes()));
        recent_reject
            .put(&key, Reject::Malformed(i.to_string(), Default::default()))
            .unwrap();
    }

    for i in 0..80u64 {
        let key = Byte32::new(blake2b_256(i.to_le_bytes()));
        let reject: ckb_jsonrpc_types::PoolTransactionReject =
            Reject::Malformed(i.to_string(), Default::default()).into();
        assert_eq!(
            recent_reject.get(&key).unwrap().unwrap(),
            serde_json::to_string(&reject).unwrap()
        )
    }

    for i in 0..80u64 {
        let key = Byte32::new(blake2b_256(i.to_le_bytes()));
        recent_reject
            .put(&key, Reject::Malformed(i.to_string(), Default::default()))
            .unwrap();
    }

    assert!(recent_reject.total_keys_num < 100);
}

#[test]
fn put_enforces_count_limit_after_successful_writes() {
    let tmp_dir = tempfile::Builder::new().tempdir().unwrap();
    let shard_num = 1;
    let limit = 1;
    let ttl = -1;

    let mut recent_reject = RecentReject::build(tmp_dir.path(), shard_num, limit, ttl).unwrap();
    let first_key = Byte32::new(blake2b_256(1u64.to_le_bytes()));
    let second_key = Byte32::new(blake2b_256(2u64.to_le_bytes()));

    recent_reject
        .put(
            &first_key,
            Reject::Malformed("first".to_string(), Default::default()),
        )
        .unwrap();
    assert_eq!(recent_reject.get_estimate_total_keys_num(), 1);
    assert!(recent_reject.get(&first_key).unwrap().is_some());

    recent_reject
        .put(
            &second_key,
            Reject::Malformed("second".to_string(), Default::default()),
        )
        .unwrap();

    assert!(recent_reject.get_estimate_total_keys_num() <= limit);
    assert!(recent_reject.get(&first_key).unwrap().is_none());
}
