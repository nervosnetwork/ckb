use ckb_hash::blake2b_256;
use ckb_types::{core::tx_pool::Reject, packed::Byte32};

use crate::component::recent_reject::RecentReject;

#[test]
fn test_basic() {
    let tmp_dir = tempfile::Builder::new().tempdir().unwrap();
    let shard_num = 2;
    let limit = 100;
    let ttl = -1;

    let recent_reject = RecentReject::build(tmp_dir.path(), shard_num, limit, ttl).unwrap();

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

    assert!(recent_reject.get_estimate_total_keys_num() < 100);
}

#[test]
fn put_enforces_count_limit_after_successful_writes() {
    let tmp_dir = tempfile::Builder::new().tempdir().unwrap();
    let shard_num = 1;
    let limit = 1;
    let ttl = -1;

    let recent_reject = RecentReject::build(tmp_dir.path(), shard_num, limit, ttl).unwrap();
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

/// Bug #54: a shard can be temporarily absent while the shrink path is
/// recreating it. Reads during that window mean "no cached rejection", not a
/// database failure that should escape through RPC.
#[test]
fn get_treats_missing_shard_as_cache_miss() {
    let tmp_dir = tempfile::Builder::new().tempdir().unwrap();
    let recent_reject = RecentReject::build(tmp_dir.path(), 1, 100, -1).unwrap();
    let key = Byte32::new(blake2b_256(7u64.to_le_bytes()));

    recent_reject
        .put(
            &key,
            Reject::Malformed("before-drop".to_string(), Default::default()),
        )
        .unwrap();
    recent_reject.drop_hash_shard_for_test(&key);

    assert_eq!(recent_reject.get(&key).unwrap(), None);
}

/// Concurrent puts racing with shard drops must not make the approximate
/// counter drift monotonically: increments happen inside the same critical
/// section as the DB write, so `shrink`'s estimate and the counter stay
/// totally ordered. The only accepted overshoot is a bounded check-then-
/// shrink race across threads.
#[test]
fn concurrent_put_and_shrink_keep_counter_bounded() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    let tmp_dir = tempfile::Builder::new().tempdir().unwrap();
    let limit = 50u64;
    let threads = 4u64;
    // A single shard: every shrink deterministically drops the one column
    // family all threads are writing to.
    let recent_reject = Arc::new(RecentReject::build(tmp_dir.path(), 1, limit, -1).unwrap());

    let next = Arc::new(AtomicU64::new(0));
    std::thread::scope(|s| {
        for _ in 0..threads {
            let recent_reject = Arc::clone(&recent_reject);
            let next = Arc::clone(&next);
            s.spawn(move || {
                for _ in 0..100 {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    let key = Byte32::new(blake2b_256(i.to_le_bytes()));
                    recent_reject
                        .put(&key, Reject::Malformed(i.to_string(), Default::default()))
                        .unwrap();
                }
            });
        }
    });

    let total = recent_reject.get_estimate_total_keys_num();
    assert!(
        total <= limit + threads + 1,
        "counter must stay bounded by the limit plus the check-then-shrink race, got {total}"
    );
    // Reads still self-heal after shard drops.
    let key = Byte32::new(blake2b_256(0u64.to_le_bytes()));
    let _ = recent_reject.get(&key).unwrap();
}
