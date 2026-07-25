//! Bounded, sequence-ordered publication journal for stable-state effects.
//!
//! An owner reserves count/byte capacity before changing lifecycle or pool
//! state, commits the reservation at the mutation linearization point while
//! holding the corresponding authoritative lock, and opens that lock only
//! after the effect is queued. Residency remains charged while queued and
//! while the publisher is actively executing the effect.
use std::collections::{HashMap, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EffectOutboxLimits {
    pub(crate) max_batches: usize,
    pub(crate) max_bytes: usize,
}

#[cfg(test)]
#[path = "tests/effect_outbox_seam.rs"]
mod test_seam;

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

#[derive(Debug, PartialEq, Eq, Hash)]
pub(crate) struct EffectReservation {
    id: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReservationState {
    bytes: usize,
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
    PublisherBusy,
    NoActiveEffect,
    ActiveSequenceMismatch { expected: u64, actual: u64 },
    AccountingInvariant,
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
        })
    }

    pub(crate) fn usage(&self) -> EffectOutboxUsage {
        self.usage
    }

    pub(crate) fn reserve(&mut self, bytes: usize) -> Result<EffectReservation, EffectOutboxError> {
        // Every outstanding reservation can later commit. Preflight sequence
        // space for all of them plus this one, so sequence allocation at the
        // post-mutation commit point is infallible for a valid permit.
        let required = u64::try_from(self.reservations.len())
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
        self.reservations.insert(id, ReservationState { bytes });
        self.next_reservation_id = next_id;
        self.usage = EffectOutboxUsage {
            batches: next_batches,
            bytes: next_bytes,
        };
        Ok(EffectReservation { id })
    }

    /// Atomically shrink a conservative reservation, allocate the next FIFO
    /// sequence and enqueue the immutable effect at the authoritative state
    /// mutation boundary. There is no externally visible "bound but not
    /// queued" state: commit order is mutation order.
    pub(crate) fn commit_reserved(
        &mut self,
        reservation: &EffectReservation,
        bytes: usize,
        effect: E,
    ) -> Result<u64, EffectOutboxError> {
        let state = self
            .reservations
            .get(&reservation.id)
            .copied()
            .ok_or(EffectOutboxError::MissingReservation)?;
        if bytes > state.bytes {
            return Err(EffectOutboxError::AccountingInvariant);
        }
        let refunded = state
            .bytes
            .checked_sub(bytes)
            .ok_or(EffectOutboxError::AccountingInvariant)?;
        let next_usage_bytes = self
            .usage
            .bytes
            .checked_sub(refunded)
            .ok_or(EffectOutboxError::AccountingInvariant)?;
        let sequence = self.next_sequence;
        let next_sequence = sequence
            .checked_add(1)
            .ok_or(EffectOutboxError::SequenceExhausted)?;

        // Every fallible invariant check is complete. The queue and map were
        // preallocated to the batch limit at construction.
        self.reservations.remove(&reservation.id);
        self.queued.push_back(EffectEnvelope {
            sequence,
            bytes,
            effect,
        });
        self.usage.bytes = next_usage_bytes;
        self.next_sequence = next_sequence;
        Ok(sequence)
    }

    pub(crate) fn cancel(
        &mut self,
        reservation: &EffectReservation,
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
