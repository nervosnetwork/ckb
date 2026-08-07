use super::super::state::test_support::RejectionKind;
use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::authority) struct EffectSnapshot {
    queued: VecDeque<QueuedEffectRecord>,
    latest_generation_reset: Option<EffectRecord>,
    usage: EffectRegionUsage,
    closed: bool,
}

/// Test-only extraction of the committed stream at one authority stable cut.
/// It exposes production facts, not model-normalized values, so refinement
/// adapters must still perform their own independent semantic mapping.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::authority) struct EffectTraceBatch {
    pub(in crate::authority) sequence: ApplySequence,
    pub(in crate::authority) class: Option<EffectTraceClass>,
    pub(in crate::authority) processed_steps: usize,
    pub(in crate::authority) effects: Vec<CommittedEffect>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) enum EffectTraceClass {
    Remote,
    Trusted,
    Critical,
}

impl From<EffectClass> for EffectTraceClass {
    fn from(class: EffectClass) -> Self {
        match class {
            EffectClass::Remote => Self::Remote,
            EffectClass::Trusted => Self::Trusted,
            EffectClass::Critical => Self::Critical,
        }
    }
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

    pub(in crate::authority) fn with_remote_effects_per_batch_for_foundation(
        self,
        max_effects: usize,
    ) -> Self {
        Self {
            bounds: EffectBatchBounds::new(
                EffectBatchBound::new(max_effects, self.bounds.remote.max_bytes),
                self.bounds.trusted,
                self.bounds.critical,
            ),
            ..self
        }
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
        fn flatten_queued(
            records: impl IntoIterator<Item = QueuedEffectRecord>,
        ) -> Vec<(Option<EffectClass>, CommittedEffect)> {
            records
                .into_iter()
                .flat_map(|queued| {
                    queued
                        .record
                        .batch
                        .effects()
                        .iter()
                        .cloned()
                        .map(move |effect| (Some(queued.class), effect))
                        .collect::<Vec<_>>()
                })
                .collect()
        }

        fn flatten_reset(
            record: Option<EffectRecord>,
        ) -> Vec<(Option<EffectClass>, CommittedEffect)> {
            record.map_or_else(Vec::new, |record| {
                record
                    .batch
                    .effects()
                    .iter()
                    .cloned()
                    .map(|effect| (None, effect))
                    .collect()
            })
        }

        flatten_queued(self.queued.clone()) == flatten_queued(other.queued.clone())
            && flatten_reset(self.latest_generation_reset.clone())
                == flatten_reset(other.latest_generation_reset.clone())
            && self.closed == other.closed
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::authority) struct EffectObservation {
    pub(in crate::authority) queued: Vec<ApplySequence>,
    pub(in crate::authority) queued_processed_steps: Vec<usize>,
    pub(in crate::authority) latest_generation_reset: Option<ApplySequence>,
    pub(in crate::authority) generation_reset_processed_steps: Option<usize>,
    pub(in crate::authority) remote_usage: EffectUsage,
    pub(in crate::authority) ordinary_usage: EffectUsage,
    pub(in crate::authority) total_usage: EffectUsage,
    pub(in crate::authority) pending_recent_rejects: usize,
    pub(in crate::authority) closed: bool,
}

impl EffectReceipt {
    pub(in crate::authority) fn sequence(&self) -> ApplySequence {
        self.token.sequence
    }

    pub(in crate::authority) fn effects(&self) -> &[CommittedEffect] {
        self.batch.effects()
    }

    pub(in crate::authority) fn charge_bytes(&self) -> usize {
        self.batch.charge_bytes()
    }

    pub(in crate::authority) fn complete_for_foundation(mut self) -> CompletedEffectReceipt {
        self.processed = EffectProgress(self.batch.publication_steps());
        CompletedEffectReceipt {
            token: self.token,
            batch: self.batch,
        }
    }
}

impl EffectSettlement {
    pub(in crate::authority) fn claim_generation_reset_source_for_foundation(mut self) -> Self {
        self.token.source = EffectLeaseSource::GenerationReset;
        self
    }

    pub(in crate::authority) fn with_sequence_for_foundation(
        mut self,
        sequence: ApplySequence,
    ) -> Self {
        self.token.sequence = sequence;
        self
    }

