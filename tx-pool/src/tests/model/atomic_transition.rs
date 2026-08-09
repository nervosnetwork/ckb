//! Sealed reference capabilities for one atomic authority commit.
//!
//! Clock reservation and required projection controls are pure Plan values.
//! They add no owner, task, lock or rollback state.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ModelAuthorityClocks {
    pub(crate) next_version: u128,
    pub(crate) next_arrival: u128,
    pub(crate) next_sequence: u128,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ClockDemand {
    version_count: u128,
    arrival_count: u128,
}

impl ClockDemand {
    pub(crate) fn new(
        version_count: usize,
        arrival_count: usize,
    ) -> Result<Self, ClockCommitError> {
        if arrival_count > version_count {
            return Err(ClockCommitError::ArrivalWithoutVersion);
        }
        Ok(Self {
            version_count: u128::try_from(version_count)
                .map_err(|_| ClockCommitError::VersionOverflow)?,
            arrival_count: u128::try_from(arrival_count)
                .map_err(|_| ClockCommitError::ArrivalOverflow)?,
        })
    }

    pub(crate) const fn version_count(self) -> u128 {
        self.version_count
    }

    pub(crate) const fn arrival_count(self) -> u128 {
        self.arrival_count
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClockCommitError {
    ArrivalWithoutVersion,
    VersionOverflow,
    ArrivalOverflow,
    SequenceOverflow,
    IndexOutOfBounds,
}

/// One nonempty authority Apply's complete clock reservation.
///
/// Zero owner identities is legal for effect, dependency-marker and other
/// projection-only commits. The Apply sequence still advances exactly once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ClockCommit {
    before: ModelAuthorityClocks,
    after: ModelAuthorityClocks,
    sequence: u128,
    version_count: u128,
    arrival_count: u128,
}

impl ClockCommit {
    pub(crate) fn reserve(
        before: ModelAuthorityClocks,
        demand: ClockDemand,
    ) -> Result<Self, ClockCommitError> {
        let after = ModelAuthorityClocks {
            next_version: before
                .next_version
                .checked_add(demand.version_count)
                .ok_or(ClockCommitError::VersionOverflow)?,
            next_arrival: before
                .next_arrival
                .checked_add(demand.arrival_count)
                .ok_or(ClockCommitError::ArrivalOverflow)?,
            next_sequence: before
                .next_sequence
                .checked_add(1)
                .ok_or(ClockCommitError::SequenceOverflow)?,
        };
        Ok(Self {
            before,
            after,
            sequence: before.next_sequence,
            version_count: demand.version_count,
            arrival_count: demand.arrival_count,
        })
    }

    pub(crate) const fn before(self) -> ModelAuthorityClocks {
        self.before
    }

    pub(crate) const fn after(self) -> ModelAuthorityClocks {
        self.after
    }

    pub(crate) const fn sequence(self) -> u128 {
        self.sequence
    }

    pub(crate) fn version(self, index: usize) -> Result<u128, ClockCommitError> {
        let index = u128::try_from(index).map_err(|_| ClockCommitError::IndexOutOfBounds)?;
        if index >= self.version_count {
            return Err(ClockCommitError::IndexOutOfBounds);
        }
        self.before
            .next_version
            .checked_add(index)
            .ok_or(ClockCommitError::VersionOverflow)
    }

    pub(crate) fn arrival(self, index: usize) -> Result<u128, ClockCommitError> {
        let index = u128::try_from(index).map_err(|_| ClockCommitError::IndexOutOfBounds)?;
        if index >= self.arrival_count {
            return Err(ClockCommitError::IndexOutOfBounds);
        }
        self.before
            .next_arrival
            .checked_add(index)
            .ok_or(ClockCommitError::ArrivalOverflow)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ModelDependencyControl(pub(crate) u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ModelEffectControl(pub(crate) u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TransitionControlDemand {
    None,
    Dependency,
    Effect,
    DependencyAndEffect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TransitionControlError {
    MissingDependency,
    UnexpectedDependency,
    MissingEffect,
    UnexpectedEffect,
}

/// Closed proof that one owner transition carries exactly its required
/// dependency/effect controls. No default branch can erase a required field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TransitionControlCommit {
    dependency: Option<ModelDependencyControl>,
    effect: Option<ModelEffectControl>,
}

impl TransitionControlCommit {
    pub(crate) fn seal(
        demand: TransitionControlDemand,
        dependency: Option<ModelDependencyControl>,
        effect: Option<ModelEffectControl>,
    ) -> Result<Self, TransitionControlError> {
        let needs_dependency = matches!(
            demand,
            TransitionControlDemand::Dependency | TransitionControlDemand::DependencyAndEffect
        );
        let needs_effect = matches!(
            demand,
            TransitionControlDemand::Effect | TransitionControlDemand::DependencyAndEffect
        );
        match (needs_dependency, dependency.is_some()) {
            (true, false) => return Err(TransitionControlError::MissingDependency),
            (false, true) => return Err(TransitionControlError::UnexpectedDependency),
            (true, true) | (false, false) => {}
        }
        match (needs_effect, effect.is_some()) {
            (true, false) => return Err(TransitionControlError::MissingEffect),
            (false, true) => return Err(TransitionControlError::UnexpectedEffect),
            (true, true) | (false, false) => {}
        }
        Ok(Self { dependency, effect })
    }

    pub(crate) const fn dependency(self) -> Option<ModelDependencyControl> {
        self.dependency
    }

    pub(crate) const fn effect(self) -> Option<ModelEffectControl> {
        self.effect
    }
}
