use super::{
    dependency_progress::{ModelDependencyCut, ModelDependencyKey},
    evidence_transition::{ModelEvidenceFrontier, ModelKnownDependencies},
    proposal::{ProposalStatusReceipt, ProposalView},
    time_context::{ModelContextSensitivity, model_context_sensitivity},
};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, VecDeque},
};

pub(super) const EFFECT_ENVELOPE_BYTES: u32 = 4;
const EFFECT_ID_BYTES: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct TxId(pub(super) u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct WitnessId(pub(super) u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct ProposalId(pub(super) u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct CellId(pub(super) u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct HeaderId(pub(super) u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct PeerId(pub(super) u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct ViewId(pub(super) u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct RulesId(pub(super) u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct EntryVersion(pub(super) u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct ApplyStamp(pub(super) u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct Arrival(pub(super) u16);

/// Exact semantic order of production's Ready frontier, represented in
/// strongest-first order for the reference model. `TxId` is the finite-domain
/// abstraction of the raw transaction hash; `serialized_bytes` is derived
/// from the transaction's sealed cost quotient rather than aliased to its raw
/// retained-payload charge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ReadyKey {
    pub(super) source_priority: u8,
    pub(super) fee: u64,
    pub(super) serialized_bytes: u32,
    pub(super) arrival: Arrival,
    pub(super) transaction: TxId,
    pub(super) version: EntryVersion,
}

impl Ord for ReadyKey {
    fn cmp(&self, other: &Self) -> Ordering {
        let left_rate = u128::from(self.fee) * u128::from(other.serialized_bytes);
        let right_rate = u128::from(other.fee) * u128::from(self.serialized_bytes);
        self.source_priority
            .cmp(&other.source_priority)
            .then_with(|| right_rate.cmp(&left_rate))
            .then_with(|| other.fee.cmp(&self.fee))
            .then_with(|| self.arrival.cmp(&other.arrival))
            .then_with(|| self.transaction.cmp(&other.transaction))
            .then_with(|| other.version.cmp(&self.version))
    }
}

impl PartialOrd for ReadyKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct CapabilityId(pub(super) u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct DirectRequestId(pub(super) u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct PoolGeneration(pub(super) u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct ChainRevision(pub(super) u16);

/// One exact installed chain occurrence. `tip` may repeat across an ABA
/// sequence; `revision` is the authority-local ordering token that makes the
/// occurrence unique.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct ChainView {
    pub(super) tip: ViewId,
    pub(super) revision: ChainRevision,
}

impl ChainView {
    pub(super) const fn initial(tip: ViewId) -> Self {
        Self {
            tip,
            revision: ChainRevision(0),
        }
    }

    pub(super) fn advance(self, tip: ViewId) -> Option<Self> {
        Some(Self {
            tip,
            revision: ChainRevision(self.revision.0.checked_add(1)?),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct MonotonicTick(pub(super) u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct RemoteDeadline(pub(super) u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct RemoteResidency {
    pub(super) peer: PeerId,
    pub(super) expires_at: RemoteDeadline,
}

impl RemoteResidency {
    pub(super) const fn new(peer: PeerId, expires_at: RemoteDeadline) -> Self {
        Self { peer, expires_at }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(super) struct ResourceVector {
    pub(super) entries: u16,
    pub(super) bytes: u32,
    pub(super) edges: u16,
}

impl ResourceVector {
    pub(super) const ZERO: Self = Self {
        entries: 0,
        bytes: 0,
        edges: 0,
    };

    pub(super) fn checked_add(self, other: Self) -> Option<Self> {
        Some(Self {
            entries: self.entries.checked_add(other.entries)?,
            bytes: self.bytes.checked_add(other.bytes)?,
            edges: self.edges.checked_add(other.edges)?,
        })
    }

    pub(super) fn fits(self, limit: Self) -> bool {
        self.entries <= limit.entries && self.bytes <= limit.bytes && self.edges <= limit.edges
    }
}

/// Semantic publication class for one immutable committed-effect batch.
///
/// The class is selected by the command authority, not reconstructed from an
/// effect's audience: a Proposal promoted from Remote remains attributable to
/// its peer while consuming trusted publication headroom.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum EffectClass {
    Remote,
    Trusted,
    Critical,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(super) struct EffectCapacity {
    pub(super) batches: u16,
    pub(super) bytes: u32,
}

impl EffectCapacity {
    pub(super) const fn new(batches: u16, bytes: u32) -> Self {
        Self { batches, bytes }
    }

    fn checked_add(self, other: Self) -> Option<Self> {
        Some(Self {
            batches: self.batches.checked_add(other.batches)?,
            bytes: self.bytes.checked_add(other.bytes)?,
        })
    }

    fn fits(self, limit: Self) -> bool {
        self.batches <= limit.batches && self.bytes <= limit.bytes
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct EffectBatchBound {
    pub(super) effects: u16,
    pub(super) bytes: u32,
}

impl EffectBatchBound {
    pub(super) const fn new(effects: u16, bytes: u32) -> Self {
        Self { effects, bytes }
    }
}

/// Partitioned committed-effect capacity. `trusted_headroom` extends the
/// ordinary region without being available to Remote work; critical headroom
/// extends only the total region. This is the mathematical counterpart of the
/// production journal's three nested regions, not a second scheduling policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct EffectLimits {
    pub(super) remote: EffectCapacity,
    pub(super) trusted_headroom: EffectCapacity,
    pub(super) critical_headroom: EffectCapacity,
    pub(super) remote_bound: EffectBatchBound,
    pub(super) trusted_bound: EffectBatchBound,
    pub(super) critical_bound: EffectBatchBound,
}

impl EffectLimits {
    pub(super) const fn small() -> Self {
        Self {
            remote: EffectCapacity::new(8, 1_536),
            trusted_headroom: EffectCapacity::new(4, 768),
            critical_headroom: EffectCapacity::new(1, 192),
            remote_bound: EffectBatchBound::new(5, 192),
            trusted_bound: EffectBatchBound::new(5, 192),
            critical_bound: EffectBatchBound::new(5, 192),
        }
    }

    pub(super) const fn bound(self, class: EffectClass) -> EffectBatchBound {
        match class {
            EffectClass::Remote => self.remote_bound,
            EffectClass::Trusted => self.trusted_bound,
            EffectClass::Critical => self.critical_bound,
        }
    }

    fn ordinary(self) -> Option<EffectCapacity> {
        self.remote.checked_add(self.trusted_headroom)
    }

    fn total(self) -> Option<EffectCapacity> {
        self.ordinary()?.checked_add(self.critical_headroom)
    }

    fn validates(self, largest_effects: u16, largest_bytes: u32) -> bool {
        let Some(ordinary) = self.ordinary() else {
            return false;
        };
        let Some(total) = self.total() else {
            return false;
        };
        if self.remote.batches == 0
            || self.remote.bytes == 0
            || ordinary.batches == 0
            || ordinary.bytes == 0
            || total.batches == 0
            || total.bytes == 0
        {
            return false;
        }
        [self.remote_bound, self.trusted_bound, self.critical_bound]
            .into_iter()
            .all(|bound| {
                bound.effects >= largest_effects
                    && bound.bytes >= largest_bytes
                    && bound.effects != 0
                    && bound.bytes != 0
            })
            && self.remote_bound.bytes <= self.remote.bytes
            && self.trusted_bound.bytes <= ordinary.bytes
            && self.critical_bound.bytes <= total.bytes
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(super) struct EffectUsage {
    pub(super) remote: EffectCapacity,
    pub(super) ordinary: EffectCapacity,
    pub(super) total: EffectCapacity,
}

impl EffectUsage {
    fn checked_charge(self, class: EffectClass, bytes: u32) -> Option<Self> {
        let batch = EffectCapacity::new(1, bytes);
        Some(match class {
            EffectClass::Remote => Self {
                remote: self.remote.checked_add(batch)?,
                ordinary: self.ordinary.checked_add(batch)?,
                total: self.total.checked_add(batch)?,
            },
            EffectClass::Trusted => Self {
                ordinary: self.ordinary.checked_add(batch)?,
                total: self.total.checked_add(batch)?,
                ..self
            },
            EffectClass::Critical => Self {
                total: self.total.checked_add(batch)?,
                ..self
            },
        })
    }

    fn fits(self, limits: EffectLimits) -> bool {
        let Some(ordinary) = limits.ordinary() else {
            return false;
        };
        let Some(total) = limits.total() else {
            return false;
        };
        self.remote.fits(limits.remote) && self.ordinary.fits(ordinary) && self.total.fits(total)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct ModelLimits {
    pub(super) owners: ResourceVector,
    pub(super) retained: ResourceVector,
    pub(super) accepted: ResourceVector,
    pub(super) replacement_history: ResourceVector,
    pub(super) remote_per_peer: ResourceVector,
    pub(super) effects: EffectLimits,
    pub(super) compute_permits: u16,
    pub(super) peer_ban_fences: u16,
    pub(super) peer_ban_duration: u64,
}

impl ModelLimits {
    pub(super) const fn small() -> Self {
        Self {
            owners: ResourceVector {
                entries: 4,
                bytes: 64,
                edges: 16,
            },
            retained: ResourceVector {
                entries: 4,
                bytes: 64,
                edges: 16,
            },
            accepted: ResourceVector {
                entries: 4,
                bytes: 64,
                edges: 16,
            },
            replacement_history: ResourceVector {
                entries: 2,
                bytes: 32,
                edges: 8,
            },
            remote_per_peer: ResourceVector {
                entries: 2,
                bytes: 32,
                edges: 8,
            },
            effects: EffectLimits::small(),
            compute_permits: 2,
            peer_ban_fences: 2,
            peer_ban_duration: 10,
        }
    }

    pub(super) fn largest_indivisible_effect_batch(self) -> Option<(u16, u32)> {
        let records = self.owners.entries.checked_add(1)?;
        // One owner-sized direct candidate may be published together with one
        // effect for every resident owner. Resident effects may carry the
        // complete payload partition, while newly waiting Remote owners carry
        // the complete dependency-edge partition. Envelope bytes are charged
        // once per record. Adding all three independent terms is the bound;
        // taking their maximum would not cover a mixed RBF/dependency Apply.
        let bytes = self
            .owners
            .bytes
            .checked_mul(2)?
            .checked_add(u32::from(self.owners.edges).checked_mul(EFFECT_ID_BYTES)?)?
            .checked_add(u32::from(records).checked_mul(EFFECT_ENVELOPE_BYTES)?)?;
        Some((records, bytes))
    }

    pub(super) fn validate(self) -> Result<ValidatedLimits, ConfigurationError> {
        if self.compute_permits == 0
            || self.owners.entries == 0
            || self.peer_ban_fences == 0
            || self.peer_ban_duration == 0
        {
            return Err(ConfigurationError::ZeroCapacity);
        }
        let Some((largest_effect_batch_records, largest_effect_batch_bytes)) =
            self.largest_indivisible_effect_batch()
        else {
            return Err(ConfigurationError::IndivisibleEffectBatch);
        };
        if !self
            .effects
            .validates(largest_effect_batch_records, largest_effect_batch_bytes)
        {
            return Err(ConfigurationError::IndivisibleEffectBatch);
        }
        if !self.retained.fits(self.owners)
            || !self.accepted.fits(self.owners)
            || !self.replacement_history.fits(self.owners)
            || !self.replacement_history.fits(self.retained)
            || !self.remote_per_peer.fits(self.retained)
        {
            return Err(ConfigurationError::InvalidPartition);
        }
        Ok(ValidatedLimits(self))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct ValidatedLimits(ModelLimits);

impl ValidatedLimits {
    pub(super) const fn get(self) -> ModelLimits {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum ConfigurationError {
    ZeroCapacity,
    IndivisibleEffectBatch,
    InvalidPartition,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum InputOrigin {
    Chain,
    Pool(TxId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum PeerBanDeadline {
    At(MonotonicTick),
    ProcessLifetime,
}

impl PeerBanDeadline {
    pub(super) fn after(observed_at: MonotonicTick, duration: u64) -> Self {
        observed_at
            .0
            .checked_add(duration)
            .map_or(Self::ProcessLifetime, |deadline| {
                Self::At(MonotonicTick(deadline))
            })
    }

    pub(super) fn is_active_at(self, observed_at: MonotonicTick) -> bool {
        match self {
            Self::At(deadline) => deadline > observed_at,
            Self::ProcessLifetime => true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct PeerBanRecord {
    pub(super) deadline: PeerBanDeadline,
    pub(super) order: ApplyStamp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ModelTransactionCost {
    payload_bytes: u32,
    fee: u64,
    cycles: u64,
}

/// Exact `FeeRate` quotient used by the current tx-pool policy surface.
///
/// Production stores shannons per kilo-weight and computes a fee with a
/// saturating multiplication followed by integer division. Keeping that
/// arithmetic in one model type prevents RBF and minimum-fee relations from
/// silently substituting `rate * bytes` or a fixed 1000-shannon rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ModelFeeRate(u64);

impl ModelFeeRate {
    const KILO_WEIGHT: u64 = 1_000;

    pub(crate) const fn from_u64(shannons_per_kilo_weight: u64) -> Self {
        Self(shannons_per_kilo_weight)
    }

    pub(crate) const fn as_u64(self) -> u64 {
        self.0
    }

    pub(crate) const fn fee(self, weight: u64) -> u64 {
        self.0.saturating_mul(weight) / Self::KILO_WEIGHT
    }
}

/// Runtime replacement policy is part of the descriptive model input. The
/// default fixture matches the repository default, while the explicit
/// constructor covers every configured rate and the disabled state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum ModelReplacementPolicy {
    Disabled,
    Enabled { minimum_rate: ModelFeeRate },
}

impl ModelReplacementPolicy {
    const DEFAULT_MINIMUM_RATE: ModelFeeRate = ModelFeeRate::from_u64(1_500);

    pub(super) const fn default_enabled() -> Self {
        Self::Enabled {
            minimum_rate: Self::DEFAULT_MINIMUM_RATE,
        }
    }

    pub(super) const fn minimum_rate(self) -> Option<ModelFeeRate> {
        match self {
            Self::Disabled => None,
            Self::Enabled { minimum_rate } => Some(minimum_rate),
        }
    }
}

/// One transaction's minimum observation-and-cost quotient.
///
/// `payload_bytes` is the raw Molecule transaction size charged while the
/// payload is retained. A transaction inside a block additionally occupies
/// one Molecule `NUMBER_SIZE` offset, so economic ordering, block limits and
/// eviction derive (and never independently supply) `serialized_bytes`.
/// `fee` becomes observable only after resolution and `cycles` only after
/// verification; storing their eventual sealed values here is a prophecy
/// representation, not permission for an earlier transition to inspect them.
impl ModelTransactionCost {
    const BLOCK_VECTOR_OFFSET_BYTES: u32 = 4;
    const FIXTURE: Self = Self {
        payload_bytes: 4,
        fee: 10,
        cycles: 0,
    };

    pub(crate) const fn new(payload_bytes: u32, fee: u64, cycles: u64) -> Option<Self> {
        if payload_bytes
            .checked_add(Self::BLOCK_VECTOR_OFFSET_BYTES)
            .is_none()
        {
            return None;
        }
        Some(Self {
            payload_bytes,
            fee,
            cycles,
        })
    }

    pub(crate) const fn payload_bytes(self) -> u32 {
        self.payload_bytes
    }

    pub(crate) const fn serialized_bytes(self) -> u32 {
        // `new` is the only constructor and proves this addition cannot
        // overflow; keeping the derived coordinate unstored is the smaller
        // normal form.
        self.payload_bytes + Self::BLOCK_VECTOR_OFFSET_BYTES
    }

    pub(crate) const fn fee(self) -> u64 {
        self.fee
    }

    pub(crate) const fn cycles(self) -> u64 {
        self.cycles
    }

    pub(crate) const fn with_fee(self, fee: u64) -> Self {
        Self { fee, ..self }
    }

    pub(crate) const fn with_cycles(self, cycles: u64) -> Self {
        Self { cycles, ..self }
    }

    pub(crate) const fn with_payload_bytes(self, payload_bytes: u32) -> Option<Self> {
        Self::new(payload_bytes, self.fee, self.cycles)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct Transaction {
    pub(super) id: TxId,
    pub(super) witness: WitnessId,
    pub(super) proposal: ProposalId,
    pub(super) inputs: BTreeSet<CellId>,
    /// Exact subset of inputs whose packed `since` value is nonzero.  Keeping
    /// the producer set, instead of a free boolean, makes malformed model
    /// fibers mechanically rejectable.
    pub(super) since_inputs: BTreeSet<CellId>,
    /// Direct cell-dependency outpoints encoded by the transaction.  This is
    /// intentionally not the resolved read footprint: dep-group members are
    /// discovered from cell data under a resolution cut.
    pub(super) cell_deps: BTreeSet<CellId>,
    /// Exact subset of direct declarations whose encoded `dep_type` is
    /// `DepGroup`.  Keeping the primitive tag separate prevents a model trace
    /// from inventing group expansion for an ordinary code dependency.
    pub(super) dep_groups: BTreeSet<CellId>,
    pub(super) header_deps: BTreeSet<HeaderId>,
    pub(super) outputs: BTreeSet<CellId>,
    pub(super) cost: ModelTransactionCost,
}

impl Transaction {
    pub(super) fn independent(id: u8, witness: u8, input: u8, output: u8) -> Self {
        Self {
            id: TxId(id),
            witness: WitnessId(witness),
            proposal: ProposalId(id),
            inputs: BTreeSet::from([CellId(input)]),
            since_inputs: BTreeSet::new(),
            cell_deps: BTreeSet::new(),
            dep_groups: BTreeSet::new(),
            header_deps: BTreeSet::new(),
            outputs: BTreeSet::from([CellId(output)]),
            cost: ModelTransactionCost::FIXTURE,
        }
    }

    pub(super) fn dependent(id: u8, witness: u8, input: u8, output: u8) -> Self {
        Self {
            id: TxId(id),
            witness: WitnessId(witness),
            proposal: ProposalId(id),
            inputs: BTreeSet::from([CellId(input)]),
            since_inputs: BTreeSet::new(),
            cell_deps: BTreeSet::new(),
            dep_groups: BTreeSet::new(),
            header_deps: BTreeSet::new(),
            outputs: BTreeSet::from([CellId(output)]),
            cost: ModelTransactionCost::FIXTURE,
        }
    }

    pub(super) const fn with_cost(mut self, cost: ModelTransactionCost) -> Self {
        self.cost = cost;
        self
    }

    pub(super) fn with_cycles(mut self, cycles: u64) -> Self {
        self.cost = self.cost.with_cycles(cycles);
        self
    }

    pub(super) fn with_fee(mut self, fee: u64) -> Self {
        self.cost = self.cost.with_fee(fee);
        self
    }

    pub(super) fn with_payload_bytes(mut self, payload_bytes: u32) -> Option<Self> {
        self.cost = self.cost.with_payload_bytes(payload_bytes)?;
        Some(self)
    }

    pub(super) fn with_since(mut self, input: CellId) -> Option<Self> {
        self.inputs.contains(&input).then(|| {
            self.since_inputs.insert(input);
            self
        })
    }

    pub(super) fn has_valid_time_shape(&self) -> bool {
        self.since_inputs.is_subset(&self.inputs)
    }

    pub(super) fn has_valid_dependency_shape(&self) -> bool {
        self.dep_groups.is_subset(&self.cell_deps)
    }

    pub(super) fn declared_dependencies(&self) -> Option<ModelKnownDependencies> {
        self.has_valid_dependency_shape().then(|| {
            self.inputs
                .iter()
                .map(|cell| ModelDependencyKey::cell(cell.0))
                .chain(
                    self.cell_deps
                        .iter()
                        .map(|cell| ModelDependencyKey::cell(cell.0)),
                )
                .chain(
                    self.header_deps
                        .iter()
                        .map(|header| ModelDependencyKey::header(header.0)),
                )
                .collect()
        })
    }

    pub(super) fn charge(&self) -> Option<ResourceVector> {
        let edges = self
            .inputs
            .len()
            .checked_add(self.cell_deps.len())?
            .checked_add(self.header_deps.len())?;
        Some(ResourceVector {
            entries: 1,
            bytes: self.cost.payload_bytes(),
            edges: u16::try_from(edges).ok()?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum Source {
    Recovery(PoolGeneration),
    Proposal { base: ProposalBase },
    Remote(RemoteResidency),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum ProposalBase {
    Trusted,
    Remote(RemoteResidency),
}

impl Source {
    pub(super) const fn priority(self) -> u8 {
        match self {
            Self::Recovery(_) => 0,
            Self::Proposal { .. } => 1,
            Self::Remote(_) => 2,
        }
    }

    /// Effect control follows the current command authority. Immutable peer
    /// attribution is deliberately not used for capacity selection.
    pub(super) const fn effect_class(self) -> EffectClass {
        match self {
            Self::Remote(_) => EffectClass::Remote,
            Self::Recovery(_) | Self::Proposal { .. } => EffectClass::Trusted,
        }
    }

    pub(super) const fn ingress_peer(self) -> Option<PeerId> {
        match self {
            Self::Proposal {
                base: ProposalBase::Remote(residency),
            }
            | Self::Remote(residency) => Some(residency.peer),
            Self::Proposal {
                base: ProposalBase::Trusted,
            } => None,
            Self::Recovery(_) => None,
        }
    }

    pub(super) const fn active_remote_deadline(self) -> Option<RemoteDeadline> {
        match self {
            Self::Remote(residency) => Some(residency.expires_at),
            Self::Recovery(_) | Self::Proposal { .. } => None,
        }
    }

    pub(super) const fn accepted_provenance(self) -> AcceptedProvenance {
        match self.ingress_peer() {
            Some(peer) => AcceptedProvenance::Peer(peer),
            None => AcceptedProvenance::Trusted,
        }
    }

    /// Verification-result validity follows the authority that owned the
    /// payload at checkout, not immutable peer attribution retained for later
    /// effects. A Proposal is trusted even when it preserves its remote base.
    pub(super) const fn work_payload_policy(self) -> WorkPayloadPolicy {
        match self {
            Self::Remote(_) => WorkPayloadPolicy::Remote,
            Self::Recovery(_) | Self::Proposal { .. } => WorkPayloadPolicy::Trusted,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum AcceptedProvenance {
    Trusted,
    Peer(PeerId),
}

impl AcceptedProvenance {
    pub(super) const fn ingress_peer(self) -> Option<PeerId> {
        match self {
            Self::Trusted => None,
            Self::Peer(peer) => Some(peer),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum RetainedSource {
    Recovery(PoolGeneration),
    Proposal,
    Remote(RemoteResidency),
}

impl From<RetainedSource> for Source {
    fn from(source: RetainedSource) -> Self {
        match source {
            RetainedSource::Recovery(generation) => Self::Recovery(generation),
            RetainedSource::Proposal => Self::Proposal {
                base: ProposalBase::Trusted,
            },
            RetainedSource::Remote(residency) => Self::Remote(residency),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum WorkKind {
    Resolve,
    Verify,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum VerifyCycleClass {
    #[default]
    Small,
    Large,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum VerifyCapability {
    Any,
    SmallCycleOnly,
}

impl VerifyCapability {
    pub(super) const fn permits(self, class: VerifyCycleClass) -> bool {
        matches!(self, Self::Any) || matches!(class, VerifyCycleClass::Small)
    }
}

/// The exact authority-visible permit that owns one retained computation.
///
/// `ResolveThenVerify` is the only permit whose move-only capability may
/// advance from Resolve to Verify without an authority Apply. The owner keeps
/// this permit stable while the capability carries the private stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum WorkPermit {
    ResolveOnly,
    VerifyOnly(VerifyCapability),
    ResolveThenVerify(VerifyCapability),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum DirectKind {
    Local,
    TestAccept,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum AcceptedStatus {
    Pending,
    Gap,
    Proposed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct EvidenceContext {
    pub(super) chain: ChainView,
    pub(super) rules: RulesId,
    pub(super) witness: WitnessId,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct ResolvedEvidence {
    pub(super) context: EvidenceContext,
    pub(super) verify_class: VerifyCycleClass,
    pub(super) input_origins: BTreeMap<CellId, InputOrigin>,
    /// Exact dep-group container-to-member expansion read from resolved cell
    /// data. Keys equal the transaction's `DepGroup` declarations and every
    /// member set is nonempty.
    pub(super) dep_group_members: BTreeMap<CellId, BTreeSet<CellId>>,
    /// Origins of the canonical expanded conditional-read footprint: direct
    /// cell deps plus group members, minus transaction inputs because the
    /// input/spender role dominates an overlapping read.
    pub(super) dep_origins: BTreeMap<CellId, InputOrigin>,
    /// Exact subset of chain-origin inputs and expanded cell dependencies
    /// whose producer is a non-genesis cellbase.  Dep-group containers are
    /// deliberately absent because the consensus maturity verifier does not
    /// read that role.
    pub(super) chain_cellbases: BTreeSet<CellId>,
    // Header dependencies are immutable chain-view reads. Keeping them in a
    // distinct set makes pool origin unrepresentable while still binding the
    // complete transaction footprint to this exact evidence cut.
    pub(super) header_deps: BTreeSet<HeaderId>,
}

impl ResolvedEvidence {
    fn dependency_reads(
        transaction: &Transaction,
        dep_group_members: &BTreeMap<CellId, BTreeSet<CellId>>,
    ) -> Option<BTreeSet<CellId>> {
        if !transaction.has_valid_dependency_shape()
            || dep_group_members.keys().copied().collect::<BTreeSet<_>>() != transaction.dep_groups
            || dep_group_members.values().any(BTreeSet::is_empty)
        {
            return None;
        }
        Some(
            transaction
                .cell_deps
                .iter()
                .chain(dep_group_members.values().flatten())
                .filter(|cell| !transaction.inputs.contains(cell))
                .copied()
                .collect(),
        )
    }

    pub(super) fn for_transaction(
        transaction: &Transaction,
        chain: ChainView,
        rules: RulesId,
    ) -> Option<Self> {
        Self::with_dep_group_members(transaction, chain, rules, BTreeMap::new())
    }

    pub(super) fn with_dep_group_members(
        transaction: &Transaction,
        chain: ChainView,
        rules: RulesId,
        dep_group_members: BTreeMap<CellId, BTreeSet<CellId>>,
    ) -> Option<Self> {
        let dependency_reads = Self::dependency_reads(transaction, &dep_group_members)?;
        Some(Self {
            context: EvidenceContext {
                chain,
                rules,
                witness: transaction.witness,
            },
            // The class is not a transaction primitive. Production derives
            // it while sealing resolution evidence from the checkout payload
            // policy and configured threshold. Fixtures default to the
            // trusted/small quotient and may replace it only on this receipt.
            verify_class: VerifyCycleClass::Small,
            input_origins: transaction
                .inputs
                .iter()
                .copied()
                .map(|cell| (cell, InputOrigin::Chain))
                .collect(),
            dep_group_members,
            dep_origins: dependency_reads
                .iter()
                .copied()
                .map(|cell| (cell, InputOrigin::Chain))
                .collect(),
            chain_cellbases: BTreeSet::new(),
            header_deps: transaction.header_deps.clone(),
        })
    }

    pub(super) fn with_pool_input(
        transaction: &Transaction,
        chain: ChainView,
        rules: RulesId,
        cell: CellId,
        parent: TxId,
    ) -> Option<Self> {
        let mut evidence = Self::for_transaction(transaction, chain, rules)?;
        if evidence.input_origins.contains_key(&cell) {
            evidence
                .input_origins
                .insert(cell, InputOrigin::Pool(parent));
        }
        Some(evidence)
    }

    pub(super) fn with_verify_class(mut self, verify_class: VerifyCycleClass) -> Self {
        self.verify_class = verify_class;
        self
    }

    pub(super) fn with_pool_dependency(mut self, cell: CellId, parent: TxId) -> Option<Self> {
        self.dep_origins.contains_key(&cell).then(|| {
            self.dep_origins.insert(cell, InputOrigin::Pool(parent));
            self
        })
    }

    pub(super) fn with_chain_cellbase(mut self, cell: CellId) -> Option<Self> {
        let chain_origin = self
            .input_origins
            .get(&cell)
            .or_else(|| self.dep_origins.get(&cell))
            == Some(&InputOrigin::Chain);
        chain_origin.then(|| {
            self.chain_cellbases.insert(cell);
            self
        })
    }

    pub(super) fn context_sensitivity(
        &self,
        transaction: &Transaction,
    ) -> Option<ModelContextSensitivity> {
        if !transaction.has_valid_time_shape()
            || self.chain_cellbases.iter().any(|cell| {
                self.input_origins
                    .get(cell)
                    .or_else(|| self.dep_origins.get(cell))
                    != Some(&InputOrigin::Chain)
            })
        {
            return None;
        }
        Some(model_context_sensitivity(
            !transaction.since_inputs.is_empty(),
            !self.chain_cellbases.is_empty(),
        ))
    }

    pub(super) fn dependencies(&self, transaction: &Transaction) -> Option<ModelKnownDependencies> {
        (Self::dependency_reads(transaction, &self.dep_group_members)?
            == self.dep_origins.keys().copied().collect::<BTreeSet<_>>())
        .then(|| {
            self.input_origins
                .keys()
                .map(|cell| ModelDependencyKey::cell(cell.0))
                .chain(
                    self.dep_origins
                        .keys()
                        .map(|cell| ModelDependencyKey::cell(cell.0)),
                )
                .chain(
                    self.header_deps
                        .iter()
                        .map(|header| ModelDependencyKey::header(header.0)),
                )
                .collect()
        })
    }

    pub(super) fn conditional_reads(&self) -> BTreeSet<CellId> {
        self.dep_origins.keys().copied().collect()
    }

    pub(super) fn is_for(
        &self,
        transaction: &Transaction,
        chain: ChainView,
        rules: RulesId,
    ) -> bool {
        self.context
            == (EvidenceContext {
                chain,
                rules,
                witness: transaction.witness,
            })
            && self.input_origins.keys().copied().collect::<BTreeSet<_>>() == transaction.inputs
            && self.dependencies(transaction).is_some()
            && self.context_sensitivity(transaction).is_some()
            && self.header_deps == transaction.header_deps
    }

    pub(super) fn has_transaction_shape(&self, transaction: &Transaction, rules: RulesId) -> bool {
        self.context.rules == rules
            && self.context.witness == transaction.witness
            && self.input_origins.keys().copied().collect::<BTreeSet<_>>() == transaction.inputs
            && self.dependencies(transaction).is_some()
            && self.context_sensitivity(transaction).is_some()
            && self.header_deps == transaction.header_deps
    }
}

/// Resolved content paired with the exact dependency cut that produced it.
/// The wrapper is the model counterpart of production `ResolvedFacts`; it
/// prevents checkout or Ready/Accepted storage from silently rebinding old
/// dependency evidence to a newer authority sequence.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct ResolvedEvidenceAtCut {
    evidence: ResolvedEvidence,
    pub(super) dependency_cut: ModelDependencyCut,
}

impl ResolvedEvidenceAtCut {
    pub(super) const fn new(
        evidence: ResolvedEvidence,
        dependency_cut: ModelDependencyCut,
    ) -> Self {
        Self {
            evidence,
            dependency_cut,
        }
    }

    pub(super) const fn verify_class(&self) -> VerifyCycleClass {
        self.evidence.verify_class
    }
}

impl std::ops::Deref for ResolvedEvidenceAtCut {
    type Target = ResolvedEvidence;

    fn deref(&self) -> &Self::Target {
        &self.evidence
    }
}

impl std::ops::DerefMut for ResolvedEvidenceAtCut {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.evidence
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum WorkStage {
    Resolve,
    Verify(ResolvedEvidenceAtCut),
}

/// Minimum validity domain of a terminal worker rejection. The public reason
/// is observationally irrelevant to this model, but whether the reason remains
/// valid after a chain change is not.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SettlementRejection {
    ChainBound,
    ResourceBound,
}

/// Checkout-time quotient of the production payload policy. Legal model
/// traces permit only equality or Remote-to-Trusted promotion; the exact
/// declared-cycle truth table remains owned by `settlement_transition`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum WorkPayloadPolicy {
    Remote,
    Trusted,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum WorkResult {
    Resolved(ResolvedEvidence),
    Verified,
    Missing(MissingDependencies),
    Rejected(SettlementRejection),
    VerificationRejected,
    Retry,
}

impl WorkResult {
    pub(super) const fn resolve_rejected() -> Self {
        Self::Rejected(SettlementRejection::ChainBound)
    }

    pub(super) const fn resource_rejected() -> Self {
        Self::Rejected(SettlementRejection::ResourceBound)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct MissingDependencies {
    transaction: TxId,
    cells: BTreeSet<CellId>,
    /// Proof that every missing cell not named directly by the transaction
    /// was discovered as a member of one declared dep-group container.
    group_missing_members: BTreeMap<CellId, BTreeSet<CellId>>,
}

impl MissingDependencies {
    pub(super) fn for_transaction(
        transaction: &Transaction,
        cells: BTreeSet<CellId>,
    ) -> Option<Self> {
        Self::for_dependencies(transaction, cells, BTreeMap::new())
    }

    pub(super) fn for_dependencies(
        transaction: &Transaction,
        cells: BTreeSet<CellId>,
        group_missing_members: BTreeMap<CellId, BTreeSet<CellId>>,
    ) -> Option<Self> {
        let directly_referenced = transaction
            .inputs
            .iter()
            .chain(&transaction.cell_deps)
            .copied()
            .collect::<BTreeSet<_>>();
        let discovered = group_missing_members
            .values()
            .flatten()
            .copied()
            .collect::<BTreeSet<_>>();
        let extra = cells
            .difference(&directly_referenced)
            .copied()
            .collect::<BTreeSet<_>>();
        (!cells.is_empty()
            && transaction.has_valid_dependency_shape()
            && group_missing_members
                .keys()
                .all(|container| transaction.dep_groups.contains(container))
            && group_missing_members
                .values()
                .all(|members| !members.is_empty())
            && discovered == extra)
            .then_some(Self {
                transaction: transaction.id,
                cells,
                group_missing_members,
            })
    }

    pub(super) fn from_resolved(
        transaction: &Transaction,
        evidence: &ResolvedEvidence,
        cells: BTreeSet<CellId>,
    ) -> Option<Self> {
        evidence.dependencies(transaction)?;
        let directly_referenced = transaction
            .inputs
            .iter()
            .chain(&transaction.cell_deps)
            .copied()
            .collect::<BTreeSet<_>>();
        let extra = cells
            .difference(&directly_referenced)
            .copied()
            .collect::<BTreeSet<_>>();
        let group_missing_members = evidence
            .dep_group_members
            .iter()
            .filter_map(|(container, members)| {
                let missing = members
                    .intersection(&extra)
                    .copied()
                    .collect::<BTreeSet<_>>();
                (!missing.is_empty()).then_some((*container, missing))
            })
            .collect();
        Self::for_dependencies(transaction, cells, group_missing_members)
    }

    pub(super) fn is_for(&self, transaction: &Transaction) -> bool {
        Self::for_dependencies(
            transaction,
            self.cells.clone(),
            self.group_missing_members.clone(),
        )
        .as_ref()
            == Some(self)
    }

    pub(super) fn cells(&self) -> &BTreeSet<CellId> {
        &self.cells
    }

    pub(super) fn dependencies(&self, transaction: &Transaction) -> Option<ModelKnownDependencies> {
        self.is_for(transaction).then(|| {
            transaction
                .declared_dependencies()
                .expect("validated transaction dependency shape")
                .into_iter()
                .chain(
                    self.cells
                        .iter()
                        .map(|cell| ModelDependencyKey::cell(cell.0)),
                )
                .collect()
        })
    }

    pub(super) fn missing_keys(&self) -> ModelKnownDependencies {
        self.cells
            .iter()
            .map(|cell| ModelDependencyKey::cell(cell.0))
            .collect()
    }

    pub(super) fn extend(&mut self, transaction: &Transaction, cells: &BTreeSet<CellId>) -> bool {
        if self.transaction != transaction.id
            || cells.iter().any(|cell| {
                !self.dependencies(transaction).is_some_and(|dependencies| {
                    dependencies.contains(&ModelDependencyKey::cell(cell.0))
                })
            })
        {
            return false;
        }
        self.cells.extend(cells.iter().copied());
        true
    }

    pub(super) fn retain_unavailable(&mut self, available: &BTreeSet<CellId>) {
        self.cells.retain(|cell| !available.contains(cell));
    }

    pub(super) fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }
}

impl WorkStage {
    pub(super) const fn kind(&self) -> WorkKind {
        match self {
            Self::Resolve => WorkKind::Resolve,
            Self::Verify(_) => WorkKind::Verify,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct ActiveWork {
    pub(super) permit: WorkPermit,
    pub(super) dependency_cut: ModelDependencyCut,
    pub(super) dependencies: ModelKnownDependencies,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum RetainedPhase {
    Queued(WorkStage),
    Computing(ActiveWork),
    Waiting { missing: MissingDependencies },
    Ready(ResolvedEvidenceAtCut),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct RetainedOwner {
    pub(super) source: Source,
    pub(super) phase: RetainedPhase,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum OwnerLocation {
    Retained(RetainedOwner),
    Accepted {
        provenance: AcceptedProvenance,
        accepted_at_wall: u64,
        evidence: ResolvedEvidenceAtCut,
        /// Sealed cache of the current chain proposal projection. This is part
        /// of Accepted ownership and changes atomically with `proposals`.
        proposal: ProposalStatusReceipt,
    },
    ReplacementHistory {
        missing: MissingDependencies,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct Owner {
    pub(super) version: EntryVersion,
    pub(super) arrival: Arrival,
    pub(super) transaction: Transaction,
    pub(super) location: OwnerLocation,
}

impl Owner {
    pub(super) const fn retained_source(&self) -> Option<Source> {
        match &self.location {
            OwnerLocation::Retained(retained) => Some(retained.source),
            OwnerLocation::Accepted { .. } | OwnerLocation::ReplacementHistory { .. } => None,
        }
    }

    pub(super) const fn ingress_peer(&self) -> Option<PeerId> {
        match &self.location {
            OwnerLocation::Retained(retained) => retained.source.ingress_peer(),
            OwnerLocation::Accepted { provenance, .. } => provenance.ingress_peer(),
            OwnerLocation::ReplacementHistory { .. } => None,
        }
    }

    /// Canonical dependency slot for this exact lifecycle location.  This is
    /// the sole model projection used by checkout, event indexing and
    /// frontier pruning; callers cannot independently choose raw versus
    /// expanded dependencies.
    pub(super) fn dependencies(&self) -> Option<ModelKnownDependencies> {
        match &self.location {
            OwnerLocation::Retained(RetainedOwner { phase, .. }) => match phase {
                RetainedPhase::Queued(WorkStage::Resolve) => {
                    self.transaction.declared_dependencies()
                }
                RetainedPhase::Queued(WorkStage::Verify(evidence))
                | RetainedPhase::Ready(evidence) => evidence.dependencies(&self.transaction),
                RetainedPhase::Computing(active) => Some(active.dependencies.clone()),
                RetainedPhase::Waiting { missing } => missing.dependencies(&self.transaction),
            },
            OwnerLocation::Accepted { evidence, .. } => evidence.dependencies(&self.transaction),
            OwnerLocation::ReplacementHistory { missing } => {
                missing.dependencies(&self.transaction)
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum AcceptanceEffect {
    Admission {
        status: AcceptedStatus,
        ingress_peer: Option<PeerId>,
    },
    Duplicate {
        requesting_peer: Option<PeerId>,
    },
    ChainStatusChange {
        status: AcceptedStatus,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum MembershipRejection {
    Unavailable,
    Policy,
    Resource,
    CandidateEvicted,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum RejectionEffect {
    Validation {
        ingress_peer: Option<PeerId>,
    },
    Membership {
        ingress_peer: Option<PeerId>,
        reason: MembershipRejection,
    },
    Replaced {
        winner: TxId,
    },
    CapacityEvicted,
    Expired,
    ChainConflict {
        cell: CellId,
        accepted: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum LogicalEffect {
    Accepted {
        transaction: TxId,
        payload_bytes: u32,
        cause: AcceptanceEffect,
    },
    Rejected {
        transaction: TxId,
        payload_bytes: u32,
        cause: RejectionEffect,
    },
    ChainCommitted(TxId),
    PeerCohortRevoked(PeerId),
    RemoteExpired(TxId),
    IngressReleased(TxId),
    ParentTransactionsRequested {
        transaction: TxId,
        parent_count: u16,
    },
    GenerationReset,
}

impl LogicalEffect {
    pub(super) const fn admitted(
        transaction: &Transaction,
        status: AcceptedStatus,
        ingress_peer: Option<PeerId>,
    ) -> Self {
        Self::Accepted {
            transaction: transaction.id,
            payload_bytes: transaction.cost.payload_bytes(),
            cause: AcceptanceEffect::Admission {
                status,
                ingress_peer,
            },
        }
    }

    pub(super) const fn accepted_duplicate(
        transaction: TxId,
        requesting_peer: Option<PeerId>,
    ) -> Self {
        Self::Accepted {
            transaction,
            payload_bytes: 0,
            cause: AcceptanceEffect::Duplicate { requesting_peer },
        }
    }

    pub(super) const fn status_changed(transaction: &Transaction, status: AcceptedStatus) -> Self {
        Self::Accepted {
            transaction: transaction.id,
            payload_bytes: transaction.cost.payload_bytes(),
            cause: AcceptanceEffect::ChainStatusChange { status },
        }
    }

    pub(super) const fn validation_rejected(
        transaction: &Transaction,
        ingress_peer: Option<PeerId>,
    ) -> Self {
        Self::Rejected {
            transaction: transaction.id,
            payload_bytes: transaction.cost.payload_bytes(),
            cause: RejectionEffect::Validation { ingress_peer },
        }
    }

    pub(super) const fn membership_rejected(
        transaction: &Transaction,
        ingress_peer: Option<PeerId>,
        reason: MembershipRejection,
    ) -> Self {
        Self::Rejected {
            transaction: transaction.id,
            payload_bytes: transaction.cost.payload_bytes(),
            cause: RejectionEffect::Membership {
                ingress_peer,
                reason,
            },
        }
    }

    pub(super) const fn replaced(transaction: &Transaction, winner: TxId) -> Self {
        Self::Rejected {
            transaction: transaction.id,
            payload_bytes: transaction.cost.payload_bytes(),
            cause: RejectionEffect::Replaced { winner },
        }
    }

    pub(super) const fn capacity_evicted(transaction: &Transaction) -> Self {
        Self::Rejected {
            transaction: transaction.id,
            payload_bytes: transaction.cost.payload_bytes(),
            cause: RejectionEffect::CapacityEvicted,
        }
    }

    pub(super) const fn expired(transaction: &Transaction) -> Self {
        Self::Rejected {
            transaction: transaction.id,
            payload_bytes: transaction.cost.payload_bytes(),
            cause: RejectionEffect::Expired,
        }
    }

    pub(super) const fn chain_conflict(
        transaction: &Transaction,
        cell: CellId,
        accepted: bool,
    ) -> Self {
        Self::Rejected {
            transaction: transaction.id,
            payload_bytes: transaction.cost.payload_bytes(),
            cause: RejectionEffect::ChainConflict { cell, accepted },
        }
    }

    pub(super) fn parent_transactions_requested(
        transaction: TxId,
        parent_count: usize,
    ) -> Option<Self> {
        Some(Self::ParentTransactionsRequested {
            transaction,
            parent_count: u16::try_from(parent_count).ok()?,
        })
    }

    pub(super) fn charge_bytes(&self) -> Option<u32> {
        match self {
            Self::Accepted { payload_bytes, .. } | Self::Rejected { payload_bytes, .. } => {
                payload_bytes.checked_add(EFFECT_ENVELOPE_BYTES)
            }
            Self::ParentTransactionsRequested { parent_count, .. } => u32::from(*parent_count)
                .checked_mul(EFFECT_ID_BYTES)?
                .checked_add(EFFECT_ENVELOPE_BYTES),
            Self::ChainCommitted(_)
            | Self::PeerCohortRevoked(_)
            | Self::RemoteExpired(_)
            | Self::IngressReleased(_) => Some(EFFECT_ENVELOPE_BYTES),
            Self::GenerationReset => Some(0),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct EffectRecord {
    pub(super) stamp: ApplyStamp,
    pub(super) ordinal: u16,
    pub(super) class: EffectClass,
    /// Immutable batch metadata is repeated on each logical record so partial
    /// endpoint progress cannot release journal capacity before the batch's
    /// final record settles.
    pub(super) batch_effects: u16,
    pub(super) batch_bytes: u32,
    pub(super) logical: LogicalEffect,
}

impl EffectRecord {
    pub(super) fn batch_shape(effects: &[LogicalEffect]) -> Option<(u16, u32)> {
        let count = u16::try_from(effects.len()).ok()?;
        let bytes = effects.iter().try_fold(0u32, |used, effect| {
            used.checked_add(effect.charge_bytes()?)
        })?;
        Some((count, bytes))
    }

    /// Sealed model constructor for one immutable publication batch.
    ///
    /// Repeating the full batch shape on every remaining record is what keeps
    /// a partially consumed batch charged as one indivisible journal unit.
    pub(super) fn from_batch(
        stamp: ApplyStamp,
        class: EffectClass,
        effects: Vec<LogicalEffect>,
    ) -> Option<Vec<Self>> {
        if effects.is_empty()
            || effects
                .iter()
                .any(|effect| matches!(effect, LogicalEffect::GenerationReset))
                && !(class == EffectClass::Critical
                    && matches!(effects.as_slice(), [LogicalEffect::GenerationReset]))
        {
            return None;
        }
        let (batch_effects, batch_bytes) = Self::batch_shape(&effects)?;
        effects
            .into_iter()
            .enumerate()
            .map(|(ordinal, logical)| {
                Some(Self {
                    stamp,
                    ordinal: u16::try_from(ordinal).ok()?,
                    class,
                    batch_effects,
                    batch_bytes,
                    logical,
                })
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum EffectClaimSource {
    Queued,
    GenerationReset,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct WorkCapability {
    pub(super) id: CapabilityId,
    pub(super) transaction: TxId,
    pub(super) version: EntryVersion,
    permit: WorkPermit,
    stage: WorkStage,
    pub(super) chain: ChainView,
    pub(super) rules: RulesId,
    pub(super) dependency_cut: ModelDependencyCut,
    payload_policy: WorkPayloadPolicy,
}

impl WorkCapability {
    // Every argument is an independent coordinate of the checked-out linear
    // capability. Grouping them in a second input DTO would duplicate the
    // model authority without reducing the state space.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn for_checkout(
        id: CapabilityId,
        transaction: TxId,
        version: EntryVersion,
        permit: WorkPermit,
        stage: WorkStage,
        chain: ChainView,
        rules: RulesId,
        dependency_cut: ModelDependencyCut,
        payload_policy: WorkPayloadPolicy,
    ) -> Option<Self> {
        permit.permits_checkout(&stage).then_some(Self {
            id,
            transaction,
            version,
            permit,
            stage,
            chain,
            rules,
            dependency_cut,
            payload_policy,
        })
    }

    pub(super) const fn permit(&self) -> WorkPermit {
        self.permit
    }

    pub(super) const fn stage(&self) -> &WorkStage {
        &self.stage
    }

    pub(super) const fn kind(&self) -> WorkKind {
        self.stage.kind()
    }

    pub(super) const fn payload_policy(&self) -> WorkPayloadPolicy {
        self.payload_policy
    }

    /// The legal public validation-rejection result for this checked-out
    /// stage. Resolve owns a chain-bound rejection; Verify owns the exact
    /// resolved receipt already sealed in `WorkStage::Verify`.
    pub(super) const fn stage_rejection_result(&self) -> WorkResult {
        match self.stage {
            WorkStage::Resolve => WorkResult::resolve_rejected(),
            WorkStage::Verify(_) => WorkResult::VerificationRejected,
        }
    }

    pub(super) fn continue_resolve_then_verify(&mut self, evidence: ResolvedEvidence) -> bool {
        let WorkPermit::ResolveThenVerify(capability) = self.permit else {
            return false;
        };
        if !matches!(self.stage, WorkStage::Resolve) || !capability.permits(evidence.verify_class) {
            return false;
        }
        self.stage = WorkStage::Verify(ResolvedEvidenceAtCut::new(evidence, self.dependency_cut));
        true
    }

    pub(super) const fn is_compatible(&self) -> bool {
        self.permit.permits_active(&self.stage)
    }
}

impl WorkPermit {
    const fn permits_checkout(self, stage: &WorkStage) -> bool {
        match (self, stage) {
            (Self::ResolveOnly | Self::ResolveThenVerify(_), WorkStage::Resolve) => true,
            (Self::VerifyOnly(capability), WorkStage::Verify(evidence)) => {
                capability.permits(evidence.verify_class())
            }
            _ => false,
        }
    }

    const fn permits_active(self, stage: &WorkStage) -> bool {
        self.permits_checkout(stage)
            || match (self, stage) {
                (Self::ResolveThenVerify(capability), WorkStage::Verify(evidence)) => {
                    capability.permits(evidence.verify_class())
                }
                _ => false,
            }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct FinishedWorkCapability {
    pub(super) capability: WorkCapability,
    pub(super) result: WorkResult,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct DirectCapability {
    pub(super) id: CapabilityId,
    pub(super) request: DirectRequestId,
    pub(super) kind: DirectKind,
    pub(super) transaction: Transaction,
    pub(super) chain: ChainView,
    pub(super) rules: RulesId,
    pub(super) dependency_cut: ModelDependencyCut,
    pub(super) dependencies: ModelKnownDependencies,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct EffectClaim {
    pub(super) source: EffectClaimSource,
    pub(super) stamp: ApplyStamp,
    pub(super) ordinal: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct AuthorityState {
    pub(super) generation: PoolGeneration,
    pub(super) chain: ChainView,
    /// Sole chain-derived proposal projection installed atomically with
    /// `chain`; primitive per-height history remains outside tx-pool authority.
    pub(super) proposals: ProposalView,
    pub(super) rules: RulesId,
    /// Exact dependency event frontier shared with the settlement relation.
    /// It is a derived projection of owner edges and primitive availability /
    /// definitive-loss events, never a second lifecycle authority.
    pub(super) dependency_frontier: ModelEvidenceFrontier,
    pub(super) owners: BTreeMap<TxId, Owner>,
    /// Capacity-charged immutable publication batches in commit order.
    pub(super) effects: VecDeque<EffectRecord>,
    /// Rebuildable reset publication. A newer reset subsumes the previous
    /// reset without consuming charged journal capacity.
    pub(super) latest_generation_reset: Option<EffectRecord>,
    pub(super) peer_bans: BTreeMap<PeerId, PeerBanRecord>,
    pub(super) last_apply: ApplyStamp,
    pub(super) next_version: u16,
    pub(super) next_arrival: u16,
    pub(super) replacement_policy: ModelReplacementPolicy,
    pub(super) limits: ModelLimits,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct LinearState {
    pub(super) work: BTreeMap<CapabilityId, WorkCapability>,
    pub(super) finished_work: BTreeMap<CapabilityId, FinishedWorkCapability>,
    pub(super) direct_work: BTreeMap<CapabilityId, DirectCapability>,
    pub(super) free_compute_permits: u16,
    pub(super) next_capability: u16,
    pub(super) effect_claim: Option<EffectClaim>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct Omega {
    pub(super) authority: AuthorityState,
    pub(super) linear: LinearState,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum ModelInvariantError {
    CounterOrder,
    DependencyCutOrder,
    OwnerKey,
    OwnerChargeOverflow,
    OwnerResourceLimit,
    RetainedResourceLimit,
    AcceptedResourceLimit,
    ReplacementHistoryResourceLimit,
    RemotePeerResourceLimit,
    DuplicateOwnerVersion,
    DuplicateProposalId,
    InvalidMissingDependencies,
    InvalidTransactionTimeFacts,
    InvalidTransactionDependencyFacts,
    InvalidStoredEvidence,
    InvalidProposalStatusReceipt,
    InvalidReplacementHistory,
    AcceptedDoubleSpend,
    DuplicateAcceptedOutput,
    StaleChainOrigin,
    AcceptedCausalCycle,
    MissingPoolParent,
    InvalidPoolParentOutput,
    InvalidWorkCapability,
    ComputingWithoutCapability,
    DuplicateCurrentCapability,
    InvalidFinishedCapability,
    RetainedWorkerCapacity,
    CapabilityKey,
    DuplicateDirectRequest,
    CapabilityPermitConservation,
    EffectCapacity,
    EffectOrder,
    EffectClaim,
    PeerBanBound,
    PeerBanOrder,
}

impl Omega {
    pub(super) fn new(limits: ValidatedLimits, view: ViewId, rules: RulesId) -> Self {
        Self::new_with_replacement_policy(
            limits,
            view,
            rules,
            ModelReplacementPolicy::default_enabled(),
        )
    }

    pub(super) fn new_with_replacement_policy(
        limits: ValidatedLimits,
        view: ViewId,
        rules: RulesId,
        replacement_policy: ModelReplacementPolicy,
    ) -> Self {
        let limits = limits.get();
        Self {
            authority: AuthorityState {
                generation: PoolGeneration(0),
                chain: ChainView::initial(view),
                proposals: ProposalView::empty(),
                rules,
                dependency_frontier: ModelEvidenceFrontier::default(),
                owners: BTreeMap::new(),
                effects: VecDeque::new(),
                latest_generation_reset: None,
                peer_bans: BTreeMap::new(),
                last_apply: ApplyStamp(0),
                next_version: 1,
                next_arrival: 1,
                replacement_policy,
                limits,
            },
            linear: LinearState {
                work: BTreeMap::new(),
                finished_work: BTreeMap::new(),
                direct_work: BTreeMap::new(),
                free_compute_permits: limits.compute_permits,
                next_capability: 1,
                effect_claim: None,
            },
        }
    }

    pub(super) fn dependency_cuts(&self) -> BTreeSet<ModelDependencyCut> {
        fn record_evidence(
            evidence: &ResolvedEvidenceAtCut,
            cuts: &mut BTreeSet<ModelDependencyCut>,
        ) {
            cuts.insert(evidence.dependency_cut);
        }

        fn record_capability(capability: &WorkCapability, cuts: &mut BTreeSet<ModelDependencyCut>) {
            cuts.insert(capability.dependency_cut);
            if let WorkStage::Verify(evidence) = &capability.stage {
                record_evidence(evidence, cuts);
            }
        }

        let mut cuts = self.authority.dependency_frontier.dependency_cuts();
        for owner in self.authority.owners.values() {
            match &owner.location {
                OwnerLocation::Retained(RetainedOwner { phase, .. }) => match phase {
                    RetainedPhase::Queued(WorkStage::Verify(evidence))
                    | RetainedPhase::Ready(evidence) => record_evidence(evidence, &mut cuts),
                    RetainedPhase::Computing(active) => {
                        cuts.insert(active.dependency_cut);
                    }
                    RetainedPhase::Queued(WorkStage::Resolve) | RetainedPhase::Waiting { .. } => {}
                },
                OwnerLocation::Accepted { evidence, .. } => record_evidence(evidence, &mut cuts),
                OwnerLocation::ReplacementHistory { .. } => {}
            }
        }
        for capability in self.linear.work.values() {
            record_capability(capability, &mut cuts);
        }
        for finished in self.linear.finished_work.values() {
            record_capability(&finished.capability, &mut cuts);
        }
        cuts.extend(
            self.linear
                .direct_work
                .values()
                .map(|capability| capability.dependency_cut),
        );
        cuts
    }

    pub(super) fn remap_dependency_cuts(
        &mut self,
        mapping: &BTreeMap<ModelDependencyCut, ModelDependencyCut>,
    ) -> bool {
        fn remap(
            cut: &mut ModelDependencyCut,
            mapping: &BTreeMap<ModelDependencyCut, ModelDependencyCut>,
        ) {
            if cut.0 != 0 {
                *cut = mapping[cut];
            }
        }

        fn remap_evidence(
            evidence: &mut ResolvedEvidenceAtCut,
            mapping: &BTreeMap<ModelDependencyCut, ModelDependencyCut>,
        ) {
            remap(&mut evidence.dependency_cut, mapping);
        }

        fn remap_capability(
            capability: &mut WorkCapability,
            mapping: &BTreeMap<ModelDependencyCut, ModelDependencyCut>,
        ) {
            remap(&mut capability.dependency_cut, mapping);
            if let WorkStage::Verify(evidence) = &mut capability.stage {
                remap_evidence(evidence, mapping);
            }
        }

        let cuts = self.dependency_cuts();
        let mut previous = None;
        for cut in cuts.iter().copied().filter(|cut| cut.0 != 0) {
            let Some(mapped) = mapping.get(&cut).copied() else {
                return false;
            };
            if previous.is_some_and(|prior| prior > mapped) {
                return false;
            }
            previous = Some(mapped);
        }

        let mut next = self.clone();
        if !next
            .authority
            .dependency_frontier
            .remap_dependency_cuts(mapping)
        {
            return false;
        }
        for owner in next.authority.owners.values_mut() {
            match &mut owner.location {
                OwnerLocation::Retained(RetainedOwner { phase, .. }) => match phase {
                    RetainedPhase::Queued(WorkStage::Verify(evidence))
                    | RetainedPhase::Ready(evidence) => remap_evidence(evidence, mapping),
                    RetainedPhase::Computing(active) => {
                        remap(&mut active.dependency_cut, mapping);
                    }
                    RetainedPhase::Queued(WorkStage::Resolve) | RetainedPhase::Waiting { .. } => {}
                },
                OwnerLocation::Accepted { evidence, .. } => remap_evidence(evidence, mapping),
                OwnerLocation::ReplacementHistory { .. } => {}
            }
        }
        for capability in next.linear.work.values_mut() {
            remap_capability(capability, mapping);
        }
        for finished in next.linear.finished_work.values_mut() {
            remap_capability(&mut finished.capability, mapping);
        }
        for capability in next.linear.direct_work.values_mut() {
            remap(&mut capability.dependency_cut, mapping);
        }
        *self = next;
        true
    }

    pub(super) fn owner_usage(&self) -> Result<ResourceVector, ModelInvariantError> {
        self.authority
            .owners
            .values()
            .try_fold(ResourceVector::ZERO, |total, owner| {
                let charge = owner
                    .transaction
                    .charge()
                    .ok_or(ModelInvariantError::OwnerChargeOverflow)?;
                total
                    .checked_add(charge)
                    .ok_or(ModelInvariantError::OwnerChargeOverflow)
            })
    }

    pub(super) fn proposal_owner(&self, proposal: ProposalId) -> Option<TxId> {
        self.authority
            .owners
            .iter()
            .find_map(|(id, owner)| (owner.transaction.proposal == proposal).then_some(*id))
    }

    pub(super) fn proposal_status(&self, transaction: &Transaction) -> AcceptedStatus {
        self.proposal_status_receipt(transaction).value()
    }

    pub(super) fn proposal_status_receipt(
        &self,
        transaction: &Transaction,
    ) -> ProposalStatusReceipt {
        match self.authority.owners.get(&transaction.id) {
            Some(Owner {
                transaction: owned,
                location: OwnerLocation::Accepted { proposal, .. },
                ..
            }) if owned == transaction => *proposal,
            _ => self.authority.proposals.status(transaction.proposal),
        }
    }

    pub(super) fn retained_usage(&self) -> Result<ResourceVector, ModelInvariantError> {
        self.usage_matching(|owner| matches!(owner.location, OwnerLocation::Retained(_)))
    }

    pub(super) fn accepted_usage(&self) -> Result<ResourceVector, ModelInvariantError> {
        self.usage_matching(|owner| matches!(owner.location, OwnerLocation::Accepted { .. }))
    }

    pub(super) fn replacement_history_usage(&self) -> Result<ResourceVector, ModelInvariantError> {
        self.usage_matching(|owner| {
            matches!(owner.location, OwnerLocation::ReplacementHistory { .. })
        })
    }

    pub(super) fn remote_peer_usage(
        &self,
        peer: PeerId,
    ) -> Result<ResourceVector, ModelInvariantError> {
        self.usage_matching(|owner| {
            owner.retained_source().and_then(Source::ingress_peer) == Some(peer)
        })
    }

    fn usage_matching(
        &self,
        include: impl Fn(&Owner) -> bool,
    ) -> Result<ResourceVector, ModelInvariantError> {
        self.authority
            .owners
            .values()
            .filter(|owner| include(owner))
            .try_fold(ResourceVector::ZERO, |total, owner| {
                let charge = owner
                    .transaction
                    .charge()
                    .ok_or(ModelInvariantError::OwnerChargeOverflow)?;
                total
                    .checked_add(charge)
                    .ok_or(ModelInvariantError::OwnerChargeOverflow)
            })
    }

    pub(super) fn effect_batch_shape(effects: &[LogicalEffect]) -> Option<(u16, u32)> {
        EffectRecord::batch_shape(effects)
    }

    pub(super) fn can_append_effects(&self, class: EffectClass, effects: &[LogicalEffect]) -> bool {
        if effects.is_empty() {
            return true;
        }
        if !self.can_append_effects_against_empty_bound(class, effects) {
            return false;
        }
        if matches!(effects, [LogicalEffect::GenerationReset]) {
            return true;
        }
        let Some((_, bytes)) = Self::effect_batch_shape(effects) else {
            return false;
        };
        self.effect_usage()
            .and_then(|usage| usage.checked_charge(class, bytes))
            .is_some_and(|usage| usage.fits(self.authority.limits.effects))
    }

    /// Injects one otherwise valid committed batch for capacity-bound model
    /// fixtures. Tests use this instead of hand-building journal records, so
    /// batch metadata and stamp ordering cannot drift from the model law.
    pub(super) fn append_effect_fixture(
        &mut self,
        class: EffectClass,
        effects: Vec<LogicalEffect>,
    ) -> bool {
        if effects.is_empty() || !self.can_append_effects(class, &effects) {
            return false;
        }
        let Some(stamp) = self.authority.last_apply.0.checked_add(1).map(ApplyStamp) else {
            return false;
        };
        let Some(records) = EffectRecord::from_batch(stamp, class, effects) else {
            return false;
        };
        if !self.install_effect_records(records) {
            return false;
        }
        self.authority.last_apply = stamp;
        true
    }

    fn can_append_effects_against_empty_bound(
        &self,
        class: EffectClass,
        effects: &[LogicalEffect],
    ) -> bool {
        if matches!(effects, [LogicalEffect::GenerationReset]) {
            return class == EffectClass::Critical;
        }
        if effects.is_empty()
            || effects
                .iter()
                .any(|effect| matches!(effect, LogicalEffect::GenerationReset))
        {
            return effects.is_empty();
        }
        let Some((count, bytes)) = Self::effect_batch_shape(effects) else {
            return false;
        };
        let bound = self.authority.limits.effects.bound(class);
        count <= bound.effects && bytes <= bound.bytes
    }

    pub(super) fn effect_usage(&self) -> Option<EffectUsage> {
        let mut usage = EffectUsage::default();
        let mut previous_batch = None;
        for effect in &self.authority.effects {
            if previous_batch == Some(effect.stamp) {
                continue;
            }
            previous_batch = Some(effect.stamp);
            usage = usage.checked_charge(effect.class, effect.batch_bytes)?;
        }
        Some(usage)
    }

    pub(super) fn next_effect_record(&self) -> Option<(EffectClaimSource, &EffectRecord)> {
        match (
            self.authority.effects.front(),
            self.authority.latest_generation_reset.as_ref(),
        ) {
            (Some(queued), Some(reset)) if reset.stamp < queued.stamp => {
                Some((EffectClaimSource::GenerationReset, reset))
            }
            (Some(queued), _) => Some((EffectClaimSource::Queued, queued)),
            (None, Some(reset)) => Some((EffectClaimSource::GenerationReset, reset)),
            (None, None) => None,
        }
    }

    pub(super) fn has_pending_effects(&self) -> bool {
        !self.authority.effects.is_empty() || self.authority.latest_generation_reset.is_some()
    }

    pub(super) fn install_effect_records(&mut self, mut records: Vec<EffectRecord>) -> bool {
        let Some(first) = records.first() else {
            return false;
        };
        let latest_stamp = self
            .authority
            .effects
            .back()
            .map(|effect| effect.stamp)
            .into_iter()
            .chain(
                self.authority
                    .latest_generation_reset
                    .as_ref()
                    .map(|reset| reset.stamp),
            )
            .max();
        if latest_stamp.is_some_and(|latest| latest >= first.stamp) {
            return false;
        }
        if matches!(first.logical, LogicalEffect::GenerationReset) {
            if records.len() != 1 {
                return false;
            }
            let Some(reset) = records.pop() else {
                return false;
            };
            self.authority.latest_generation_reset = Some(reset);
        } else {
            self.authority.effects.extend(records);
        }
        true
    }

    pub(super) fn check_invariants(&self) -> Result<(), ModelInvariantError> {
        if self
            .dependency_cuts()
            .iter()
            .any(|cut| cut.0 > self.authority.last_apply.0)
        {
            return Err(ModelInvariantError::DependencyCutOrder);
        }
        let usage = self.owner_usage()?;
        if !usage.fits(self.authority.limits.owners) {
            return Err(ModelInvariantError::OwnerResourceLimit);
        }
        if !self.retained_usage()?.fits(self.authority.limits.retained) {
            return Err(ModelInvariantError::RetainedResourceLimit);
        }
        if !self.accepted_usage()?.fits(self.authority.limits.accepted) {
            return Err(ModelInvariantError::AcceptedResourceLimit);
        }
        if !self
            .replacement_history_usage()?
            .fits(self.authority.limits.replacement_history)
        {
            return Err(ModelInvariantError::ReplacementHistoryResourceLimit);
        }
        for peer in self
            .authority
            .owners
            .values()
            .filter_map(|owner| owner.retained_source().and_then(Source::ingress_peer))
            .collect::<BTreeSet<_>>()
        {
            if !self
                .remote_peer_usage(peer)?
                .fits(self.authority.limits.remote_per_peer)
            {
                return Err(ModelInvariantError::RemotePeerResourceLimit);
            }
        }

        let mut versions = BTreeSet::new();
        let mut proposals = BTreeSet::new();
        let mut spenders = BTreeMap::new();
        let mut accepted_producers = BTreeMap::new();
        for (id, owner) in &self.authority.owners {
            if !matches!(owner.location, OwnerLocation::Accepted { .. }) {
                continue;
            }
            for output in &owner.transaction.outputs {
                if accepted_producers.insert(*output, *id).is_some() {
                    return Err(ModelInvariantError::DuplicateAcceptedOutput);
                }
            }
        }
        for (id, owner) in &self.authority.owners {
            if *id != owner.transaction.id {
                return Err(ModelInvariantError::OwnerKey);
            }
            if !owner.transaction.has_valid_time_shape() {
                return Err(ModelInvariantError::InvalidTransactionTimeFacts);
            }
            if !owner.transaction.has_valid_dependency_shape() {
                return Err(ModelInvariantError::InvalidTransactionDependencyFacts);
            }
            if !versions.insert(owner.version) {
                return Err(ModelInvariantError::DuplicateOwnerVersion);
            }
            if !proposals.insert(owner.transaction.proposal) {
                return Err(ModelInvariantError::DuplicateProposalId);
            }
            match &owner.location {
                OwnerLocation::Retained(RetainedOwner {
                    phase: RetainedPhase::Waiting { missing, .. },
                    ..
                }) if !missing.is_for(&owner.transaction) => {
                    return Err(ModelInvariantError::InvalidMissingDependencies);
                }
                OwnerLocation::Retained(RetainedOwner {
                    phase:
                        RetainedPhase::Queued(WorkStage::Verify(evidence))
                        | RetainedPhase::Ready(evidence),
                    ..
                }) if !evidence.has_transaction_shape(&owner.transaction, self.authority.rules) => {
                    return Err(ModelInvariantError::InvalidStoredEvidence);
                }
                OwnerLocation::Accepted {
                    evidence, proposal, ..
                } => {
                    if !evidence.has_transaction_shape(&owner.transaction, self.authority.rules) {
                        return Err(ModelInvariantError::InvalidStoredEvidence);
                    }
                    if *proposal != self.authority.proposals.status(owner.transaction.proposal) {
                        return Err(ModelInvariantError::InvalidProposalStatusReceipt);
                    }
                    for (cell, origin) in &evidence.input_origins {
                        if spenders.insert(*cell, *id).is_some() {
                            return Err(ModelInvariantError::AcceptedDoubleSpend);
                        }
                        match origin {
                            InputOrigin::Chain if accepted_producers.contains_key(cell) => {
                                return Err(ModelInvariantError::StaleChainOrigin);
                            }
                            InputOrigin::Pool(parent) => {
                                if parent == id {
                                    return Err(ModelInvariantError::AcceptedCausalCycle);
                                }
                                let Some(parent_owner) = self.authority.owners.get(parent) else {
                                    return Err(ModelInvariantError::MissingPoolParent);
                                };
                                if !matches!(parent_owner.location, OwnerLocation::Accepted { .. })
                                {
                                    return Err(ModelInvariantError::MissingPoolParent);
                                }
                                if accepted_producers.get(cell) != Some(parent)
                                    || !parent_owner.transaction.outputs.contains(cell)
                                {
                                    return Err(ModelInvariantError::InvalidPoolParentOutput);
                                }
                            }
                            InputOrigin::Chain => {}
                        }
                    }
                    for (cell, origin) in &evidence.dep_origins {
                        match origin {
                            InputOrigin::Chain if accepted_producers.contains_key(cell) => {
                                return Err(ModelInvariantError::StaleChainOrigin);
                            }
                            InputOrigin::Pool(parent) => {
                                if parent == id {
                                    return Err(ModelInvariantError::AcceptedCausalCycle);
                                }
                                let Some(parent_owner) = self.authority.owners.get(parent) else {
                                    return Err(ModelInvariantError::MissingPoolParent);
                                };
                                if !matches!(parent_owner.location, OwnerLocation::Accepted { .. })
                                {
                                    return Err(ModelInvariantError::MissingPoolParent);
                                }
                                if accepted_producers.get(cell) != Some(parent)
                                    || !parent_owner.transaction.outputs.contains(cell)
                                {
                                    return Err(ModelInvariantError::InvalidPoolParentOutput);
                                }
                            }
                            InputOrigin::Chain => {}
                        }
                    }
                }
                OwnerLocation::ReplacementHistory { missing }
                    if !missing.is_for(&owner.transaction) =>
                {
                    return Err(ModelInvariantError::InvalidReplacementHistory);
                }
                _ => {}
            }
        }
        let mut accepted_indegree = BTreeMap::new();
        let mut accepted_children = BTreeMap::<TxId, BTreeSet<TxId>>::new();
        for (id, owner) in &self.authority.owners {
            let OwnerLocation::Accepted { evidence, .. } = &owner.location else {
                continue;
            };
            let parents = evidence
                .input_origins
                .values()
                .chain(evidence.dep_origins.values())
                .filter_map(|origin| match origin {
                    InputOrigin::Pool(parent) => Some(*parent),
                    InputOrigin::Chain => None,
                })
                .collect::<BTreeSet<_>>();
            accepted_indegree.insert(*id, parents.len());
            for parent in parents {
                accepted_children.entry(parent).or_default().insert(*id);
            }
        }
        let mut acyclic_frontier = accepted_indegree
            .iter()
            .filter_map(|(id, degree)| (*degree == 0).then_some(*id))
            .collect::<VecDeque<_>>();
        let mut acyclic_count = 0usize;
        while let Some(parent) = acyclic_frontier.pop_front() {
            acyclic_count = acyclic_count
                .checked_add(1)
                .ok_or(ModelInvariantError::AcceptedCausalCycle)?;
            let Some(children) = accepted_children.get(&parent) else {
                continue;
            };
            for child in children {
                let Some(degree) = accepted_indegree.get_mut(child) else {
                    return Err(ModelInvariantError::MissingPoolParent);
                };
                *degree = degree
                    .checked_sub(1)
                    .ok_or(ModelInvariantError::AcceptedCausalCycle)?;
                if *degree == 0 {
                    acyclic_frontier.push_back(*child);
                }
            }
        }
        if acyclic_count != accepted_indegree.len() {
            return Err(ModelInvariantError::AcceptedCausalCycle);
        }
        if self.authority.owners.values().any(|owner| {
            owner.version.0 >= self.authority.next_version
                || owner.arrival.0 >= self.authority.next_arrival
        }) {
            return Err(ModelInvariantError::CounterOrder);
        }

        for owner in self.authority.owners.values() {
            let OwnerLocation::Retained(RetainedOwner {
                phase: RetainedPhase::Computing(active),
                ..
            }) = &owner.location
            else {
                continue;
            };
            let current = self
                .linear
                .work
                .values()
                .chain(
                    self.linear
                        .finished_work
                        .values()
                        .map(|finished| &finished.capability),
                )
                .filter(|capability| {
                    capability.transaction == owner.transaction.id
                        && capability.version == owner.version
                        && capability.permit() == active.permit
                        && capability.dependency_cut == active.dependency_cut
                })
                .count();
            match current {
                0 => return Err(ModelInvariantError::ComputingWithoutCapability),
                1 => {}
                _ => return Err(ModelInvariantError::DuplicateCurrentCapability),
            }
        }

        for (key, capability) in &self.linear.work {
            if *key != capability.id || capability.id.0 >= self.linear.next_capability {
                return Err(ModelInvariantError::CapabilityKey);
            }
            if !capability.is_compatible() {
                return Err(ModelInvariantError::InvalidWorkCapability);
            }
            let Some(owner) = self.authority.owners.get(&capability.transaction) else {
                continue;
            };
            if owner.version != capability.version
                || capability.chain != self.authority.chain
                || capability.rules != self.authority.rules
            {
                continue;
            }
            if !matches!(
                &owner.location,
                OwnerLocation::Retained(RetainedOwner {
                    phase: RetainedPhase::Computing(active),
                    ..
                }) if active.permit == capability.permit()
                    && active.dependency_cut == capability.dependency_cut
            ) {
                return Err(ModelInvariantError::DuplicateCurrentCapability);
            }
            if let WorkStage::Verify(evidence) = capability.stage()
                && !evidence.is_for(
                    &owner.transaction,
                    self.authority.chain,
                    self.authority.rules,
                )
            {
                return Err(ModelInvariantError::InvalidStoredEvidence);
            }
        }

        for (key, finished) in &self.linear.finished_work {
            let capability = &finished.capability;
            if *key != capability.id
                || capability.id.0 >= self.linear.next_capability
                || self.linear.work.contains_key(key)
            {
                return Err(ModelInvariantError::CapabilityKey);
            }
            if !capability.is_compatible() {
                return Err(ModelInvariantError::InvalidWorkCapability);
            }
            let Some(owner) = self.authority.owners.get(&capability.transaction) else {
                continue;
            };
            if owner.version != capability.version
                || capability.chain != self.authority.chain
                || capability.rules != self.authority.rules
            {
                continue;
            }
            if !matches!(
                &owner.location,
                OwnerLocation::Retained(RetainedOwner {
                    phase: RetainedPhase::Computing(active),
                    ..
                }) if active.permit == capability.permit()
                    && active.dependency_cut == capability.dependency_cut
            ) {
                return Err(ModelInvariantError::InvalidFinishedCapability);
            }
            if let WorkStage::Verify(evidence) = capability.stage()
                && !evidence.is_for(
                    &owner.transaction,
                    self.authority.chain,
                    self.authority.rules,
                )
            {
                return Err(ModelInvariantError::InvalidStoredEvidence);
            }
        }

        let retained_worker_slots = self
            .linear
            .work
            .len()
            .checked_add(self.linear.finished_work.len())
            .ok_or(ModelInvariantError::RetainedWorkerCapacity)?;
        if retained_worker_slots > usize::from(self.authority.limits.compute_permits) {
            return Err(ModelInvariantError::RetainedWorkerCapacity);
        }

        let mut direct_requests = BTreeSet::new();
        for (key, capability) in &self.linear.direct_work {
            if *key != capability.id
                || capability.id.0 >= self.linear.next_capability
                || self.linear.work.contains_key(key)
                || self.linear.finished_work.contains_key(key)
            {
                return Err(ModelInvariantError::CapabilityKey);
            }
            if !direct_requests.insert(capability.request) {
                return Err(ModelInvariantError::DuplicateDirectRequest);
            }
            if !capability.transaction.has_valid_time_shape() {
                return Err(ModelInvariantError::InvalidTransactionTimeFacts);
            }
            if capability.chain != self.authority.chain || capability.rules != self.authority.rules
            {
                continue;
            }
        }

        let held = self
            .linear
            .work
            .len()
            .checked_add(self.linear.direct_work.len())
            .and_then(|held| u16::try_from(held).ok())
            .ok_or(ModelInvariantError::CapabilityPermitConservation)?;
        if self.linear.free_compute_permits.checked_add(held)
            != Some(self.authority.limits.compute_permits)
        {
            return Err(ModelInvariantError::CapabilityPermitConservation);
        }

        let Some(effect_usage) = self.effect_usage() else {
            return Err(ModelInvariantError::EffectCapacity);
        };
        if !effect_usage.fits(self.authority.limits.effects) {
            return Err(ModelInvariantError::EffectCapacity);
        }
        let mut previous = None;
        let effects = self.authority.effects.iter().collect::<Vec<_>>();
        for effect in &effects {
            let key = (effect.stamp, effect.ordinal);
            if effect.stamp.0 == 0
                || effect.stamp > self.authority.last_apply
                || previous.is_some_and(|previous_key| previous_key >= key)
            {
                return Err(ModelInvariantError::EffectOrder);
            }
            previous = Some(key);
        }
        let mut batch_start = 0usize;
        while batch_start < effects.len() {
            let first = effects[batch_start];
            let mut batch_end = batch_start + 1;
            while batch_end < effects.len() && effects[batch_end].stamp == first.stamp {
                batch_end += 1;
            }
            let batch = &effects[batch_start..batch_end];
            let remaining_bytes = batch.iter().try_fold(0u32, |used, record| {
                used.checked_add(record.logical.charge_bytes()?)
            });
            let Some(remaining_bytes) = remaining_bytes else {
                return Err(ModelInvariantError::EffectCapacity);
            };
            let bound = self.authority.limits.effects.bound(first.class);
            let last_ordinal = batch.last().map(|record| record.ordinal);
            if first.ordinal > 0 && batch_start != 0
                || batch.iter().enumerate().any(|(offset, record)| {
                    u16::try_from(offset)
                        .ok()
                        .and_then(|offset| first.ordinal.checked_add(offset))
                        != Some(record.ordinal)
                        || record.class != first.class
                        || record.batch_effects != first.batch_effects
                        || record.batch_bytes != first.batch_bytes
                })
                || last_ordinal.and_then(|ordinal| ordinal.checked_add(1))
                    != Some(first.batch_effects)
                || first.batch_effects > bound.effects
                || first.batch_bytes > bound.bytes
                || remaining_bytes > first.batch_bytes
                || first.ordinal == 0
                    && (u16::try_from(batch.len()).ok() != Some(first.batch_effects)
                        || remaining_bytes != first.batch_bytes)
                || matches!(first.logical, LogicalEffect::GenerationReset)
            {
                return Err(ModelInvariantError::EffectOrder);
            }
            batch_start = batch_end;
        }
        if let Some(reset) = &self.authority.latest_generation_reset
            && (reset.stamp.0 == 0
                || reset.stamp > self.authority.last_apply
                || reset.ordinal != 0
                || reset.class != EffectClass::Critical
                || reset.batch_effects != 1
                || reset.batch_bytes != 0
                || !matches!(reset.logical, LogicalEffect::GenerationReset)
                || effects.iter().any(|effect| effect.stamp == reset.stamp))
        {
            return Err(ModelInvariantError::EffectOrder);
        }
        if let Some(claim) = self.linear.effect_claim {
            let valid = match claim.source {
                EffectClaimSource::Queued => self.authority.effects.front().is_some_and(|effect| {
                    (effect.stamp, effect.ordinal) == (claim.stamp, claim.ordinal)
                }),
                EffectClaimSource::GenerationReset => {
                    self.authority
                        .latest_generation_reset
                        .as_ref()
                        .is_some_and(|reset| {
                            claim.ordinal == 0 && reset.ordinal == 0 && reset.stamp >= claim.stamp
                        })
                        && self
                            .authority
                            .effects
                            .front()
                            .is_none_or(|queued| queued.stamp > claim.stamp)
                }
            };
            if !valid {
                return Err(ModelInvariantError::EffectClaim);
            }
        }
        if u16::try_from(self.authority.peer_bans.len())
            .ok()
            .is_none_or(|count| count > self.authority.limits.peer_ban_fences)
        {
            return Err(ModelInvariantError::PeerBanBound);
        }
        let mut ban_orders = BTreeSet::new();
        if self.authority.peer_bans.values().any(|record| {
            record.order > self.authority.last_apply || !ban_orders.insert(record.order)
        }) {
            return Err(ModelInvariantError::PeerBanOrder);
        }
        Ok(())
    }

    pub(super) fn ready_order(&self) -> Vec<TxId> {
        let mut ready = self
            .authority
            .owners
            .values()
            .filter_map(|owner| {
                matches!(
                    owner.location,
                    OwnerLocation::Retained(RetainedOwner {
                        phase: RetainedPhase::Ready(_),
                        ..
                    })
                )
                .then_some(ReadyKey {
                    source_priority: owner.retained_source()?.priority(),
                    fee: owner.transaction.cost.fee(),
                    serialized_bytes: owner.transaction.cost.serialized_bytes(),
                    arrival: owner.arrival,
                    transaction: owner.transaction.id,
                    version: owner.version,
                })
            })
            .collect::<Vec<_>>();
        ready.sort_unstable();
        ready.into_iter().map(|key| key.transaction).collect()
    }

    pub(super) fn queued_order(&self) -> Vec<TxId> {
        let mut queued = self
            .authority
            .owners
            .values()
            .filter_map(|owner| {
                matches!(
                    owner.location,
                    OwnerLocation::Retained(RetainedOwner {
                        phase: RetainedPhase::Queued(_),
                        ..
                    })
                )
                .then_some((
                    owner.retained_source()?.priority(),
                    owner.arrival,
                    owner.transaction.id,
                ))
            })
            .collect::<Vec<_>>();
        queued.sort_unstable();
        queued.into_iter().map(|(_, _, id)| id).collect()
    }
}
