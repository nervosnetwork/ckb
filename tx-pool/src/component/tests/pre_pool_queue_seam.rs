use super::*;

impl FairQueue {
    fn insert(&mut self, key: WorkKey) -> Result<(), PrePoolError> {
        let next_len = self
            .len
            .checked_add(1)
            .ok_or(PrePoolError::CounterExhausted)?;
        if self.contains(&key) {
            return Err(PrePoolError::ProjectionInconsistent(
                "queue already contains the inserted work key",
            ));
        }
        self.apply_insert(key);
        self.len = next_len;
        Ok(())
    }

    fn pop(&mut self, capability: WorkCapability) -> Result<Option<WorkKey>, PrePoolError> {
        let Some(key) = self.peek(capability).cloned() else {
            return Ok(None);
        };
        let next_turn = self.plan_checkout(&key, capability)?;
        let next_len = self
            .len
            .checked_sub(1)
            .ok_or(PrePoolError::ProjectionInconsistent(
                "queue length omits its runnable head",
            ))?;
        self.apply_checkout(&key, next_turn);
        self.len = next_len;
        Ok(Some(key))
    }

    pub(in crate::component::pre_pool) fn audit(&self) -> Result<(), String> {
        let mut expected_heads = BTreeSet::new();
        let mut expected_small_cycle_heads = BTreeSet::new();
        let mut count = 0usize;
        for (owner, queue) in &self.owners {
            if queue.work.is_empty() {
                return Err("fair queue retains an empty owner".to_string());
            }
            if queue
                .work
                .iter()
                .any(|key| WorkOwner::from(key.source) != *owner)
            {
                return Err("fair queue owner contains a foreign work key".to_string());
            }
            count = count
                .checked_add(queue.work.len())
                .ok_or_else(|| "fair queue length overflow".to_string())?;
            if let Some(head) = Self::head_for(*owner, queue, WorkCapability::Any) {
                expected_heads.insert(head);
            }
            if self.lane == WorkLane::Verify
                && let Some(head) = Self::head_for(*owner, queue, WorkCapability::SmallCycleOnly)
            {
                expected_small_cycle_heads.insert(head);
            }
        }
        if count != self.len {
            return Err(format!(
                "fair queue {:?} cached length drift: actual_members={count}, cached={}",
                self.lane, self.len
            ));
        }
        if expected_heads != self.heads {
            return Err("fair queue runnable-head projection drift".to_string());
        }
        if expected_small_cycle_heads != self.small_cycle_heads {
            return Err("fair queue small-cycle-head projection drift".to_string());
        }
        Ok(())
    }

    pub(in crate::component::pre_pool) fn work_keys(&self) -> BTreeSet<WorkKey> {
        self.owners
            .values()
            .flat_map(|queue| queue.work.iter().cloned())
            .collect()
    }
}

#[test]
fn large_owner_head_does_not_hide_its_small_cycle_work() {
    fn key(hash: u8, fee: u64, is_large_cycle: bool) -> WorkKey {
        WorkKey {
            hash: Byte32::new([hash; 32]),
            version: EntryVersion(u128::from(hash)),
            source: PrePoolSource::Remote(crate::component::pre_pool::RemoteSource::new(
                PeerIndex::from(1),
                0,
            )),
            arrival: Arrival(u128::from(hash)),
            schedule: VerifySchedule::new(fee, is_large_cycle),
            fee_ordered: true,
        }
    }

    let small = key(1, 1, false);
    let large = key(2, 2, true);
    let mut queue = FairQueue::new(WorkLane::Verify);
    queue.insert(small.clone()).unwrap();
    queue.insert(large.clone()).unwrap();

    assert_eq!(queue.peek(WorkCapability::Any), Some(&large));
    assert_eq!(queue.peek(WorkCapability::SmallCycleOnly), Some(&small));
    assert_eq!(
        queue.pop(WorkCapability::SmallCycleOnly).unwrap(),
        Some(small)
    );
    assert_eq!(queue.peek(WorkCapability::Any), Some(&large));
    queue.audit().unwrap();
}
