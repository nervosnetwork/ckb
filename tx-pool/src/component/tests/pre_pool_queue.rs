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
            if queue.is_empty() {
                return Err("fair queue retains an empty owner".to_string());
            }
            if queue
                .small_work
                .iter()
                .chain(&queue.large_work)
                .any(|key| WorkOwner::from(key.source) != *owner)
            {
                return Err("fair queue owner contains a foreign work key".to_string());
            }
            if queue
                .small_work
                .iter()
                .any(|key| key.schedule.cycle_class != VerifyCycleClass::Small)
                || queue
                    .large_work
                    .iter()
                    .any(|key| key.schedule.cycle_class != VerifyCycleClass::Large)
            {
                return Err("fair queue cycle-class partition drift".to_string());
            }
            if self.lane != WorkLane::Verify && !queue.large_work.is_empty() {
                return Err("non-verify queue retains large-cycle work".to_string());
            }
            let owner_len = queue
                .small_work
                .len()
                .checked_add(queue.large_work.len())
                .ok_or_else(|| "fair queue owner length overflow".to_string())?;
            count = count
                .checked_add(owner_len)
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
            .flat_map(|queue| queue.small_work.iter().chain(&queue.large_work).cloned())
            .collect()
    }
}

#[test]
fn large_owner_head_does_not_hide_its_small_cycle_work() {
    fn key(hash: u8, fee: u64, is_large_cycle: bool) -> WorkKey {
        WorkKey {
            hash: Byte32::new([hash; 32]),
            revision: EntryRevision(u128::from(hash)),
            source: PrePoolSource::Remote(crate::component::pre_pool::RemoteSource::new(
                PeerIndex::from(1),
                0,
            )),
            arrival: Arrival(u128::from(hash)),
            schedule: VerifySchedule::new(
                fee,
                if is_large_cycle {
                    VerifyCycleClass::Large
                } else {
                    VerifyCycleClass::Small
                },
            ),
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

#[test]
fn large_cycle_population_is_partitioned_from_small_head() {
    fn key(hash: u16, fee: u64, cycle_class: VerifyCycleClass) -> WorkKey {
        let mut raw = [0u8; 32];
        raw[..2].copy_from_slice(&hash.to_le_bytes());
        WorkKey {
            hash: Byte32::new(raw),
            revision: EntryRevision(u128::from(hash)),
            source: PrePoolSource::Remote(crate::component::pre_pool::RemoteSource::new(
                PeerIndex::from(1),
                0,
            )),
            arrival: Arrival(u128::from(hash)),
            schedule: VerifySchedule::new(fee, cycle_class),
            fee_ordered: true,
        }
    }

    let mut queue = FairQueue::new(WorkLane::Verify);
    for hash in 1..=4_096 {
        queue
            .insert(key(hash, u64::from(hash), VerifyCycleClass::Large))
            .unwrap();
    }
    let small = key(0, u64::MAX, VerifyCycleClass::Small);
    queue.insert(small.clone()).unwrap();

    let owner = queue
        .owners
        .get(&WorkOwner::Remote(PeerIndex::from(1)))
        .unwrap();
    assert_eq!(owner.small_work.len(), 1);
    assert_eq!(owner.large_work.len(), 4_096);
    assert_eq!(queue.peek(WorkCapability::SmallCycleOnly), Some(&small));
    assert_eq!(queue.peek(WorkCapability::Any), Some(&small));
    queue.audit().unwrap();
}