    pub(in crate::authority) fn with_processed_steps_for_foundation(
        mut self,
        processed: usize,
    ) -> Self {
        self.processed = EffectProgress(processed);
        self
    }
}

impl EffectLog {
    pub(in crate::authority) fn for_foundation() -> Self {
        let limits = EffectLimits::for_foundation();
        Self {
            limits,
            queued: VecDeque::with_capacity(limits.regions.total.batches),
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
                .map(|queued| queued.record.sequence)
                .collect(),
            queued_processed_steps: self
                .queued
                .iter()
                .map(|queued| queued.record.processed.0)
                .collect(),
            latest_generation_reset: self
                .latest_generation_reset
                .as_ref()
                .map(|record| record.sequence),
            generation_reset_processed_steps: self
                .latest_generation_reset
                .as_ref()
                .map(|record| record.processed.0),
            remote_usage: self.usage.remote,
            ordinary_usage: self.usage.ordinary,
            total_usage: self.usage.total,
            pending_recent_rejects: self.pending_recent_rejects.len(),
            closed: self.closed,
        }
    }

    pub(in crate::authority) fn trace_batches(&self) -> Vec<EffectTraceBatch> {
        let mut batches = self
            .queued
            .iter()
            .map(|queued| EffectTraceBatch {
                sequence: queued.record.sequence,
                class: Some(queued.class.into()),
                processed_steps: queued.record.processed.0,
                effects: queued.record.batch.effects().to_vec(),
            })
            .collect::<Vec<_>>();
        if let Some(reset) = &self.latest_generation_reset {
            batches.push(EffectTraceBatch {
                sequence: reset.sequence,
                class: None,
                processed_steps: reset.processed.0,
                effects: reset.batch.effects().to_vec(),
            });
        }
        batches.sort_unstable_by_key(|batch| batch.sequence);
        batches
    }

    pub(in crate::authority) fn snapshot(&self) -> EffectSnapshot {
        EffectSnapshot {
            queued: self.queued.clone(),
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
            .try_fold(None, |previous, queued| {
                if previous.is_some_and(|previous| previous >= queued.record.sequence) {
                    None
                } else {
                    Some(Some(queued.record.sequence))
                }
            })
            .is_some();
        if !queued_ordered {
            return false;
        }
        let mut rebuilt = EffectRegionUsage::default();
        for queued in &self.queued {
            let Some(next) =
                rebuilt.checked_charge(queued.class, queued.record.batch.charge_bytes())
            else {
                return false;
            };
            rebuilt = next;
        }
        let queued_sequences_before_clock = self
            .queued
            .iter()
            .all(|queued| queued.record.sequence < next_sequence);
        let reset_sequence_before_clock = self
            .latest_generation_reset
            .as_ref()
            .is_none_or(|reset| reset.sequence < next_sequence);
        let queued_progress_incomplete = self
            .queued
            .iter()
            .all(|queued| queued.record.processed.is_pending(&queued.record.batch));
        let reset_progress_incomplete = self
            .latest_generation_reset
            .as_ref()
            .is_none_or(|reset| reset.processed.is_pending(&reset.batch));
        let pending_projection_complete =
            self.queued.iter().all(|queued| {
                queued
                    .record
                    .batch
                    .pending_recent_rejects()
                    .all(|(effect_index, rejection)| {
                        self.pending_recent_rejects
                            .get(&rejection.raw_hash())
                            .is_some_and(|pending| {
                                pending.sequence > queued.record.sequence
                                    || (pending.sequence == queued.record.sequence
                                        && pending.effect_index >= effect_index
                                        && Arc::ptr_eq(&pending.batch, &queued.record.batch))
                            })
                    })
            }) && self.pending_recent_rejects.iter().all(|(hash, pending)| {
                pending
                    .batch
                    .effects()
                    .get(pending.effect_index)
                    .and_then(CommittedEffect::recordable_rejection)
                    .is_some_and(|rejection| rejection.raw_hash() == *hash)
                    && self.queued.iter().any(|queued| {
                        queued.record.sequence == pending.sequence
                            && Arc::ptr_eq(&queued.record.batch, &pending.batch)
                    })
            });
        rebuilt == self.usage
            && self.usage_within_limits()
            && queued_sequences_before_clock
            && reset_sequence_before_clock
            && queued_progress_incomplete
            && reset_progress_incomplete
            && self.latest_generation_reset.as_ref().is_none_or(|reset| {
                reset.batch.charge_bytes() == 0
                    && Arc::ptr_eq(&reset.batch, &self.generation_reset_batch)
            })
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
