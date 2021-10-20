use crate::error::Reject;
use ckb_db::DBWithTTL;
use ckb_error::AnyError;
use ckb_types::{packed::Byte32, prelude::*};
use rand::distributions::Uniform;
use rand::{thread_rng, Rng};
use std::path::Path;

const DEFAULT_SHARDS: u32 = 5;

#[derive(Debug)]
pub struct RecentReject {
    ttl: i32,
    shard_num: u32,
    pub(crate) count_limit: u64,
    pub(crate) total_keys_num: u64,
    pub(crate) db: DBWithTTL,
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
            .map(|cf| db.estimate_num_keys_cf(&cf))
            .collect::<Result<Vec<_>, _>>()?;

        let total_keys_num = estimate_keys_num.iter().map(|num| num.unwrap_or(0)).sum();

        Ok(RecentReject {
            shard_num,
            count_limit,
            ttl,
            db,
            total_keys_num,
        })
    }

    pub fn put(&mut self, hash: &Byte32, reject: Reject) -> Result<(), AnyError> {
        let hash_slice = hash.as_slice();
        let shard = self.get_shard(hash_slice).to_string();
        let reject: ckb_jsonrpc_types::PoolTransactionReject = reject.into();
        let json_string = serde_json::to_string(&reject)?;
        self.db.put(&shard, hash_slice, json_string)?;

        let total_keys_num = self.total_keys_num.checked_add(1);
        if total_keys_num > Some(self.count_limit) || total_keys_num.is_none() {
            self.shrink()?;
        } else {
            self.total_keys_num = total_keys_num.expect("checked cannot fail");
        }
        Ok(())
    }

    pub fn get(&self, hash: &Byte32) -> Result<Option<String>, AnyError> {
        let slice = hash.as_slice();
        let shard = self.get_shard(slice).to_string();
        let ret = self.db.get_pinned(&shard, slice)?;
        Ok(ret.map(|bytes| unsafe { String::from_utf8_unchecked(bytes.to_vec()) }))
    }

    fn shrink(&mut self) -> Result<u64, AnyError> {
        let mut rng = thread_rng();
        let shard = rng.sample(Uniform::new(0, self.shard_num)).to_string();
        self.db.drop_cf(&shard)?;
        self.db.create_cf_with_ttl(&shard, self.ttl)?;

        let estimate_keys_num = (0..self.shard_num)
            .map(|num| self.db.estimate_num_keys_cf(&num.to_string()))
            .collect::<Result<Vec<_>, _>>()?;

        let total_keys_num = estimate_keys_num.iter().map(|num| num.unwrap_or(0)).sum();
        self.total_keys_num = total_keys_num;
        Ok(total_keys_num)
    }

    fn get_shard(&self, hash: &[u8]) -> u32 {
        let mut low_u32 = [0u8; 4];
        low_u32.copy_from_slice(&hash[0..4]);
        u32::from_le_bytes(low_u32) % self.shard_num
    }
}
