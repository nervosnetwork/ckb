use super::state::{AcceptedStatus, ApplySequence, RawTxHash, RejectionKind};
use ckb_network::PeerIndex;
use ckb_types::core::TransactionView;
use std::{collections::VecDeque, num::NonZeroUsize, sync::Arc};

const EFFECT_ENVELOPE_BYTES: usize = 128;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct EffectCapacity {
    pub(super) batches: usize,
    pub(super) bytes: usize,
}

impl EffectCapacity {
    pub(super) const fn new(batches: usize, bytes: usize) -> Self {
        Self { batches, bytes }
    }

    fn checked_add(self, other: Self) -> Option<Self> {
        Some(Self {
            batches: self.batches.checked_add(other.batches)?,
            bytes: self.bytes.checked_add(other.bytes)?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct EffectBatchBounds {
    max_effects: usize,
    remote_bytes: usize,
    trusted_bytes: usize,
    critical_bytes: usize,
}

impl EffectBatchBounds {
    pub(super) const fn new(
        max_effects: usize,
        remote_bytes: usize,
        trusted_bytes: usize,
        critical_bytes: usize,
    ) -> Self {
        Self {
            max_effects,
            remote_bytes,
            trusted_bytes,
            critical_bytes,
        }
    }

    fn bytes_for(self, class: EffectClass) -> usize {
        match class {
            EffectClass::Remote => self.remote_bytes,
            EffectClass::Trusted => self.trusted_bytes,
            EffectClass::Critical => self.critical_bytes,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EffectConfigError {
    EmptyRemoteRegion,
    EmptyBatchBound,
    Arithmetic,
    IndivisibleBatch,
    Allocation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct EffectLimits {
    regions: EffectRegions,
    bounds: EffectBatchBounds,
}

impl EffectLimits {
    pub(super) fn partitioned(
        remote: EffectCapacity,
        trusted_headroom: EffectCapacity,
        critical_headroom: EffectCapacity,
        bounds: EffectBatchBounds,
    ) -> Result<Self, EffectConfigError> {
        if remote.batches == 0 || remote.bytes == 0 {
            return Err(EffectConfigError::EmptyRemoteRegion);
        }
        if bounds.max_effects == 0
            || bounds.remote_bytes == 0
            || bounds.trusted_bytes == 0
            || bounds.critical_bytes == 0
        {
            return Err(EffectConfigError::EmptyBatchBound);
        }
        if EFFECT_ENVELOPE_BYTES > bounds.remote_bytes
            || EFFECT_ENVELOPE_BYTES > bounds.trusted_bytes
            || EFFECT_ENVELOPE_BYTES > bounds.critical_bytes
        {
            return Err(EffectConfigError::IndivisibleBatch);
        }
        let ordinary = remote
            .checked_add(trusted_headroom)
            .ok_or(EffectConfigError::Arithmetic)?;
        let total = ordinary
            .checked_add(critical_headroom)
            .ok_or(EffectConfigError::Arithmetic)?;
        if bounds.remote_bytes > remote.bytes
            || bounds.trusted_bytes > ordinary.bytes
            || bounds.critical_bytes > total.bytes
        {
            return Err(EffectConfigError::IndivisibleBatch);
        }
        Ok(Self {
            regions: EffectRegions::new(remote, ordinary, total),
            bounds,
        })
    }

    #[cfg(test)]
    fn for_foundation() -> Self {
        Self {
            regions: EffectRegions::new(
                EffectCapacity::new(8, 64 * 1024),
                EffectCapacity::new(12, 128 * 1024),
                EffectCapacity::new(14, 192 * 1024),
            ),
            bounds: EffectBatchBounds::new(16, 32 * 1024, 64 * 1024, 128 * 1024),
        }
    }

    fn max_batch_bytes(self, class: EffectClass) -> usize {
        self.bounds.bytes_for(class)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EffectClass {
    Remote,
    Trusted,
    Critical,
}

/// Capacity trust and overflow semantics are one closed policy. In
/// particular, non-rebuildable critical detail cannot accidentally inherit
/// the generation-reset fallback merely because it uses critical headroom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EffectPolicy {
    Remote,
    Trusted,
    CriticalDetail,
    CriticalRebuildable,
}

impl EffectPolicy {
    const fn class(self) -> EffectClass {
        match self {
            Self::Remote => EffectClass::Remote,
            Self::Trusted => EffectClass::Trusted,
            Self::CriticalDetail | Self::CriticalRebuildable => EffectClass::Critical,
        }
    }

    const fn can_reset(self) -> bool {
        match self {
            Self::Remote | Self::Trusted | Self::CriticalDetail => false,
            Self::CriticalRebuildable => true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum CommittedEffect {
    Accepted {
        tx: Arc<TransactionView>,
        status: AcceptedStatus,
    },
    Rejected {
        tx: Arc<TransactionView>,
        reason: RejectionKind,
    },
    /// The transaction became canonical while it still had a local owner.
    /// This clears pending relay/callback projections without manufacturing a
    /// pool status or a rejection record.
    ChainCommitted {
        tx: Arc<TransactionView>,
    },
    /// Administrative ingress revocation clears only the relayer's pending
    /// projection. It is not a transaction rejection and must not populate a
    /// raw-hash negative cache, so another peer may provide the same tx again.
    PeerRevoked {
        tx_hash: RawTxHash,
        peer: PeerIndex,
    },
    /// A remote residency lease elapsed before Accepted ownership. Expiry has
    /// the same refetch semantics as peer revocation, but remains remote
    /// capacity work and does not imply hostile-peer policy.
    RemoteExpired {
        tx_hash: RawTxHash,
        peer: PeerIndex,
    },
    GenerationReset,
}

impl CommittedEffect {
    fn charge_bytes(&self) -> Option<usize> {
        match self {
            Self::Accepted { tx, .. } | Self::Rejected { tx, .. } | Self::ChainCommitted { tx } => {
                EFFECT_ENVELOPE_BYTES.checked_add(tx.data().total_size())
            }
            Self::PeerRevoked { .. } | Self::RemoteExpired { .. } => Some(EFFECT_ENVELOPE_BYTES),
            Self::GenerationReset => Some(0),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EffectBuildError {
    Empty,
    TooMany,
    TooLarge,
    Arithmetic,
    ReservedReset,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct EffectBatch {
    effects: Box<[CommittedEffect]>,
    charge_bytes: usize,
}

impl EffectBatch {
    fn build(
        effects: Vec<CommittedEffect>,
        class: EffectClass,
        limits: EffectLimits,
    ) -> Result<Arc<Self>, EffectBuildError> {
        if effects.is_empty() {
            return Err(EffectBuildError::Empty);
        }
        if effects.len() > limits.bounds.max_effects {
            return Err(EffectBuildError::TooMany);
        }
        if effects
            .iter()
            .any(|effect| matches!(effect, CommittedEffect::GenerationReset))
        {
            return Err(EffectBuildError::ReservedReset);
        }
        let charge_bytes = effects.iter().try_fold(0usize, |total, effect| {
            total.checked_add(effect.charge_bytes()?)
        });
        let charge_bytes = charge_bytes.ok_or(EffectBuildError::Arithmetic)?;
        if charge_bytes > limits.max_batch_bytes(class) {
            return Err(EffectBuildError::TooLarge);
        }
        Ok(Arc::new(Self {
            effects: effects.into_boxed_slice(),
            charge_bytes,
        }))
    }

    fn reset() -> Arc<Self> {
        Arc::new(Self {
            effects: Box::new([CommittedEffect::GenerationReset]),
            charge_bytes: 0,
        })
    }

    pub(super) fn effects(&self) -> &[CommittedEffect] {
        &self.effects
    }

    pub(super) fn charge_bytes(&self) -> usize {
        self.charge_bytes
    }
}

#[derive(Debug)]
pub(super) struct EffectPublication {
    policy: EffectPolicy,
    batch: Arc<EffectBatch>,
}

impl EffectPublication {
    fn new(
        policy: EffectPolicy,
        effects: Vec<CommittedEffect>,
        limits: EffectLimits,
    ) -> Result<Self, EffectBuildError> {
        Ok(Self {
            policy,
            batch: EffectBatch::build(effects, policy.class(), limits)?,
        })
    }
}

/// A non-empty prefix proven to fit the remote effect region's indivisible
/// batch shape. The selected count is carried with the publication so the
/// authority transition cannot remove more owners than the journal can
/// describe.
pub(super) struct RemoteEffectPrefix {
    publication: EffectPublication,
    selected: NonZeroUsize,
}

impl RemoteEffectPrefix {
    pub(super) fn into_parts(self) -> (EffectPublication, NonZeroUsize) {
        (self.publication, self.selected)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct EffectUsage {
    pub(super) batches: usize,
    pub(super) bytes: usize,
}

impl EffectUsage {
    fn checked_charge(self, bytes: usize) -> Option<Self> {
        Some(Self {
            batches: self.batches.checked_add(1)?,
            bytes: self.bytes.checked_add(bytes)?,
        })
    }

    fn checked_release(self, bytes: usize) -> Option<Self> {
        Some(Self {
            batches: self.batches.checked_sub(1)?,
            bytes: self.bytes.checked_sub(bytes)?,
        })
    }

    fn fits(self, bytes: usize, limit: EffectCapacity) -> bool {
        self.batches
            .checked_add(1)
            .is_some_and(|batches| batches <= limit.batches)
            && self
                .bytes
                .checked_add(bytes)
                .is_some_and(|total| total <= limit.bytes)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct EffectRegions {
    remote: EffectCapacity,
    ordinary: EffectCapacity,
    total: EffectCapacity,
}

impl EffectRegions {
    const fn new(remote: EffectCapacity, ordinary: EffectCapacity, total: EffectCapacity) -> Self {
        Self {
            remote,
            ordinary,
            total,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct EffectRegionUsage {
    remote: EffectUsage,
    ordinary: EffectUsage,
    total: EffectUsage,
}

impl EffectRegionUsage {
    fn fits(self, limits: EffectRegions, class: EffectClass, bytes: usize) -> bool {
        match class {
            EffectClass::Remote => {
                self.remote.fits(bytes, limits.remote)
                    && self.ordinary.fits(bytes, limits.ordinary)
                    && self.total.fits(bytes, limits.total)
            }
            EffectClass::Trusted => {
                self.ordinary.fits(bytes, limits.ordinary) && self.total.fits(bytes, limits.total)
            }
            EffectClass::Critical => self.total.fits(bytes, limits.total),
        }
    }

    fn checked_charge(self, class: EffectClass, bytes: usize) -> Option<Self> {
        match class {
            EffectClass::Remote => Some(Self {
                remote: self.remote.checked_charge(bytes)?,
                ordinary: self.ordinary.checked_charge(bytes)?,
                total: self.total.checked_charge(bytes)?,
            }),
            EffectClass::Trusted => Some(Self {
                remote: self.remote,
                ordinary: self.ordinary.checked_charge(bytes)?,
                total: self.total.checked_charge(bytes)?,
            }),
            EffectClass::Critical => Some(Self {
                remote: self.remote,
                ordinary: self.ordinary,
                total: self.total.checked_charge(bytes)?,
            }),
        }
    }

    fn checked_release(self, class: EffectClass, bytes: usize) -> Option<Self> {
        match class {
            EffectClass::Remote => Some(Self {
                remote: self.remote.checked_release(bytes)?,
                ordinary: self.ordinary.checked_release(bytes)?,
                total: self.total.checked_release(bytes)?,
            }),
            EffectClass::Trusted => Some(Self {
                remote: self.remote,
                ordinary: self.ordinary.checked_release(bytes)?,
                total: self.total.checked_release(bytes)?,
            }),
            EffectClass::Critical => Some(Self {
                remote: self.remote,
                ordinary: self.ordinary,
                total: self.total.checked_release(bytes)?,
            }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct EffectEnvelope {
    sequence: ApplySequence,
    class: Option<EffectClass>,
    batch: Arc<EffectBatch>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct EffectSnapshot {
    queued: VecDeque<EffectEnvelope>,
    active: Option<EffectEnvelope>,
    latest_generation_reset: Option<EffectEnvelope>,
    usage: EffectRegionUsage,
    closed: bool,
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct EffectObservation {
    pub(super) queued: Vec<ApplySequence>,
    pub(super) active: Option<ApplySequence>,
    pub(super) latest_generation_reset: Option<ApplySequence>,
    pub(super) remote_usage: EffectUsage,
    pub(super) ordinary_usage: EffectUsage,
    pub(super) total_usage: EffectUsage,
    pub(super) closed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EffectError {
    Full,
    Closed,
    StaleLease,
    Projection,
}

struct AppendPlan {
    envelope: EffectEnvelope,
    usage: EffectRegionUsage,
}

#[derive(Clone, Copy)]
enum CheckoutSource {
    Queued,
    GenerationReset,
}

struct CheckoutPlan {
    source: CheckoutSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EffectDisposition {
    Published,
    CircuitDisposed,
    Retain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EffectToken {
    sequence: ApplySequence,
}

#[derive(Debug)]
#[must_use = "effect I/O must return its exact authority settlement"]
pub(super) struct EffectLease {
    token: EffectToken,
    batch: Arc<EffectBatch>,
}

impl EffectLease {
    pub(super) fn sequence(&self) -> ApplySequence {
        self.token.sequence
    }

    pub(super) fn effects(&self) -> &[CommittedEffect] {
        self.batch.effects()
    }

    pub(super) fn charge_bytes(&self) -> usize {
        self.batch.charge_bytes()
    }

    pub(super) fn published(self) -> EffectSettlement {
        EffectSettlement {
            token: self.token,
            batch: self.batch,
            disposition: EffectDisposition::Published,
        }
    }

    pub(super) fn circuit_disposed(self) -> EffectSettlement {
        EffectSettlement {
            token: self.token,
            batch: self.batch,
            disposition: EffectDisposition::CircuitDisposed,
        }
    }

    pub(super) fn retain(self) -> EffectSettlement {
        EffectSettlement {
            token: self.token,
            batch: self.batch,
            disposition: EffectDisposition::Retain,
        }
    }
}

#[derive(Debug)]
#[must_use = "effect settlement must be applied or discarded as stale"]
pub(super) struct EffectSettlement {
    token: EffectToken,
    batch: Arc<EffectBatch>,
    disposition: EffectDisposition,
}

struct SettlementPlan {
    disposition: EffectDisposition,
    after_usage: EffectRegionUsage,
}

struct ResetPlan {
    envelope: EffectEnvelope,
}

#[derive(Default)]
enum EffectMutation {
    #[default]
    None,
    Append(AppendPlan),
    Checkout(CheckoutPlan),
    Settle(SettlementPlan),
    Reset(ResetPlan),
    Close,
}

#[derive(Default)]
pub(super) struct EffectDelta(EffectMutation);

#[derive(Debug)]
pub(super) struct EffectLog {
    limits: EffectLimits,
    queued: VecDeque<EffectEnvelope>,
    active: Option<EffectEnvelope>,
    latest_generation_reset: Option<EffectEnvelope>,
    usage: EffectRegionUsage,
    closed: bool,
    generation_reset_batch: Arc<EffectBatch>,
}

impl EffectLog {
    pub(super) fn new(limits: EffectLimits) -> Result<Self, EffectConfigError> {
        let mut queued = VecDeque::new();
        queued
            .try_reserve(limits.regions.total.batches)
            .map_err(|_| EffectConfigError::Allocation)?;
        Ok(Self {
            limits,
            queued,
            active: None,
            latest_generation_reset: None,
            usage: EffectRegionUsage::default(),
            closed: false,
            generation_reset_batch: EffectBatch::reset(),
        })
    }

    #[cfg(test)]
    pub(super) fn for_foundation() -> Self {
        let limits = EffectLimits::for_foundation();
        Self {
            limits,
            queued: VecDeque::with_capacity(limits.regions.total.batches),
            active: None,
            latest_generation_reset: None,
            usage: EffectRegionUsage::default(),
            closed: false,
            generation_reset_batch: EffectBatch::reset(),
        }
    }

    pub(super) fn ensure_open(&self) -> Result<(), EffectError> {
        if self.closed {
            Err(EffectError::Closed)
        } else {
            Ok(())
        }
    }

    pub(super) fn build_publication(
        &self,
        policy: EffectPolicy,
        effects: Vec<CommittedEffect>,
    ) -> Result<EffectPublication, EffectBuildError> {
        EffectPublication::new(policy, effects, self.limits)
    }

    /// Select the largest leading remote cleanup cohort that fits one effect
    /// batch. The caller supplies deadline order; this method preserves that
    /// order and never turns attacker-originated expiry into trusted or
    /// critical journal work.
    pub(super) fn build_remote_prefix(
        &self,
        mut effects: Vec<CommittedEffect>,
    ) -> Result<Option<RemoteEffectPrefix>, EffectBuildError> {
        let mut selected = 0usize;
        let mut bytes = 0usize;
        for effect in &effects {
            if selected == self.limits.bounds.max_effects {
                break;
            }
            let effect_bytes = effect.charge_bytes().ok_or(EffectBuildError::Arithmetic)?;
            let next_bytes = bytes
                .checked_add(effect_bytes)
                .ok_or(EffectBuildError::Arithmetic)?;
            if next_bytes > self.limits.bounds.remote_bytes {
                if selected == 0 {
                    return Err(EffectBuildError::TooLarge);
                }
                break;
            }
            bytes = next_bytes;
            selected = selected
                .checked_add(1)
                .ok_or(EffectBuildError::Arithmetic)?;
        }
        let Some(selected) = NonZeroUsize::new(selected) else {
            return Ok(None);
        };
        effects.truncate(selected.get());
        let publication = EffectPublication::new(EffectPolicy::Remote, effects, self.limits)?;
        Ok(Some(RemoteEffectPrefix {
            publication,
            selected,
        }))
    }

    pub(super) fn snapshot(&self) -> EffectSnapshot {
        EffectSnapshot {
            queued: self.queued.clone(),
            active: self.active.clone(),
            latest_generation_reset: self.latest_generation_reset.clone(),
            usage: self.usage,
            closed: self.closed,
        }
    }

    pub(super) fn plan_publication(
        &self,
        publication: &EffectPublication,
        sequence: ApplySequence,
    ) -> Result<EffectDelta, EffectError> {
        self.ensure_open()?;
        self.validate_new_sequence(sequence)?;
        let class = publication.policy.class();
        let bytes = publication.batch.charge_bytes();
        if publication.batch.effects().len() > self.limits.bounds.max_effects
            || bytes > self.limits.max_batch_bytes(class)
        {
            return Err(EffectError::Projection);
        }
        if self.usage.fits(self.limits.regions, class, bytes) {
            let usage = self
                .usage
                .checked_charge(class, bytes)
                .ok_or(EffectError::Projection)?;
            return Ok(EffectDelta(EffectMutation::Append(AppendPlan {
                envelope: EffectEnvelope {
                    sequence,
                    class: Some(class),
                    batch: Arc::clone(&publication.batch),
                },
                usage,
            })));
        }
        if publication.policy.can_reset() {
            return Ok(self.reset_delta(sequence));
        }
        Err(EffectError::Full)
    }

    pub(super) fn plan_generation_reset(
        &self,
        sequence: ApplySequence,
    ) -> Result<EffectDelta, EffectError> {
        self.ensure_open()?;
        self.validate_new_sequence(sequence)?;
        Ok(self.reset_delta(sequence))
    }

    /// Publish rebuildable critical detail or collapse it to the same
    /// constant-size generation reset when either the batch shape or current
    /// journal capacity cannot preserve every item. This is the fail-open
    /// cleanup path used by administrative owner revocation: state removal
    /// must not wait for ordinary effect capacity, while consumers still get
    /// an authoritative reconciliation signal.
    pub(super) fn plan_critical_rebuildable(
        &self,
        effects: Vec<CommittedEffect>,
        sequence: ApplySequence,
    ) -> Result<EffectDelta, EffectError> {
        self.ensure_open()?;
        self.validate_new_sequence(sequence)?;
        let publication =
            match EffectPublication::new(EffectPolicy::CriticalRebuildable, effects, self.limits) {
                Ok(publication) => publication,
                Err(
                    EffectBuildError::TooMany
                    | EffectBuildError::TooLarge
                    | EffectBuildError::Arithmetic,
                ) => return Ok(self.reset_delta(sequence)),
                Err(EffectBuildError::Empty | EffectBuildError::ReservedReset) => {
                    return Err(EffectError::Projection);
                }
            };
        self.plan_publication(&publication, sequence)
    }

    fn reset_delta(&self, sequence: ApplySequence) -> EffectDelta {
        EffectDelta(EffectMutation::Reset(ResetPlan {
            envelope: EffectEnvelope {
                sequence,
                class: None,
                batch: Arc::clone(&self.generation_reset_batch),
            },
        }))
    }

    pub(super) fn plan_checkout(&self) -> Result<Option<(EffectDelta, EffectLease)>, EffectError> {
        if self.active.is_some() {
            return Ok(None);
        }
        let queued = self.queued.front();
        let reset = self.latest_generation_reset.as_ref();
        let (source, envelope) = match (queued, reset) {
            (Some(queued), Some(reset)) if reset.sequence < queued.sequence => {
                (CheckoutSource::GenerationReset, reset)
            }
            (Some(queued), _) => (CheckoutSource::Queued, queued),
            (None, Some(reset)) => (CheckoutSource::GenerationReset, reset),
            (None, None) => return Ok(None),
        };
        Ok(Some((
            EffectDelta(EffectMutation::Checkout(CheckoutPlan { source })),
            EffectLease {
                token: EffectToken {
                    sequence: envelope.sequence,
                },
                batch: Arc::clone(&envelope.batch),
            },
        )))
    }

    pub(super) fn plan_settlement(
        &self,
        settlement: &EffectSettlement,
    ) -> Result<EffectDelta, EffectError> {
        let active = self.active.as_ref().ok_or(EffectError::StaleLease)?;
        if active.sequence != settlement.token.sequence
            || !Arc::ptr_eq(&active.batch, &settlement.batch)
        {
            return Err(EffectError::StaleLease);
        }
        let after_usage = match settlement.disposition {
            EffectDisposition::Published | EffectDisposition::CircuitDisposed => {
                active.class.map_or(Some(self.usage), |class| {
                    self.usage
                        .checked_release(class, active.batch.charge_bytes())
                })
            }
            EffectDisposition::Retain => Some(self.usage),
        }
        .ok_or(EffectError::Projection)?;
        Ok(EffectDelta(EffectMutation::Settle(SettlementPlan {
            disposition: settlement.disposition,
            after_usage,
        })))
    }

    pub(super) fn plan_close(&self) -> Result<EffectDelta, EffectError> {
        if self.closed {
            return Err(EffectError::Closed);
        }
        Ok(EffectDelta(EffectMutation::Close))
    }

    pub(super) fn apply(&mut self, delta: EffectDelta) -> Option<Arc<EffectBatch>> {
        match delta.0 {
            EffectMutation::None => None,
            EffectMutation::Append(plan) => {
                self.usage = plan.usage;
                self.queued.push_back(plan.envelope);
                None
            }
            EffectMutation::Checkout(plan) => {
                let selected = match plan.source {
                    CheckoutSource::Queued => self.queued.pop_front(),
                    CheckoutSource::GenerationReset => self.latest_generation_reset.take(),
                };
                // The exclusive prepared plan proves this source is present.
                // Keeping the Option branch explicit avoids panic-based
                // invariant handling if future code violates that contract.
                if let Some(selected) = selected {
                    self.active = Some(selected);
                }
                None
            }
            EffectMutation::Settle(plan) => self.apply_settlement(plan),
            EffectMutation::Reset(plan) => self
                .latest_generation_reset
                .replace(plan.envelope)
                .map(|envelope| envelope.batch),
            EffectMutation::Close => {
                self.closed = true;
                None
            }
        }
    }

    fn apply_settlement(&mut self, plan: SettlementPlan) -> Option<Arc<EffectBatch>> {
        let active = self.active.take()?;
        self.usage = plan.after_usage;
        match plan.disposition {
            EffectDisposition::Published | EffectDisposition::CircuitDisposed => Some(active.batch),
            EffectDisposition::Retain => match active.class {
                Some(_) => {
                    self.queued.push_front(active);
                    None
                }
                None => {
                    if self
                        .latest_generation_reset
                        .as_ref()
                        .is_some_and(|latest| latest.sequence > active.sequence)
                    {
                        Some(active.batch)
                    } else {
                        self.latest_generation_reset = Some(active);
                        None
                    }
                }
            },
        }
    }

    pub(super) fn is_closed_and_drained(&self) -> bool {
        self.closed
            && self.queued.is_empty()
            && self.active.is_none()
            && self.latest_generation_reset.is_none()
            && self.usage == EffectRegionUsage::default()
    }

    #[cfg(test)]
    pub(super) fn observation(&self) -> EffectObservation {
        EffectObservation {
            queued: self
                .queued
                .iter()
                .map(|envelope| envelope.sequence)
                .collect(),
            active: self.active.as_ref().map(|envelope| envelope.sequence),
            latest_generation_reset: self
                .latest_generation_reset
                .as_ref()
                .map(|envelope| envelope.sequence),
            remote_usage: self.usage.remote,
            ordinary_usage: self.usage.ordinary,
            total_usage: self.usage.total,
            closed: self.closed,
        }
    }

    pub(super) fn semantically_consistent(&self, next_sequence: ApplySequence) -> bool {
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
        let active_precedes_pending = self.active.as_ref().is_none_or(|active| {
            self.queued
                .front()
                .is_none_or(|queued| active.sequence < queued.sequence)
                && self
                    .latest_generation_reset
                    .as_ref()
                    .is_none_or(|reset| active.sequence < reset.sequence)
        });
        rebuilt == self.usage
            && self.usage_within_limits()
            && all_sequences_before_clock
            && active_precedes_pending
            && self
                .latest_generation_reset
                .as_ref()
                .is_none_or(|reset| reset.class.is_none() && reset.batch.charge_bytes() == 0)
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

    fn validate_new_sequence(&self, sequence: ApplySequence) -> Result<(), EffectError> {
        let latest = self
            .queued
            .back()
            .into_iter()
            .chain(self.active.iter())
            .chain(self.latest_generation_reset.iter())
            .map(|envelope| envelope.sequence)
            .max();
        if latest.is_some_and(|latest| latest >= sequence) {
            Err(EffectError::Projection)
        } else {
            Ok(())
        }
    }
}
