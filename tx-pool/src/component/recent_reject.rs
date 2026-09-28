use crate::error::Reject;
use crate::util::block_offload;
use ckb_db::DBWithTTL;
use ckb_error::{AnyError, OtherError};
use ckb_logger::error;
use ckb_types::{packed::Byte32, prelude::*};
use rand::distributions::Uniform;
use rand::{Rng, thread_rng};
use std::num::NonZeroU32;
use std::path::Path;
use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};

const DEFAULT_SHARDS: u32 = 5;

/// Persistent, sharded store for recently rejected transactions.
///
/// Entries are kept in a RocksDB database with TTL-based expiration and a
/// rough key-count cap.  When the cap is exceeded a random shard is dropped
/// and recreated to reclaim space.
#[derive(Debug)]
pub struct RecentReject {
    ttl: i32,
    shard_num: NonZeroU32,
    count_limit: u64,
    /// Approximate key count across all shards. Puts increment it inside the DB
    /// guard, and shrink reconciles it with all existing shards under the write
    /// guard. Concurrent puts of the same new key can both count; TTL compaction
    /// can remove keys without updating it. Reconciliation clears that history
    /// without overwriting concurrent put increments.
    total_keys_num: AtomicU64,
    /// The `RwLock` protects the **Rust-side** `BTreeMap<String, ColumnFamily>`
    /// inside `DBWithTTL`, not RocksDB itself (the C API is already
    /// thread-safe).  `put` / `get` acquire a *read* lock (concurrent), while
    /// `shrink` acquires a *write* lock (exclusive) to drop and recreate a
    /// column family.
    ///
    /// Blocking entry points own [`block_offload`] for the whole DB operation,
    /// including shard recovery and shrinking, on multi-threaded Tokio runtimes.
    db: RwLock<DBWithTTL>,
}

