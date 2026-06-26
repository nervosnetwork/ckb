use crate::error::Reject;
use ckb_db::DBWithTTL;
use ckb_error::{AnyError, OtherError};
use ckb_logger::error;
use ckb_types::{packed::Byte32, prelude::*};
use rand::distributions::Uniform;
use rand::{Rng, thread_rng};
use std::path::Path;
use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};

const DEFAULT_SHARDS: u32 = 5;

#[derive(Debug)]
pub struct RecentReject {
    ttl: i32,
    shard_num: u32,
    pub(crate) count_limit: u64,
    /// Approximate key count across all shards.  Updated with `Relaxed`
    /// ordering — precision is not critical, only back-pressure for `shrink`.
    total_keys_num: AtomicU64,
    /// The `RwLock` protects the **Rust-side** `BTreeMap<String, ColumnFamily>`
    /// inside `DBWithTTL`, not RocksDB itself (the C API is already
    /// thread-safe).  `put` / `get` acquire a *read* lock (concurrent), while
    /// `shrink` acquires a *write* lock (exclusive) to drop and recreate a
    /// column family.
    db: RwLock<DBWithTTL>,
}

impl RecentReject {
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
        let cf_names: Vec<_> = (0..shard_num).map(|c| c.to_string()).collect();
        let db = DBWithTTL::open_cf(path, cf_names.clone(), ttl)?;
        let estimate_keys_num = cf_names
            .iter()
            .map(|cf| db.estimate_num_keys_cf(cf))
            .collect::<Result<Vec<_>, _>>()?;

        let total_keys_num = Self::checked_estimate_sum(&estimate_keys_num)?;

        Ok(RecentReject {
            shard_num,
            count_limit,
            ttl,
            db: RwLock::new(db),
            total_keys_num: AtomicU64::new(total_keys_num),
        })
    }

    pub fn put(&self, hash: &Byte32, reject: Reject) -> Result<(), AnyError> {
        let hash_slice = hash.as_slice();
        let shard = self.get_shard(hash_slice).to_string();
        let reject: ckb_jsonrpc_types::PoolTransactionReject = reject.into();
        let json_string = serde_json::to_string(&reject)?;
        // Hold the read lock across the DB write so that `shrink` cannot drop
        // the column family while we are writing to it.
        let db = self.db.read().map_err(|e| OtherError::new(e.to_string()))?;
        db.put(&shard, hash_slice, json_string)?;
        drop(db);

        let new_count = self.total_keys_num.fetch_add(1, Ordering::Relaxed) + 1;
        if new_count > self.count_limit
            && let Err(e) = self.shrink()
        {
            error!("failed to shrink recent_reject: {}", e);
        }
        Ok(())
    }

    pub fn get(&self, hash: &Byte32) -> Result<Option<String>, AnyError> {
        let slice = hash.as_slice();
        let shard = self.get_shard(slice).to_string();
        let db = self.db.read().map_err(|e| OtherError::new(e.to_string()))?;
        let ret = db.get_pinned(&shard, slice)?;
        Ok(ret.map(|bytes| unsafe { String::from_utf8_unchecked(bytes.to_vec()) }))
    }

    pub fn get_estimate_total_keys_num(&self) -> u64 {
        self.total_keys_num.load(Ordering::Relaxed)
    }

    fn checked_estimate_sum(estimate_keys_num: &[Option<u64>]) -> Result<u64, OtherError> {
        estimate_keys_num.iter().try_fold(0u64, |total, num| {
            let keys_num = num.unwrap_or(0);
            total.checked_add(keys_num).ok_or_else(|| {
                OtherError::new(format!(
                    "recent reject estimated keys count overflows: {} + {}",
                    total, keys_num
                ))
            })
        })
    }

    fn shrink(&self) -> Result<u64, AnyError> {
        // Exclusive write lock: blocks all concurrent put/get while we drop
        // and recreate a column family.  This is a very cold path (triggered
        // only when key count exceeds `count_limit`), so brief contention is
        // acceptable.
        let mut db = self.db.write().map_err(|e| OtherError::new(e.to_string()))?;

        let mut rng = thread_rng();
        let shard = rng.sample(Uniform::new(0, self.shard_num)).to_string();
        // Estimate the keys in this shard before dropping it, then decrement
        // the atomic counter by that amount.  Using a bounded subtraction
        // instead of `store(estimate_total_keys_num())` prevents the counter
        // from being overwritten by a stale estimate while other threads are
        // concurrently calling `put`.
        let dropped_estimate = db.estimate_num_keys_cf(&shard)?.unwrap_or(0);
        db.drop_cf(&shard)?;
        db.create_cf_with_ttl(&shard, self.ttl)?;
        drop(db);

        // Saturating decrement to avoid underflow if the estimate overshoots
        // the actual counter.
        loop {
            let current = self.total_keys_num.load(Ordering::Relaxed);
            let new = current.saturating_sub(dropped_estimate);
            if self
                .total_keys_num
                .compare_exchange_weak(current, new, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                return Ok(new);
            }
        }
    }

    fn get_shard(&self, hash: &[u8]) -> u32 {
        let mut low_u32 = [0u8; 4];
        low_u32.copy_from_slice(&hash[0..4]);
        u32::from_le_bytes(low_u32) % self.shard_num
    }
}
