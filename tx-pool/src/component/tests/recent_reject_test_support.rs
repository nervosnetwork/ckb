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

#[test]
fn shrink_reconciles_a_missing_shard_without_discarding_remaining_records() {
    let tmp = tempfile::tempdir().unwrap();
    let recent = RecentReject::build(tmp.path(), 2, 2, -1).unwrap();
    let removed = Byte32::new([0; 32]);
    let retained = Byte32::new([1; 32]);
    assert_ne!(recent.get_shard(&removed), recent.get_shard(&retained));
    recent
        .put(&removed, Reject::Full("removed".to_owned()))
        .unwrap();
    recent
        .put(&retained, Reject::Full("retained".to_owned()))
        .unwrap();
    recent.drop_hash_shard_for_test(&removed);

    assert_eq!(recent.shrink().unwrap(), 1);
    assert_eq!(recent.get_estimate_total_keys_num(), 1);
    assert!(recent.get(&removed).unwrap().is_none());
    assert!(recent.get(&retained).unwrap().is_some());
}

#[test]
fn estimate_sum_checks_overflow_and_propagates_read_errors() {
    assert_eq!(
        RecentReject::checked_estimate_sum([Ok(None), Ok(Some(u64::MAX))]).unwrap(),
        u64::MAX
    );
    let overflow =
        RecentReject::checked_estimate_sum([Ok(Some(u64::MAX)), Ok(Some(1))]).unwrap_err();
    assert!(
        overflow
            .to_string()
            .contains("estimated keys count overflows")
    );

    let error = RecentReject::checked_estimate_sum([
        Ok(Some(1)),
        Err(OtherError::new("estimate unavailable").into()),
    ])
    .unwrap_err();
    assert!(error.to_string().contains("estimate unavailable"));
}
