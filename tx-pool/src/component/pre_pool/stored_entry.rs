use super::*;

/// The only value accepted by the primary ownership map.
///
/// `Entry` is a transition draft. Converting it into `StoredEntry` derives its
/// identity and exact resident charge and validates every entry-local shape
/// rule. There is deliberately no mutable dereference: a lifecycle change
/// must consume a clone into a draft and pass through `prepare` again.
#[derive(Clone, Debug)]
pub(super) struct StoredEntry {
    hash: Byte32,
    entry: Entry,
    charge_bytes: usize,
}

impl StoredEntry {
    pub(super) fn prepare(entry: Entry, limits: PrePoolLimits) -> Result<Self, PrePoolError> {
        let hash = crate::util::compact_packed(&entry.raw.tx.hash());
        if entry
            .dependencies
            .iter()
            .any(|key| key.parent_hash() == hash)
        {
            return Err(PrePoolError::SelfDependency(hash));
        }
        if entry.dependencies.len() > limits.max_dependencies_per_entry {
            return Err(PrePoolError::DependencyLimitExceeded);
        }
        if let EntryState::Wait(wait) = &entry.state
            && wait.observed.len() > limits.max_dependencies_per_entry
        {
            return Err(PrePoolError::DependencyLimitExceeded);
        }

        let mut memberships = 2usize;
        if entry.source.peer().is_some() {
            memberships = memberships
                .checked_add(2)
                .ok_or(PrePoolError::ResidencyChargeOverflow)?;
        }
        let parent_count = entry
            .dependencies
            .iter()
            .map(DependencyKey::parent_hash)
            .collect::<BTreeSet<_>>()
            .len();
        memberships = memberships
            .checked_add(entry.dependencies.len())
            .and_then(|value| value.checked_add(parent_count.checked_mul(2)?))
            .and_then(|value| value.checked_add(usize::from(entry.expires_at.is_some())))
            .ok_or(PrePoolError::ResidencyChargeOverflow)?;
        let current_state_memberships = match &entry.state {
            EntryState::ResolveLeased | EntryState::VerifyLeased { .. } => 0,
            EntryState::ResolveQueued { .. } => 3,
            EntryState::VerifyQueued { .. } => 4,
            EntryState::Wait(wait) => wait
                .observed
                .len()
                .checked_mul(4)
                .map(|value| value.max(3))
                .ok_or(PrePoolError::ResidencyChargeOverflow)?,
            EntryState::Ready { inputs, .. } => inputs
                .len()
                .checked_mul(2)
                .and_then(|value| value.checked_add(1))
                .ok_or(PrePoolError::ResidencyChargeOverflow)?,
        };
        let wait_reservation = entry
            .dependencies
            .len()
            .checked_mul(4)
            .map(|value| value.max(3))
            .ok_or(PrePoolError::ResidencyChargeOverflow)?;
        let state_memberships = current_state_memberships.max(wait_reservation);
        let charge_bytes = memberships
            .checked_add(state_memberships)
            .and_then(|value| value.checked_mul(limits.dependency_overhead))
            .and_then(|value| value.checked_add(limits.entry_overhead))
            .and_then(|value| value.checked_add(entry.payload_charge_bytes))
            .ok_or(PrePoolError::ResidencyChargeOverflow)?;

        Ok(Self {
            hash,
            entry,
            charge_bytes,
        })
    }

    pub(super) fn hash(&self) -> &Byte32 {
        &self.hash
    }

    pub(super) fn charge_bytes(&self) -> usize {
        self.charge_bytes
    }

    pub(super) fn into_draft(self) -> Entry {
        self.entry
    }
}

impl std::ops::Deref for StoredEntry {
    type Target = Entry;

    fn deref(&self) -> &Self::Target {
        &self.entry
    }
}
