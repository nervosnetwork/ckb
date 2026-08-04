use super::super::state::test_support::RejectionKind;
use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::authority) struct EffectSnapshot {
    queued: VecDeque<EffectEnvelope>,
    active: Option<EffectEnvelope>,
    latest_generation_reset: Option<EffectEnvelope>,
    usage: EffectRegionUsage,
    closed: bool,
}

impl EffectProgress {
    fn is_pending(self, batch: &EffectBatch) -> bool {
        self.0 < batch.publication_steps()
    }
}

impl EffectLimits {
    pub(in crate::authority) fn for_foundation() -> Self {
        Self {
            regions: EffectRegions::new(
                EffectCapacity::new(8, 64 * 1024),
                EffectCapacity::new(12, 128 * 1024),
                EffectCapacity::new(14, 192 * 1024),
            ),
            bounds: EffectBatchBounds::new(
                EffectBatchBound::new(16, 32 * 1024),
                EffectBatchBound::new(16, 64 * 1024),
                EffectBatchBound::new(64, 128 * 1024),
            ),
        }
    }

    pub(in crate::authority) fn max_batch_bytes_for_foundation(
        self,
        policy: EffectPolicy,
    ) -> usize {
        self.batch_bound(policy.class()).max_bytes
    }
}

impl CommittedRejection {
    pub(in crate::authority) fn for_foundation(
        tx: Arc<TransactionView>,
        audience: RejectionAudience,
        reason: RejectionKind,
    ) -> Self {
        Self::Validation {
            tx,
            audience,
            reason: reason.into(),
        }
    }
}

impl CommittedPeerCohortRevocation {
    pub(in crate::authority) const fn administrative_for_foundation(lease: PeerBanLease) -> Self {
        Self {
            lease,
            culprit: None,
        }
    }

    pub(in crate::authority) fn malformed_for_foundation(
        peer: PeerIndex,
        tx_hash: RawTxHash,
        reason: CommittedPublicReject,
    ) -> Option<Self> {
        Self::malformed(PeerBanLease::for_foundation(peer), tx_hash, reason)
    }
}

impl RejectionAudience {
    pub(in crate::authority) const fn ingress_peer(self) -> Option<PeerIndex> {
        self.ingress_peer
    }

    pub(in crate::authority) const fn foundation() -> Self {
        Self { ingress_peer: None }
    }
}

impl EffectPublication {
    pub(in crate::authority) fn new_for_foundation(
        policy: EffectPolicy,
        effects: Vec<CommittedEffect>,
        limits: EffectLimits,
    ) -> Result<Self, EffectBuildError> {
        Self::new(policy, effects, limits)
    }

    pub(in crate::authority) fn charge_bytes_for_foundation(&self) -> usize {
        self.batch.charge_bytes()
    }
}

impl EffectSnapshot {
    /// Compares the externally committed effect stream while deliberately
    /// ignoring journal batch boundaries and their accounting envelope.
    pub(in crate::authority) fn equivalent_stream(&self, other: &Self) -> bool {
        fn flatten(
            envelopes: impl IntoIterator<Item = EffectEnvelope>,
        ) -> Vec<(Option<EffectClass>, CommittedEffect)> {
            envelopes
                .into_iter()
                .flat_map(|envelope| {
                    envelope
                        .batch
                        .effects()
                        .iter()
                        .cloned()
                        .map(move |effect| (envelope.class, effect))
                        .collect::<Vec<_>>()
                })
                .collect()
        }

        let active = self.active.clone().into_iter();
        let other_active = other.active.clone().into_iter();
        let reset = self.latest_generation_reset.clone().into_iter();
        let other_reset = other.latest_generation_reset.clone().into_iter();

        flatten(self.queued.clone()) == flatten(other.queued.clone())
            && flatten(active) == flatten(other_active)
            && flatten(reset) == flatten(other_reset)
            && self.closed == other.closed
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::authority) struct EffectObservation {
    pub(in crate::authority) queued: Vec<ApplySequence>,
    pub(in crate::authority) active: Option<ApplySequence>,
    pub(in crate::authority) active_processed_steps: Option<usize>,
    pub(in crate::authority) latest_generation_reset: Option<ApplySequence>,
    pub(in crate::authority) remote_usage: EffectUsage,
    pub(in crate::authority) ordinary_usage: EffectUsage,
    pub(in crate::authority) total_usage: EffectUsage,
    pub(in crate::authority) pending_recent_rejects: usize,
    pub(in crate::authority) closed: bool,
}

impl EffectLease {
    pub(in crate::authority) fn sequence(&self) -> ApplySequence {
        self.token.sequence
    }

    pub(in crate::authority) fn effects(&self) -> &[CommittedEffect] {
        self.batch.effects()
    }

    pub(in crate::authority) fn charge_bytes(&self) -> usize {
        self.batch.charge_bytes()
    }

    pub(in crate::authority) fn complete_for_foundation(mut self) -> CompletedEffectLease {
        self.processed = EffectProgress(self.batch.publication_steps());
        CompletedEffectLease {
            token: self.token,
            batch: self.batch,
        }
    }
}

impl EffectLog {
    pub(in crate::authority) fn for_foundation() -> Self {
        let limits = EffectLimits::for_foundation();
        Self {
            limits,
            queued: VecDeque::with_capacity(limits.regions.total.batches),
            active: None,
            latest_generation_reset: None,
            pending_recent_rejects: HashMap::new(),
            usage: EffectRegionUsage::default(),
            closed: false,
            generation_reset_batch: EffectBatch::reset(),
        }
    }

