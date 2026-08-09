//! Stable-epoch reference relation for bounded dependency maintenance.
//!
//! External dependency events may publish a strictly newer epoch. While that
//! epoch is stable, every maintenance Apply consumes exactly one edge or one
//! completion marker from a finite obligation set.

use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ModelDependencyKey(pub(crate) u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ModelDependencyOwner(pub(crate) u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ModelDependencyCut(pub(crate) u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModelDirtyScope {
    ExistingWaiters,
    AllConsumers,
}

impl ModelDirtyScope {
    fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::ExistingWaiters, Self::ExistingWaiters) => Self::ExistingWaiters,
            (Self::ExistingWaiters, Self::AllConsumers)
            | (Self::AllConsumers, Self::ExistingWaiters | Self::AllConsumers) => {
                Self::AllConsumers
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ModelPendingDependencyEpoch {
    pub(crate) target: ModelDependencyCut,
    pub(crate) scope: ModelDirtyScope,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ModelDirtyDependencyEpoch {
    pub(crate) target: ModelDependencyCut,
    pub(crate) scope: ModelDirtyScope,
    pub(crate) cursor: Option<ModelDependencyOwner>,
    pub(crate) pending: Option<ModelPendingDependencyEpoch>,
}

impl ModelDirtyDependencyEpoch {
    pub(crate) fn new(
        target: ModelDependencyCut,
        scope: ModelDirtyScope,
        cursor: Option<ModelDependencyOwner>,
        pending: Option<ModelPendingDependencyEpoch>,
    ) -> Result<Self, DependencyProgressError> {
        if pending.is_some_and(|pending| pending.target <= target) {
            return Err(DependencyProgressError::NonMonotonicEpoch);
        }
        Ok(Self {
            target,
            scope,
            cursor,
            pending,
        })
    }
}

pub(crate) type ModelDependencyEdges = BTreeMap<ModelDependencyKey, BTreeSet<ModelDependencyOwner>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DependencyMaintenanceState {
    consumers: ModelDependencyEdges,
    waiters: ModelDependencyEdges,
    dirty: BTreeMap<ModelDependencyKey, ModelDirtyDependencyEpoch>,
    dirty_cursor: Option<ModelDependencyKey>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DependencyProgressError {
    DirtyWithoutConsumer(ModelDependencyKey),
    WaiterWithoutConsumer {
        key: ModelDependencyKey,
        owner: ModelDependencyOwner,
    },
    CursorWithoutDirty,
    NonMonotonicEpoch,
    Arithmetic,
    StaleStep,
    OwnerProgressWithoutOwner,
    NondecreasingStep,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DependencyMaintenanceStep {
    Advance {
        key: ModelDependencyKey,
        owner: ModelDependencyOwner,
    },
    Complete {
        key: ModelDependencyKey,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DependencyOwnerProgress {
    Unchanged,
    Requeued,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DependencyMaintenanceTransition {
    pub(crate) step: DependencyMaintenanceStep,
    pub(crate) before_rank: usize,
    pub(crate) after_rank: usize,
    pub(crate) after: DependencyMaintenanceState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DependencyEventDisposition {
    NoMaintenance,
    Activated,
    Superseded,
}

impl DependencyMaintenanceState {
    pub(crate) fn new(
        consumers: ModelDependencyEdges,
        waiters: ModelDependencyEdges,
        dirty: BTreeMap<ModelDependencyKey, ModelDirtyDependencyEpoch>,
        dirty_cursor: Option<ModelDependencyKey>,
    ) -> Result<Self, DependencyProgressError> {
        if dirty.is_empty() && dirty_cursor.is_some() {
            return Err(DependencyProgressError::CursorWithoutDirty);
        }
        for (key, owners) in &waiters {
            let consumers_for_key = consumers.get(key);
            if let Some(owner) = owners
                .iter()
                .find(|owner| consumers_for_key.is_none_or(|set| !set.contains(owner)))
            {
                return Err(DependencyProgressError::WaiterWithoutConsumer {
                    key: *key,
                    owner: *owner,
                });
            }
        }
        for key in dirty.keys() {
            if consumers.get(key).is_none_or(BTreeSet::is_empty) {
                return Err(DependencyProgressError::DirtyWithoutConsumer(*key));
            }
        }
        Ok(Self {
            consumers,
            waiters,
            dirty,
            dirty_cursor,
        })
    }

    pub(crate) fn dirty(&self) -> &BTreeMap<ModelDependencyKey, ModelDirtyDependencyEpoch> {
        &self.dirty
    }

    pub(crate) const fn dirty_cursor(&self) -> Option<ModelDependencyKey> {
        self.dirty_cursor
    }

    pub(crate) fn rank(&self) -> Result<usize, DependencyProgressError> {
        self.dirty.iter().try_fold(0usize, |total, (key, epoch)| {
            total
                .checked_add(self.epoch_rank(*key, epoch)?)
                .ok_or(DependencyProgressError::Arithmetic)
        })
    }

    fn epoch_rank(
        &self,
        key: ModelDependencyKey,
        epoch: &ModelDirtyDependencyEpoch,
    ) -> Result<usize, DependencyProgressError> {
        let current_edges = self.edges(key, epoch.scope);
        let remaining = current_edges
            .iter()
            .filter(|owner| epoch.cursor.is_none_or(|cursor| **owner > cursor))
            .count();
        let current = remaining
            .checked_add(1)
            .ok_or(DependencyProgressError::Arithmetic)?;
        let pending = match epoch.pending {
            Some(pending) => self
                .edges(key, pending.scope)
                .len()
                .checked_add(1)
                .ok_or(DependencyProgressError::Arithmetic)?,
            None => 0,
        };
        current
            .checked_add(pending)
            .ok_or(DependencyProgressError::Arithmetic)
    }

    fn edges(
        &self,
        key: ModelDependencyKey,
        scope: ModelDirtyScope,
    ) -> &BTreeSet<ModelDependencyOwner> {
        static EMPTY: BTreeSet<ModelDependencyOwner> = BTreeSet::new();
        match scope {
            ModelDirtyScope::ExistingWaiters => self.waiters.get(&key).unwrap_or(&EMPTY),
            ModelDirtyScope::AllConsumers => self.consumers.get(&key).unwrap_or(&EMPTY),
        }
    }

    fn next_dirty_key(&self) -> Option<ModelDependencyKey> {
        self.dirty_cursor
            .and_then(|cursor| {
                self.dirty
                    .range((
                        std::ops::Bound::Excluded(cursor),
                        std::ops::Bound::Unbounded,
                    ))
                    .next()
                    .map(|(key, _)| *key)
            })
            .or_else(|| self.dirty.first_key_value().map(|(key, _)| *key))
    }

    pub(crate) fn next_step(&self) -> Option<DependencyMaintenanceStep> {
        let key = self.next_dirty_key()?;
        let epoch = self.dirty.get(&key)?;
        let owner = self
            .edges(key, epoch.scope)
            .iter()
            .find(|owner| epoch.cursor.is_none_or(|cursor| **owner > cursor))
            .copied();
        Some(match owner {
            Some(owner) => DependencyMaintenanceStep::Advance { key, owner },
            None => DependencyMaintenanceStep::Complete { key },
        })
    }

    pub(crate) fn apply_next(
        &self,
    ) -> Result<Option<DependencyMaintenanceTransition>, DependencyProgressError> {
        self.apply_next_with_owner_progress(DependencyOwnerProgress::Unchanged)
    }

    pub(crate) fn apply_next_with_owner_progress(
        &self,
        owner_progress: DependencyOwnerProgress,
    ) -> Result<Option<DependencyMaintenanceTransition>, DependencyProgressError> {
        let Some(step) = self.next_step() else {
            return Ok(None);
        };
        let before_rank = self.rank()?;
        let mut after = self.clone();
        match step {
            DependencyMaintenanceStep::Advance { key, owner } => {
                if owner_progress == DependencyOwnerProgress::Requeued {
                    let remove_waiter_key = after.waiters.get_mut(&key).is_some_and(|waiters| {
                        waiters.remove(&owner);
                        waiters.is_empty()
                    });
                    if remove_waiter_key {
                        after.waiters.remove(&key);
                    }
                }
                let epoch = after
                    .dirty
                    .get_mut(&key)
                    .ok_or(DependencyProgressError::StaleStep)?;
                epoch.cursor = Some(owner);
                after.dirty_cursor = Some(key);
            }
            DependencyMaintenanceStep::Complete { key } => {
                if owner_progress != DependencyOwnerProgress::Unchanged {
                    return Err(DependencyProgressError::OwnerProgressWithoutOwner);
                }
                let epoch = after
                    .dirty
                    .remove(&key)
                    .ok_or(DependencyProgressError::StaleStep)?;
                if let Some(pending) = epoch.pending {
                    after.dirty.insert(
                        key,
                        ModelDirtyDependencyEpoch {
                            target: pending.target,
                            scope: pending.scope,
                            cursor: None,
                            pending: None,
                        },
                    );
                }
                after.dirty_cursor = (!after.dirty.is_empty()).then_some(key);
            }
        }
        let after_rank = after.rank()?;
        if after_rank >= before_rank {
            return Err(DependencyProgressError::NondecreasingStep);
        }
        Ok(Some(DependencyMaintenanceTransition {
            step,
            before_rank,
            after_rank,
            after,
        }))
    }

    pub(crate) fn publish_event(
        &mut self,
        key: ModelDependencyKey,
        target: ModelDependencyCut,
        scope: ModelDirtyScope,
    ) -> Result<DependencyEventDisposition, DependencyProgressError> {
        let latest = self
            .dirty
            .get(&key)
            .map(|epoch| epoch.pending.map_or(epoch.target, |pending| pending.target));
        if latest.is_some_and(|latest| target <= latest) {
            return Err(DependencyProgressError::NonMonotonicEpoch);
        }
        if let Some(epoch) = self.dirty.get_mut(&key) {
            epoch.pending = Some(match epoch.pending {
                Some(pending) => ModelPendingDependencyEpoch {
                    target,
                    scope: pending.scope.merge(scope),
                },
                None => ModelPendingDependencyEpoch { target, scope },
            });
            return Ok(DependencyEventDisposition::Superseded);
        }
        if self.edges(key, scope).is_empty() {
            return Ok(DependencyEventDisposition::NoMaintenance);
        }
        self.dirty.insert(
            key,
            ModelDirtyDependencyEpoch {
                target,
                scope,
                cursor: None,
                pending: None,
            },
        );
        Ok(DependencyEventDisposition::Activated)
    }
}