impl RecentReject {
    fn increment_approximate_count(&self) {
        // The counter is intentionally approximate, but wrapping to zero would
        // disable the shrink trigger. Saturation keeps the trigger active at
        // the representational limit.
        let _ = self
            .total_keys_num
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
                Some(count.saturating_add(1))
            });
    }

    /// Opens a new `RecentReject` database at `path` with the default number
    /// of shards.
    ///
    /// `count_limit` is the approximate maximum number of entries before a
    /// shard is dropped and recreated.  `ttl` is the RocksDB TTL in seconds.
    pub fn new<P>(path: P, count_limit: u64, ttl: i32) -> Result<RecentReject, AnyError>
    where
        P: AsRef<Path>,
    {
        Self::build(path, DEFAULT_SHARDS, count_limit, ttl)
    }

    pub(crate) fn build<P>(
        path: P,
        shard_num: u32,
        count_limit: u64,
        ttl: i32,
    ) -> Result<RecentReject, AnyError>
    where
        P: AsRef<Path>,
    {
        let shard_num = NonZeroU32::new(shard_num)
            .ok_or_else(|| OtherError::new("recent-reject shard count must be non-zero"))?;
        block_offload(|| {
            let cf_names = (0..shard_num.get()).map(|c| c.to_string());
            let db = DBWithTTL::open_cf(path, cf_names, ttl)?;
            let total_keys_num = Self::estimate_total_keys_num(&db, shard_num)?;

            Ok(RecentReject {
                shard_num,
                count_limit,
                ttl,
                db: RwLock::new(db),
                total_keys_num: AtomicU64::new(total_keys_num),
            })
        })
    }

    /// Stores a rejection reason for `hash`.
    ///
    /// The reject reason is serialized as JSON and written to the shard
    /// selected from the first four bytes of `hash`.
    pub fn put(&self, hash: &Byte32, reject: Reject) -> Result<(), AnyError> {
        let reject: ckb_jsonrpc_types::PoolTransactionReject = reject.try_into()?;
        let json_string = serde_json::to_string(&reject)?;
        self.put_serialized(hash, &json_string)
    }

    /// Store an already serialized public rejection payload.
    ///
    /// The effect journal uses this entry point so its stable record owns only
    /// an exact, bounded string rather than a rich verifier error that may
    /// contain shared packed views or other hidden allocations.
    pub(crate) fn put_serialized(&self, hash: &Byte32, json_string: &str) -> Result<(), AnyError> {
        let hash_slice = hash.as_slice();
        let shard = self.get_shard(hash).to_string();
        let json_bytes = json_string.as_bytes();

        block_offload(|| {
            // Fast path: hold the read lock across the DB write so that
            // `shrink` cannot drop the column family while we write to it.
            let written = {
                let db = self.db.read().map_err(|e| OtherError::new(e.to_string()))?;
                if db.has_cf(&shard) {
                    let existed = db.get_pinned(&shard, hash_slice)?.is_some();
                    db.put(&shard, hash_slice, json_bytes)?;
                    if !existed {
                        // Count newly inserted keys inside the DB critical
                        // section, ordered with `shrink`'s reconciliation.
                        // Overwrites do not inflate the approximate counter.
                        self.increment_approximate_count();
                    }
                    true
                } else {
                    false
                }
            };

            if !written {
                // Slow path: recreate a missing shard under the write lock,
                // after releasing the fast path's read guard.
                let mut db = self
                    .db
                    .write()
                    .map_err(|e| OtherError::new(e.to_string()))?;
                // Another writer may have recreated the shard while this call
                // released its read guard and waited for the write guard.
                if !db.has_cf(&shard) {
                    db.create_cf_with_ttl(&shard, self.ttl)?;
                }
                db.put(&shard, hash_slice, json_bytes)?;
                // The shard was missing a moment ago, so count this write as
                // a new key. Concurrent puts of the same key can double-count;
                // that is inside the counter's approximate tolerance.
                self.increment_approximate_count();
            }
            self.maybe_shrink();
            Ok(())
        })
    }

    /// Check the approximate counter (already incremented by `put` inside
    /// the critical section) and shrink one shard if the limit is exceeded.
    fn maybe_shrink(&self) {
        let count = self.total_keys_num.load(Ordering::SeqCst);
        if count > self.count_limit
            && let Err(e) = self.shrink()
        {
            error!("failed to shrink recent_reject: {}", e);
        }
    }

    /// Returns the serialized rejection reason for `hash`, if one exists.
    pub fn get(&self, hash: &Byte32) -> Result<Option<String>, AnyError> {
        let slice = hash.as_slice();
        let shard = self.get_shard(hash).to_string();
        block_offload(|| {
            let db = self.db.read().map_err(|e| OtherError::new(e.to_string()))?;
            // A missing shard column family (e.g. dropped by `shrink` and
            // not yet recreated by the next `put`) means "no entry", not an
            // error for the caller.
            if !db.has_cf(&shard) {
                return Ok(None);
            }
            match db.get_pinned(&shard, slice)? {
                Some(bytes) => {
                    let s = String::from_utf8(bytes.to_vec()).map_err(|e| {
                        OtherError::new(format!("recent reject value is not valid utf-8: {e}"))
                    })?;
                    Ok(Some(s))
                }
                None => Ok(None),
            }
        })
    }

    /// Returns the approximate total number of stored rejection entries.
    ///
    /// This is a best-effort counter updated with sequentially-consistent
    /// ordering; it may still differ from the exact number of keys in
    /// the database because it is an estimate rather than an exact count.
    pub fn get_estimate_total_keys_num(&self) -> u64 {
        self.total_keys_num.load(Ordering::SeqCst)
    }

    fn estimate_total_keys_num(db: &DBWithTTL, shard_num: NonZeroU32) -> Result<u64, AnyError> {
        Self::checked_estimate_sum((0..shard_num.get()).map(|shard| {
            let shard = shard.to_string();
            if db.has_cf(&shard) {
                db.estimate_num_keys_cf(&shard).map_err(AnyError::from)
            } else {
                Ok(None)
            }
        }))
    }

    fn checked_estimate_sum(
        estimate_keys_num: impl IntoIterator<Item = Result<Option<u64>, AnyError>>,
    ) -> Result<u64, AnyError> {
        estimate_keys_num.into_iter().try_fold(0u64, |total, num| {
            let keys_num = num?.unwrap_or(0);
            total.checked_add(keys_num).ok_or_else(|| {
                OtherError::new(format!(
                    "recent reject estimated keys count overflows: {} + {}",
                    total, keys_num
                ))
                .into()
            })
        })
    }

    fn shrink(&self) -> Result<u64, AnyError> {
        let mut rng = thread_rng();
        let shard = rng
            .sample(Uniform::new(0, self.shard_num.get()))
            .to_string();
        // Exclusive write lock: blocks all concurrent put/get while we
        // drop and recreate a column family.  This is a very cold path
        // (triggered only when key count exceeds `count_limit`), so brief
        // contention is acceptable.
        let mut db = self
            .db
            .write()
            .map_err(|e| OtherError::new(e.to_string()))?;

        // TTL compaction, duplicate puts or an earlier shrink can make the
        // trigger stale. Reconcile before deciding whether to discard data.
        let total = Self::estimate_total_keys_num(&db, self.shard_num)?;
        self.total_keys_num.store(total, Ordering::SeqCst);
        if total <= self.count_limit {
            return Ok(total);
        }
        if db.has_cf(&shard) {
            db.drop_cf(&shard)?;
        }
        let create_result = db.create_cf_with_ttl(&shard, self.ttl);

        // A failed recreation leaves an empty, missing shard. Count every
        // remaining shard regardless. If estimation fails, keep the last
        // complete estimate and report the error instead of storing a
        // partial sum. Both stores share the guard with every put increment.
        let remaining = Self::estimate_total_keys_num(&db, self.shard_num);
        if let Ok(total) = &remaining {
            self.total_keys_num.store(*total, Ordering::SeqCst);
        }
        drop(db);
        if let Err(e) = create_result {
            error!("failed to recreate recent_reject shard {shard}: {e}");
        }
        remaining
    }

    fn get_shard(&self, hash: &Byte32) -> u32 {
        hash.as_slice()
            .first_chunk::<4>()
            .copied()
            .map(u32::from_le_bytes)
            .unwrap_or_default()
            .rem_euclid(self.shard_num.get())
    }
}

#[cfg(test)]
#[path = "tests/recent_reject_test_support.rs"]
mod test_support;