    pub(in crate::authority) fn observation(&self) -> EffectObservation {
        EffectObservation {
            queued: self
                .queued
                .iter()
                .map(|envelope| envelope.sequence)
                .collect(),
            active: self.active.as_ref().map(|envelope| envelope.sequence),
            active_processed_steps: self.active.as_ref().map(|envelope| envelope.processed.0),
            latest_generation_reset: self
                .latest_generation_reset
                .as_ref()
                .map(|envelope| envelope.sequence),
            remote_usage: self.usage.remote,
            ordinary_usage: self.usage.ordinary,
            total_usage: self.usage.total,
            pending_recent_rejects: self.pending_recent_rejects.len(),
            closed: self.closed,
        }
    }

    pub(in crate::authority) fn snapshot(&self) -> EffectSnapshot {
        EffectSnapshot {
            queued: self.queued.clone(),
            active: self.active.clone(),
            latest_generation_reset: self.latest_generation_reset.clone(),
            usage: self.usage,
            closed: self.closed,
        }
    }

    pub(in crate::authority) fn semantically_consistent(
        &self,
        next_sequence: ApplySequence,
    ) -> bool {
        let queued_ordered = self
            .queued
            .iter()
            .try_fold(None, |previous, envelope| {
                if envelope.class.is_none()
                    || previous.is_some_and(|previous| previous >= envelope.sequence)
                {
                    None
                } else {
                    Some(Some(envelope.sequence))
                }
            })
            .is_some();
        if !queued_ordered {
            return false;
        }
        let mut rebuilt = EffectRegionUsage::default();
        for envelope in self.queued.iter().chain(self.active.iter()) {
            let Some(class) = envelope.class else {
                if self.active.as_ref() != Some(envelope) {
                    return false;
                }
                continue;
            };
            let Some(next) = rebuilt.checked_charge(class, envelope.batch.charge_bytes()) else {
                return false;
            };
            rebuilt = next;
        }
        let all_sequences_before_clock = self
            .queued
            .iter()
            .chain(self.active.iter())
            .chain(self.latest_generation_reset.iter())
            .all(|envelope| envelope.sequence < next_sequence);
        let all_progress_incomplete = self
            .queued
            .iter()
            .chain(self.active.iter())
            .chain(self.latest_generation_reset.iter())
            .all(|envelope| envelope.processed.is_pending(&envelope.batch));
        let active_precedes_pending = self.active.as_ref().is_none_or(|active| {
            self.queued
                .front()
                .is_none_or(|queued| active.sequence < queued.sequence)
                && self
                    .latest_generation_reset
                    .as_ref()
                    .is_none_or(|reset| active.sequence < reset.sequence)
        });
        let resident = self.queued.iter().chain(self.active.iter());
        let pending_projection_complete =
            resident.clone().all(|envelope| {
                envelope
                    .batch
                    .pending_recent_rejects()
                    .all(|(effect_index, rejection)| {
                        self.pending_recent_rejects
                            .get(&rejection.raw_hash())
                            .is_some_and(|pending| {
                                pending.sequence > envelope.sequence
                                    || (pending.sequence == envelope.sequence
                                        && pending.effect_index >= effect_index
                                        && Arc::ptr_eq(&pending.batch, &envelope.batch))
                            })
                    })
            }) && self.pending_recent_rejects.iter().all(|(hash, pending)| {
                pending
                    .batch
                    .effects()
                    .get(pending.effect_index)
                    .and_then(CommittedEffect::recordable_rejection)
                    .is_some_and(|rejection| rejection.raw_hash() == *hash)
                    && self
                        .queued
                        .iter()
                        .chain(self.active.iter())
                        .any(|envelope| {
                            envelope.sequence == pending.sequence
                                && Arc::ptr_eq(&envelope.batch, &pending.batch)
                        })
            });
        rebuilt == self.usage
            && self.usage_within_limits()
            && all_sequences_before_clock
            && all_progress_incomplete
            && active_precedes_pending
            && self
                .latest_generation_reset
                .as_ref()
                .is_none_or(|reset| reset.class.is_none() && reset.batch.charge_bytes() == 0)
            && pending_projection_complete
    }

    fn usage_within_limits(&self) -> bool {
        self.usage.remote.batches <= self.limits.regions.remote.batches
            && self.usage.remote.bytes <= self.limits.regions.remote.bytes
            && self.usage.ordinary.batches <= self.limits.regions.ordinary.batches
            && self.usage.ordinary.bytes <= self.limits.regions.ordinary.bytes
            && self.usage.total.batches <= self.limits.regions.total.batches
            && self.usage.total.bytes <= self.limits.regions.total.bytes
            && self.usage.remote.batches <= self.usage.ordinary.batches
            && self.usage.ordinary.batches <= self.usage.total.batches
            && self.usage.remote.bytes <= self.usage.ordinary.bytes
            && self.usage.ordinary.bytes <= self.usage.total.bytes
    }
}
