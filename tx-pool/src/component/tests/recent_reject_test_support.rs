use super::*;

impl RecentReject {
    pub(crate) fn drop_hash_shard_for_test(&self, hash: &Byte32) {
        let shard = self.get_shard(hash).to_string();
        block_offload(|| {
            self.db
                .write()
                .expect("recent-reject test lock")
                .drop_cf(&shard)
                .expect("drop recent-reject test shard");
        });
    }
}
