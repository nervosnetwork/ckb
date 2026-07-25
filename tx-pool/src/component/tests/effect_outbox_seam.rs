use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EffectOutboxAuditError {
    Usage,
    Limits,
    SequenceOrder,
    ActiveOrder,
}

impl<E> EffectOutbox<E> {
    pub(crate) fn queued_len(&self) -> usize {
        self.queued.len()
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

    pub(crate) fn set_next_sequence_for_test(&mut self, next: u64) {
        self.next_sequence = next;
    }
}
