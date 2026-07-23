//! Bounded, sequence-ordered publication journal for stable-state effects.
//!
//! An owner reserves count/byte capacity before changing lifecycle or pool
//! state, binds the reservation to the mutation order while holding the
//! corresponding authoritative lock, and enqueues the effect before opening
//! that lock. Residency remains charged while queued and while the publisher
//! is actively executing the effect.
use std::collections::{HashMap, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EffectOutboxLimits {
    pub(crate) max_batches: usize,
    pub(crate) max_bytes: usize,
}

impl EffectOutboxLimits {
    pub(crate) const fn new(max_batches: usize, max_bytes: usize) -> Self {
        Self {
            max_batches,
            max_bytes,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EffectOutboxUsage {
    pub(crate) batches: usize,
    pub(crate) bytes: usize,
}

impl EffectOutboxUsage {
    const fn empty() -> Self {
        Self {
            batches: 0,
            bytes: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct EffectReservation {
    id: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReservationState {
    bytes: usize,
    sequence: Option<u64>,
}

#[derive(Debug)]
struct EffectEnvelope<E> {
    sequence: u64,
    bytes: usize,
    effect: E,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EffectOutboxError {
    BatchLimitExceeded,
    ByteLimitExceeded,
    ReservationIdExhausted,
    AllocationFailed,
    SequenceExhausted,
    MissingReservation,
    AlreadyBound,
    UnboundReservation,
    EarlierBoundReservation,
    OutOfOrder { previous: u64, next: u64 },
    PublisherBusy,
    NoActiveEffect,
    ActiveSequenceMismatch { expected: u64, actual: u64 },
    AccountingInvariant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EffectOutboxAuditError {
    Usage,
    Limits,
    SequenceOrder,
    ActiveOrder,
}

#[derive(Debug)]
pub(crate) struct EffectOutbox<E> {
    limits: EffectOutboxLimits,
    usage: EffectOutboxUsage,
    reservations: HashMap<u64, ReservationState>,
    queued: VecDeque<EffectEnvelope<E>>,
    active: Option<EffectEnvelope<E>>,
    next_reservation_id: u64,
    next_sequence: u64,
    last_enqueued_sequence: Option<u64>,
}

impl<E> EffectOutbox<E> {
    pub(crate) fn new(limits: EffectOutboxLimits) -> Result<Self, EffectOutboxError> {
        let mut reservations = HashMap::new();
        reservations
            .try_reserve(limits.max_batches)
            .map_err(|_| EffectOutboxError::AllocationFailed)?;
        let mut queued = VecDeque::new();
        queued
            .try_reserve(limits.max_batches)
            .map_err(|_| EffectOutboxError::AllocationFailed)?;
        Ok(Self {
            limits,
            usage: EffectOutboxUsage::empty(),
            reservations,
            queued,
            active: None,
            next_reservation_id: 1,
            next_sequence: 1,
            last_enqueued_sequence: None,
        })
    }

    pub(crate) fn usage(&self) -> EffectOutboxUsage {
        self.usage
    }

    pub(crate) fn queued_len(&self) -> usize {
        self.queued.len()
    }

    pub(crate) fn reserve(&mut self, bytes: usize) -> Result<EffectReservation, EffectOutboxError> {
        let unbound = self
            .reservations
            .values()
            .filter(|state| state.sequence.is_none())
            .count();
        let required = u64::try_from(unbound)
            .ok()
            .and_then(|count| count.checked_add(1))
            .ok_or(EffectOutboxError::SequenceExhausted)?;
        self.next_sequence
            .checked_add(required)
            .ok_or(EffectOutboxError::SequenceExhausted)?;
        let next_batches = self
            .usage
            .batches
            .checked_add(1)
            .ok_or(EffectOutboxError::BatchLimitExceeded)?;
        if next_batches > self.limits.max_batches {
            return Err(EffectOutboxError::BatchLimitExceeded);
        }
        let next_bytes = self
            .usage
            .bytes
            .checked_add(bytes)
            .ok_or(EffectOutboxError::ByteLimitExceeded)?;
        if next_bytes > self.limits.max_bytes {
            return Err(EffectOutboxError::ByteLimitExceeded);
        }
        let id = self.next_reservation_id;
        let next_id = id
            .checked_add(1)
            .ok_or(EffectOutboxError::ReservationIdExhausted)?;
        self.reservations.insert(
            id,
            ReservationState {
                bytes,
                sequence: None,
            },
        );
        self.next_reservation_id = next_id;
        self.usage = EffectOutboxUsage {
            batches: next_batches,
            bytes: next_bytes,
        };
        Ok(EffectReservation { id })
    }

    /// Bind publication order while the caller holds the authoritative
    /// mutation lock. No state mutation should begin if this returns an error.
    pub(crate) fn bind_sequence(
        &mut self,
        reservation: EffectReservation,
    ) -> Result<u64, EffectOutboxError> {
        let state = self
            .reservations
            .get(&reservation.id)
            .ok_or(EffectOutboxError::MissingReservation)?;
        if state.sequence.is_some() {
            return Err(EffectOutboxError::AlreadyBound);
        }
        if self
            .reservations
            .values()
            .any(|pending| pending.sequence.is_some())
        {
            return Err(EffectOutboxError::EarlierBoundReservation);
        }
        let sequence = self.next_sequence;
        let next_sequence = sequence
            .checked_add(1)
            .ok_or(EffectOutboxError::SequenceExhausted)?;
        let state = self
            .reservations
            .get_mut(&reservation.id)
            .ok_or(EffectOutboxError::MissingReservation)?;
        state.sequence = Some(sequence);
        self.next_sequence = next_sequence;
        Ok(sequence)
    }

    /// Reduce a conservative pre-mutation reservation to the immutable
    /// batch's actual resident charge. Growth is forbidden: callers must
    /// prove capacity before changing authoritative state.
    pub(crate) fn shrink_reservation(
        &mut self,
        reservation: EffectReservation,
        bytes: usize,
    ) -> Result<(), EffectOutboxError> {
        let state = self
            .reservations
            .get_mut(&reservation.id)
            .ok_or(EffectOutboxError::MissingReservation)?;
        if bytes > state.bytes {
            return Err(EffectOutboxError::AccountingInvariant);
        }
        let refunded = state
            .bytes
            .checked_sub(bytes)
            .ok_or(EffectOutboxError::AccountingInvariant)?;
        state.bytes = bytes;
        self.usage.bytes = self
            .usage
            .bytes
            .checked_sub(refunded)
            .ok_or(EffectOutboxError::AccountingInvariant)?;
        Ok(())
    }

    /// Enqueue before releasing the authoritative mutation lock. A bound
    /// reservation with an earlier sequence must be cancelled/enqueued first;
    /// this catches accidental publication reordering at the call boundary.
    pub(crate) fn enqueue(
        &mut self,
        reservation: EffectReservation,
        effect: E,
    ) -> Result<u64, EffectOutboxError> {
        let state = self
            .reservations
            .get(&reservation.id)
            .copied()
            .ok_or(EffectOutboxError::MissingReservation)?;
        let sequence = state
            .sequence
            .ok_or(EffectOutboxError::UnboundReservation)?;
        if self
            .reservations
            .iter()
            .any(|(id, pending)| *id != reservation.id && pending.sequence.is_some())
        {
            return Err(EffectOutboxError::EarlierBoundReservation);
        }
        if let Some(previous) = self.last_enqueued_sequence
            && sequence <= previous
        {
            return Err(EffectOutboxError::OutOfOrder {
                previous,
                next: sequence,
            });
        }
        self.reservations.remove(&reservation.id);
        self.queued.push_back(EffectEnvelope {
            sequence,
            bytes: state.bytes,
            effect,
        });
        self.last_enqueued_sequence = Some(sequence);
        Ok(sequence)
    }

    pub(crate) fn cancel(
        &mut self,
        reservation: EffectReservation,
    ) -> Result<(), EffectOutboxError> {
        let state = self
            .reservations
            .get(&reservation.id)
            .copied()
            .ok_or(EffectOutboxError::MissingReservation)?;
        let next_usage = self.usage_after_release(state.bytes)?;
        self.reservations.remove(&reservation.id);
        self.usage = next_usage;
        Ok(())
    }

    /// Checkout keeps the batch/bytes charged until `complete_active`.
    pub(crate) fn checkout(&mut self) -> Result<Option<u64>, EffectOutboxError> {
        if self.active.is_some() {
            return Err(EffectOutboxError::PublisherBusy);
        }
        let Some(envelope) = self.queued.pop_front() else {
            return Ok(None);
        };
        let sequence = envelope.sequence;
        self.active = Some(envelope);
        Ok(Some(sequence))
    }

    pub(crate) fn active_effect(&self, sequence: u64) -> Result<&E, EffectOutboxError> {
        let active = self
            .active
            .as_ref()
            .ok_or(EffectOutboxError::NoActiveEffect)?;
        if active.sequence != sequence {
            return Err(EffectOutboxError::ActiveSequenceMismatch {
                expected: active.sequence,
                actual: sequence,
            });
        }
        Ok(&active.effect)
    }

    pub(crate) fn complete_active(&mut self, sequence: u64) -> Result<E, EffectOutboxError> {
        let active = self
            .active
            .as_ref()
            .ok_or(EffectOutboxError::NoActiveEffect)?;
        if active.sequence != sequence {
            return Err(EffectOutboxError::ActiveSequenceMismatch {
                expected: active.sequence,
                actual: sequence,
            });
        }
        let next_usage = self.usage_after_release(active.bytes)?;
        let active = self
            .active
            .take()
            .ok_or(EffectOutboxError::NoActiveEffect)?;
        self.usage = next_usage;
        Ok(active.effect)
    }

    /// Publisher panic/cancellation returns the active head without changing
    /// residency or allowing a later sequence to overtake it.
    pub(crate) fn retry_active(&mut self, sequence: u64) -> Result<(), EffectOutboxError> {
        let active = self
            .active
            .as_ref()
            .ok_or(EffectOutboxError::NoActiveEffect)?;
        if active.sequence != sequence {
            return Err(EffectOutboxError::ActiveSequenceMismatch {
                expected: active.sequence,
                actual: sequence,
            });
        }
        let active = self
            .active
            .take()
            .ok_or(EffectOutboxError::NoActiveEffect)?;
        self.queued.push_front(active);
        Ok(())
    }

    pub(crate) fn audit(&self) -> Result<(), EffectOutboxAuditError> {
        let mut batches = self.reservations.len();
        let mut bytes = self
            .reservations
            .values()
            .try_fold(0usize, |sum, state| sum.checked_add(state.bytes));
        batches = batches
            .checked_add(self.queued.len())
            .ok_or(EffectOutboxAuditError::Usage)?;
        if let Some(current) = bytes.as_mut() {
            for envelope in &self.queued {
                *current = current
                    .checked_add(envelope.bytes)
                    .ok_or(EffectOutboxAuditError::Usage)?;
            }
        }
        if let Some(active) = &self.active {
            batches = batches
                .checked_add(1)
                .ok_or(EffectOutboxAuditError::Usage)?;
            bytes = bytes.and_then(|value| value.checked_add(active.bytes));
        }
        if self.usage
            != (EffectOutboxUsage {
                batches,
                bytes: bytes.ok_or(EffectOutboxAuditError::Usage)?,
            })
        {
            return Err(EffectOutboxAuditError::Usage);
        }
        if self.usage.batches > self.limits.max_batches || self.usage.bytes > self.limits.max_bytes
        {
            return Err(EffectOutboxAuditError::Limits);
        }
        let mut previous = None;
        for envelope in &self.queued {
            if previous.is_some_and(|value| value >= envelope.sequence) {
                return Err(EffectOutboxAuditError::SequenceOrder);
            }
            previous = Some(envelope.sequence);
        }
        if let (Some(active), Some(front)) = (&self.active, self.queued.front())
            && active.sequence >= front.sequence
        {
            return Err(EffectOutboxAuditError::ActiveOrder);
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn set_next_sequence_for_test(&mut self, next: u64) {
        self.next_sequence = next;
    }

    fn usage_after_release(&self, bytes: usize) -> Result<EffectOutboxUsage, EffectOutboxError> {
        let next_batches = self
            .usage
            .batches
            .checked_sub(1)
            .ok_or(EffectOutboxError::AccountingInvariant)?;
        let next_bytes = self
            .usage
            .bytes
            .checked_sub(bytes)
            .ok_or(EffectOutboxError::AccountingInvariant)?;
        Ok(EffectOutboxUsage {
            batches: next_batches,
            bytes: next_bytes,
        })
    }
}
