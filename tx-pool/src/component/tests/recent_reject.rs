use ckb_hash::blake2b_256;
use ckb_types::{core::tx_pool::Reject, packed::Byte32};

use crate::component::recent_reject::RecentReject;

#[test]
fn network_verification_timeout_cannot_create_or_overwrite_recent_rejection() {
    let tmp = tempfile::tempdir().unwrap();
    let recent = RecentReject::build(tmp.path(), 1, 10, -1).unwrap();
    let hash = Byte32::new([1; 32]);
    assert!(recent.put(&hash, Reject::ExcessiveVerifyTime).is_err());
    assert!(recent.get(&hash).unwrap().is_none());
    assert_eq!(recent.get_estimate_total_keys_num(), 0);

    recent
        .put(&hash, Reject::Full("capacity".to_owned()))
        .unwrap();
    let recorded = recent.get(&hash).unwrap();
    assert!(recorded.is_some());
    assert!(recent.put(&hash, Reject::ExcessiveVerifyTime).is_err());
    assert_eq!(recent.get(&hash).unwrap(), recorded);
    assert_eq!(recent.get_estimate_total_keys_num(), 1);
}

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
            Reject::Malformed(i.to_string(), Default::default())
                .try_into()
                .unwrap();
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

/// A failed shard recreation leaves reads as cache misses until the next
/// write recreates the shard and records its rejection.
#[test]
fn missing_shard_reads_as_cache_miss_and_recovers_on_write() {
    let tmp_dir = tempfile::Builder::new().tempdir().unwrap();
    let recent_reject = RecentReject::build(tmp_dir.path(), 1, 100, -1).unwrap();
    let key = Byte32::new(blake2b_256(7u64.to_le_bytes()));
    let rejection = Reject::Malformed("retained".to_string(), Default::default());

    recent_reject.put(&key, rejection.clone()).unwrap();
    let recorded = recent_reject.get(&key).unwrap().unwrap();
    recent_reject.drop_hash_shard_for_test(&key);

    assert_eq!(recent_reject.get(&key).unwrap(), None);
    let before_recovery = recent_reject.get_estimate_total_keys_num();
    recent_reject.put(&key, rejection.clone()).unwrap();
    assert_eq!(
        recent_reject.get(&key).unwrap().as_deref(),
        Some(recorded.as_str())
    );
    assert_eq!(
        recent_reject.get_estimate_total_keys_num(),
        before_recovery + 1
    );
    recent_reject.put(&key, rejection).unwrap();
    assert_eq!(
        recent_reject.get_estimate_total_keys_num(),
        before_recovery + 1
    );
}

#[test]
fn shrink_reestimates_removed_keys_before_evicting_new_rejections() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let recent = RecentReject::build(tmp_dir.path(), 1, 3, -1).unwrap();
    for byte in 0..3 {
        recent
            .put(&Byte32::new([byte; 32]), Reject::Full("old".to_owned()))
            .unwrap();
    }
    assert_eq!(recent.get_estimate_total_keys_num(), 3);

    // Remove real database contents without changing the cached count. This
    // deterministically models the historical overestimate left when TTL
    // compaction removes keys, and also exercises on-demand CF recreation.
    let old = Byte32::new([0; 32]);
    recent.drop_hash_shard_for_test(&old);
    assert!(recent.get(&old).unwrap().is_none());
    assert_eq!(recent.get_estimate_total_keys_num(), 3);

    let fresh = Byte32::new([3; 32]);
    recent
        .put(&fresh, Reject::Full("fresh".to_owned()))
        .unwrap();
    assert!(
        recent.get(&fresh).unwrap().is_some(),
        "historical overestimates must not evict the only current rejection"
    );
    assert_eq!(recent.get_estimate_total_keys_num(), 1);

    recent
        .put(&fresh, Reject::Full("updated".to_owned()))
        .unwrap();
    assert!(recent.get(&fresh).unwrap().is_some());
    assert_eq!(recent.get_estimate_total_keys_num(), 1);
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
