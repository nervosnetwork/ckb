use super::apply_seal::{ApplyToken, OwnerResourceUpdate, PreparedOwnerResourceDelta};
use super::{
    AuthorityDelta, AuthorityFault, ClockPlanReservation, DerivedOwnerDelta, PlanError,
    PreparedApply, TxPoolAuthority,
};
use crate::authority::{
    effect::{
        CommittedAcceptance, CommittedEffect, CommittedRejection, CommittedRemoteIngressRelease,
        EffectDelta, EffectPolicy, OrderedEffectAppendError, RejectionAudience,
    },
    ingress::{
        RemoteIngressPressure, RetainedAdmissionBatch, RetainedIngressAttempt, RetainedIngressKind,
    },
    resources::{ChargeRecord, OrderedResourceProjection, ResourceBatchPlan, ResourceError},
    scheduler::SchedulerBatchDelta,
    state::{
        AdmissionBasis, AuthorityClocks, OwnedTx, PreAcceptedEntry, PreAcceptedPhase,
        PreAcceptedSource, ProposalBase, ProposalId, QueuedWork, RawTxHash, TxRecord,
        ValidatedAdmission,
    },
};
use std::collections::HashMap;
use std::time::Instant;

struct RetainedIngressUpdate {
    key: RawTxHash,
    after: OwnedTx,
}

pub(super) struct RetainedIngressDelta {
    updates: Vec<RetainedIngressUpdate>,
    owners: DerivedOwnerDelta,
    resources: ResourceBatchPlan,
    scheduler: SchedulerBatchDelta,
    dependency: crate::authority::dependency::DependencyBatchDelta,
    effect: EffectDelta,
    clocks: AuthorityClocks,
    retired: super::RetiredOwners,
}

pub(super) fn apply_retained_ingress(
    authority: &mut TxPoolAuthority,
    token: &ApplyToken,
    delta: RetainedIngressDelta,
) -> super::ApplyRetirement {
    let mut retired = delta.retired;
    let status_counts = crate::authority::shard::ShardStatusCountPlan::default();
    let support = authority.entries.owner_resource_write_support(
        delta.updates.iter().map(|update| &update.key),
        &status_counts,
        delta.resources.shard_plan(),
    );
    let updates = delta
        .updates
        .into_iter()
        .map(|update| OwnerResourceUpdate::new(update.key, Some(update.after)));
    authority.commit_owner_resources(
        token,
        PreparedOwnerResourceDelta::batch(updates, delta.resources, status_counts, support),
        &mut retired,
    );
    let authority = authority.write(token);
    authority.indexes.apply(delta.owners.indexes);
    authority.source_versions.apply(delta.owners.sources);
    authority.scheduler.apply_batch(delta.scheduler);
    authority.dependencies.apply_batch(delta.dependency);
    let retired_effect = authority.effects.apply(delta.effect);
    let _reserved_clock_high_water = delta.clocks;
    super::ApplyRetirement {
        async_process_observations: super::AsyncProcessObservations::None,
        removals: Vec::new(),
        retired,
        retired_effect,
        retired_generation: None,
    }
}

/// Prepared canonical ingress prefix. `consumed` ends before the first item
/// whose indivisible effect could not fit; no item can be split across the
/// committed and retained portions. The runtime keeps the original move-only
/// batch and uses this count to return the exact suffix after Apply.
#[must_use = "a prepared retained-ingress prefix must be applied exactly once"]
pub(in crate::authority) struct PreparedRetainedAdmissionBatch<'authority> {
    plan: Option<PreparedApply<'authority>>,
    consumed: usize,
}

#[expect(
    clippy::large_enum_variant,
    reason = "the transient Applied variant carries the move-only post-commit capability without adding one heap allocation to every successful ingress batch"
)]
pub(in crate::authority) enum CommittedRetainedAdmissionBatch {
    Unchanged {
        consumed: usize,
    },
    Applied {
        retirement: super::CommittedDelta,
        consumed: usize,
    },
}

impl PreparedRetainedAdmissionBatch<'_> {
    pub(in crate::authority) fn consumed(&self) -> usize {
        self.consumed
    }

    pub(in crate::authority) fn apply(self) -> CommittedRetainedAdmissionBatch {
        match self.plan {
            Some(plan) => CommittedRetainedAdmissionBatch::Applied {
                retirement: plan.apply(),
                consumed: self.consumed,
            },
            None => CommittedRetainedAdmissionBatch::Unchanged {
                consumed: self.consumed,
            },
        }
    }
}

struct OwnerChange {
    key: RawTxHash,
    before: Option<OwnedTx>,
    after: OwnedTx,
}

struct OwnerOverlay {
    positions: HashMap<RawTxHash, usize>,
    changes: Vec<OwnerChange>,
    proposals: HashMap<ProposalId, RawTxHash>,
}

impl OwnerOverlay {
    fn new(maximum_items: usize) -> Result<Self, PlanError> {
        let mut positions = HashMap::new();
        positions
            .try_reserve(maximum_items)
            .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
        let mut changes = Vec::new();
        changes
            .try_reserve(maximum_items)
            .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
        let mut proposals = HashMap::new();
        proposals
            .try_reserve(maximum_items)
            .map_err(|_| PlanError::Backpressure(super::Backpressure::Allocation))?;
        Ok(Self {
            positions,
            changes,
            proposals,
        })
    }

    fn current(
        &self,
        authority: &TxPoolAuthority,
        key: &RawTxHash,
    ) -> Result<Option<OwnedTx>, PlanError> {
        let Some(position) = self.positions.get(key).copied() else {
            return Ok(authority.entries.get(key).as_deref().cloned());
        };
        self.changes
            .get(position)
            .map(|change| Some(change.after.clone()))
            .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))
    }

    fn proposal_owner(
        &self,
        authority: &TxPoolAuthority,
        proposal: &ProposalId,
    ) -> Option<RawTxHash> {
        self.proposals
            .get(proposal)
            .cloned()
            .or_else(|| authority.indexes.proposal_owner(proposal))
    }

    fn replace(
        &mut self,
        authority: &TxPoolAuthority,
        key: RawTxHash,
        after: OwnedTx,
    ) -> Result<(), PlanError> {
        if let Some(position) = self.positions.get(&key).copied() {
            let Some(change) = self.changes.get_mut(position) else {
                return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
            };
            change.after = after;
            return Ok(());
        }
        let position = self.changes.len();
        let before = authority.entries.get(&key).as_deref().cloned();
        self.positions.insert(key.clone(), position);
        self.proposals
            .insert(after.record().identity.proposal.clone(), key.clone());
        self.changes.push(OwnerChange { key, before, after });
        Ok(())
    }
}

struct BatchScratch {
    owners: OwnerOverlay,
    resources: OrderedResourceProjection,
    clocks: ClockPlanReservation,
}

struct ItemDecision {
    effect: Option<CommittedEffect>,
}

impl TxPoolAuthority {
    pub(in crate::authority) fn plan_retained_admission_batch(
        &mut self,
        batch: &RetainedAdmissionBatch,
    ) -> Result<PreparedRetainedAdmissionBatch<'_>, PlanError> {
        self.effects.ensure_open()?;
        let kind = batch.kind();
        let item_count = batch.len();
        let malformed_position = batch
            .attempts()
            .position(RetainedIngressAttempt::is_malformed_remote);
        if let Some(position) = malformed_position {
            let culprit = batch
                .attempts()
                .nth(position)
                .ok_or(PlanError::Fault(AuthorityFault::MembershipProjection))?;
            let RetainedIngressAttempt::Rejected(culprit) = culprit else {
                return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
            };
            let plan = self.plan_retained_ingress_rejection(culprit.clone())?;
            return Ok(PreparedRetainedAdmissionBatch {
                plan: Some(plan),
                consumed: item_count,
            });
        }

        let policy = match kind {
            RetainedIngressKind::Remote(_) => EffectPolicy::Remote,
            RetainedIngressKind::Proposal => EffectPolicy::Trusted,
        };
        // One batch is planned against one authority cut, including the
        // monotonic peer-fence observation. Reading the clock per item could
        // otherwise make two adjacent Remote items observe different policy
        // facts inside an allegedly atomic canonical fold.
        let planned_at = Instant::now();
        let mut effects = self.effects.ordered_publication(policy, item_count)?;
        let maximum_peers = match kind {
            // A Remote batch is peer-homogeneous and never replaces another
            // retained owner; at most its one ingress peer is materialized.
            RetainedIngressKind::Remote(_) => 1,
            // Proposal promotion may release one distinct Remote attribution
            // per member, so the exact item bound is also the peer bound.
            RetainedIngressKind::Proposal => item_count,
        };
        let mut scratch = BatchScratch {
            owners: OwnerOverlay::new(item_count)?,
            resources: self
                .resources
                .ordered_projection(&self.entries, maximum_peers)?,
            clocks: ClockPlanReservation::begin(std::sync::Arc::clone(&self.clocks)),
        };
        let mut consumed = 0usize;

        for attempt in batch.attempts() {
            // This ordering is load-bearing. Every branch which publishes an
            // effect is a no-owner disposition and leaves `scratch` unchanged;
            // owner-producing branches publish no effect. The effect prefix
            // may therefore stop before the current item without retaining a
            // hidden scratch mutation outside `consumed`.
            let decision =
                self.plan_retained_batch_item(kind, attempt, planned_at, &mut scratch)?;
            if let Some(effect) = decision.effect {
                match effects.push(effect) {
                    Ok(()) => {}
                    Err(OrderedEffectAppendError::Full) if consumed == 0 => {
                        return Err(PlanError::Backpressure(super::Backpressure::EffectCapacity));
                    }
                    Err(OrderedEffectAppendError::Full) => break,
                    Err(OrderedEffectAppendError::Projection) => {
                        return Err(PlanError::Fault(AuthorityFault::EffectProjection));
                    }
                }
            }
            consumed = consumed
                .checked_add(1)
                .ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;
        }

        let publication = effects.finish()?;
        let has_apply = !scratch.owners.changes.is_empty() || publication.is_some();
        if !has_apply {
            return Ok(PreparedRetainedAdmissionBatch {
                plan: None,
                consumed,
            });
        }

        let clocks = scratch.clocks.commit()?;
        let sequence = clocks.sequence();
        let effect = match publication {
            Some(publication) => self
                .effects_for_plan()
                .plan_publication(&publication, sequence)?,
            None => EffectDelta::default(),
        };
        if scratch.owners.changes.is_empty() {
            return Ok(PreparedRetainedAdmissionBatch {
                plan: Some(self.prepared_effect_only(effect, clocks)),
                consumed,
            });
        }

        let changes = scratch.owners.changes;
        self.reserve_primary_owner_insertions(
            changes
                .iter()
                .filter(|change| change.before.is_none())
                .map(|change| &change.key),
        )?;
        let mut resource_changes = Vec::new();
        resource_changes
            .try_reserve_exact(changes.len())
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        resource_changes.extend(changes.iter().map(|change| {
            (
                change.key.clone(),
                change.before.as_ref().map(OwnedTx::charge_record),
                Some(change.after.charge_record()),
            )
        }));
        let resources = self.resources_for_plan().plan_batch(resource_changes)?;
        let scheduler = self.scheduler.plan_batch(
            changes
                .iter()
                .map(|change| (change.before.as_ref(), Some(&change.after))),
        )?;
        let dependency = self.dependencies.plan_primary_replacements(
            changes
                .iter()
                .map(|change| (change.before.as_ref(), Some(&change.after))),
        )?;
        let sources = self.source_versions.plan_replacements(
            changes
                .iter()
                .map(|change| (change.before.as_ref(), Some(&change.after))),
            sequence,
        );
        let indexes = self.indexes_for_plan().plan_replacements(
            changes
                .iter()
                .map(|change| (&change.key, change.before.as_ref(), Some(&change.after))),
        )?;
        let retired = super::retired_buffer(changes.len())?;
        let mut updates = Vec::new();
        updates
            .try_reserve_exact(changes.len())
            .map_err(|_| super::PlanError::Backpressure(super::Backpressure::Allocation))?;
        updates.extend(changes.into_iter().map(|change| RetainedIngressUpdate {
            key: change.key,
            after: change.after,
        }));
        let owners = DerivedOwnerDelta { indexes, sources };
        Ok(PreparedRetainedAdmissionBatch {
            plan: Some(PreparedApply {
                authority: self,
                delta: AuthorityDelta::RetainedIngress(RetainedIngressDelta {
                    updates,
                    owners,
                    resources,
                    scheduler,
                    dependency,
                    effect,
                    clocks: clocks.finish(),
                    retired,
                }),
            }),
            consumed,
        })
    }

    fn plan_retained_batch_item(
        &self,
        kind: RetainedIngressKind,
        attempt: &RetainedIngressAttempt,
        planned_at: Instant,
        scratch: &mut BatchScratch,
    ) -> Result<ItemDecision, PlanError> {
        match attempt {
            RetainedIngressAttempt::Rejected(rejection) => {
                let audience = match rejection.kind() {
                    RetainedIngressKind::Remote(peer) => Some(peer),
                    RetainedIngressKind::Proposal => None,
                };
                Ok(ItemDecision {
                    effect: Some(CommittedEffect::Rejected(CommittedRejection::Validation {
                        tx: std::sync::Arc::clone(rejection.transaction()),
                        audience: RejectionAudience::from_ingress(audience),
                        reason: rejection.reason().clone(),
                    })),
                })
            }
            RetainedIngressAttempt::ProposalUnavailable => Ok(ItemDecision { effect: None }),
            RetainedIngressAttempt::Validated(ingress) => {
                if ingress.kind() != kind {
                    return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                }
                self.plan_validated_retained_batch_item(
                    kind,
                    ingress.admission(),
                    planned_at,
                    scratch,
                )
            }
        }
    }

    fn plan_validated_retained_batch_item(
        &self,
        kind: RetainedIngressKind,
        admission: &ValidatedAdmission,
        planned_at: Instant,
        scratch: &mut BatchScratch,
    ) -> Result<ItemDecision, PlanError> {
        let key = admission.identity.raw.clone();
        if let RetainedIngressKind::Remote(peer) = kind {
            if self.peer_bans.contains_at(peer, planned_at) {
                return Ok(ItemDecision {
                    effect: Some(CommittedEffect::RemoteIngressReleased(
                        CommittedRemoteIngressRelease::unretained_remote_submission(key, peer),
                    )),
                });
            }
            return match scratch.owners.current(self, &key)? {
                Some(OwnedTx::Accepted(_)) => Ok(ItemDecision {
                    effect: Some(CommittedEffect::Accepted(CommittedAcceptance::Duplicate {
                        tx_hash: key,
                        requesting_peer: Some(peer),
                    })),
                }),
                Some(OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_)) => {
                    Ok(ItemDecision {
                        effect: Some(CommittedEffect::RemoteIngressReleased(
                            CommittedRemoteIngressRelease::unretained_remote_submission(key, peer),
                        )),
                    })
                }
                None => self.plan_new_retained_owner(kind, admission, scratch),
            };
        }

        let current = scratch.owners.current(self, &key)?;
        match &current {
            Some(OwnedTx::Accepted(_)) => {
                return Ok(ItemDecision { effect: None });
            }
            Some(OwnedTx::PreAccepted(entry))
                if entry.record.identity.witness == admission.identity.witness
                    && !matches!(entry.source, PreAcceptedSource::Remote(_)) =>
            {
                return Ok(ItemDecision { effect: None });
            }
            Some(OwnedTx::PreAccepted(entry))
                if matches!(entry.source, PreAcceptedSource::Recovery(_)) =>
            {
                return Ok(ItemDecision { effect: None });
            }
            Some(OwnedTx::PreAccepted(_) | OwnedTx::ReplacementHistory(_)) | None => {}
        }
        self.plan_proposal_owner(admission, current, scratch)
    }

    fn plan_new_retained_owner(
        &self,
        kind: RetainedIngressKind,
        admission: &ValidatedAdmission,
        scratch: &mut BatchScratch,
    ) -> Result<ItemDecision, PlanError> {
        if let Some(owner) = scratch
            .owners
            .proposal_owner(self, &admission.identity.proposal)
            && owner != admission.identity.raw
        {
            return self.retained_pressure(kind, admission, super::Backpressure::ProposalCollision);
        }
        let charge = self
            .resources
            .admission_charge(admission.payload_bytes, admission.encoded_edges)?;
        if let Err(error) = self.resources.validate_admission(charge) {
            return self.retained_resource_pressure(kind, admission, error);
        }
        let charge_record = ChargeRecord::PreAccepted {
            resources: charge,
            residency_peer: admission.source.ingress_peer(),
            compute_peer: None,
        };
        if let Err(error) = scratch.resources.replace(
            self.resources.read(&self.entries),
            None,
            Some(charge_record),
        ) {
            return self.retained_resource_pressure(kind, admission, error);
        }

        // Identity allocation follows every fallible resource decision which
        // does not need that identity. A pressure-excluded item therefore
        // consumes neither a version nor an arrival, while a subsequently
        // dropped nonempty Plan still leaves its already-issued identities as
        // non-reusable gaps.
        let (version, arrival, clock_branch) = scratch.clocks.owner_branch().insertion()?;
        let after = OwnedTx::PreAccepted(PreAcceptedEntry {
            record: TxRecord {
                tx: std::sync::Arc::clone(&admission.tx),
                identity: admission.identity.clone(),
                version,
                arrival,
            },
            source: admission.source,
            basis: AdmissionBasis::new(
                admission.dependencies.clone(),
                admission.payload_bytes,
                admission.encoded_edges,
                charge,
            ),
            phase: PreAcceptedPhase::Queued(QueuedWork::Resolve),
            charge,
        });
        scratch
            .owners
            .replace(self, admission.identity.raw.clone(), after)?;
        clock_branch.adopt();
        Ok(ItemDecision { effect: None })
    }

    fn plan_proposal_owner(
        &self,
        admission: &ValidatedAdmission,
        current: Option<OwnedTx>,
        scratch: &mut BatchScratch,
    ) -> Result<ItemDecision, PlanError> {
        let Some(current) = current else {
            return self.plan_new_retained_owner(RetainedIngressKind::Proposal, admission, scratch);
        };
        let charge = self
            .resources
            .admission_charge(admission.payload_bytes, admission.encoded_edges)?;
        if let Err(error) = self.resources.validate_admission(charge) {
            return self.retained_resource_pressure(
                RetainedIngressKind::Proposal,
                admission,
                error,
            );
        }

        let clock_branch = scratch.clocks.owner_branch();
        let (after, clock_branch) = match &current {
            OwnedTx::PreAccepted(entry) => {
                let same_witness = entry.record.identity.witness == admission.identity.witness;
                let proposal_base = match entry.source {
                    PreAcceptedSource::Remote(remote) => ProposalBase::Remote(remote.residency),
                    PreAcceptedSource::Proposal { base } => base,
                    PreAcceptedSource::Recovery(_) => {
                        return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
                    }
                };
                if same_witness {
                    let mut promoted = entry.clone();
                    promoted.source = PreAcceptedSource::Proposal {
                        base: proposal_base,
                    };
                    if matches!(promoted.phase, PreAcceptedPhase::Waiting(_)) {
                        promoted.phase = PreAcceptedPhase::Queued(QueuedWork::Resolve);
                        promoted.charge = promoted.original_charge();
                    }
                    (OwnedTx::PreAccepted(promoted), clock_branch)
                } else {
                    let (version, clock_branch) = clock_branch.replacement()?;
                    (
                        OwnedTx::PreAccepted(PreAcceptedEntry {
                            record: TxRecord {
                                tx: std::sync::Arc::clone(&admission.tx),
                                identity: admission.identity.clone(),
                                version,
                                arrival: entry.record.arrival,
                            },
                            source: PreAcceptedSource::Proposal {
                                base: proposal_base,
                            },
                            basis: AdmissionBasis::new(
                                admission.dependencies.clone(),
                                admission.payload_bytes,
                                admission.encoded_edges,
                                charge,
                            ),
                            phase: PreAcceptedPhase::Queued(QueuedWork::Resolve),
                            charge,
                        }),
                        clock_branch,
                    )
                }
            }
            OwnedTx::ReplacementHistory(history) => {
                let (version, clock_branch) = clock_branch.replacement()?;
                let same_witness = history.record().identity.witness == admission.identity.witness;
                let promoted = if same_witness {
                    let mut promoted = history.clone().into_recovery(self.generation, version);
                    promoted.source = PreAcceptedSource::Proposal {
                        base: ProposalBase::Trusted,
                    };
                    promoted
                } else {
                    PreAcceptedEntry {
                        record: TxRecord {
                            tx: std::sync::Arc::clone(&admission.tx),
                            identity: admission.identity.clone(),
                            version,
                            arrival: history.record().arrival,
                        },
                        source: PreAcceptedSource::Proposal {
                            base: ProposalBase::Trusted,
                        },
                        basis: AdmissionBasis::new(
                            admission.dependencies.clone(),
                            admission.payload_bytes,
                            admission.encoded_edges,
                            charge,
                        ),
                        phase: PreAcceptedPhase::Queued(QueuedWork::Resolve),
                        charge,
                    }
                };
                (OwnedTx::PreAccepted(promoted), clock_branch)
            }
            OwnedTx::Accepted(_) => {
                return Err(PlanError::Fault(AuthorityFault::MembershipProjection));
            }
        };
        if let Err(error) = scratch.resources.replace(
            self.resources.read(&self.entries),
            Some(current.charge_record()),
            Some(after.charge_record()),
        ) {
            return self.retained_resource_pressure(
                RetainedIngressKind::Proposal,
                admission,
                error,
            );
        }
        scratch
            .owners
            .replace(self, admission.identity.raw.clone(), after)?;
        clock_branch.adopt();
        Ok(ItemDecision { effect: None })
    }

    fn retained_resource_pressure(
        &self,
        kind: RetainedIngressKind,
        admission: &ValidatedAdmission,
        error: ResourceError,
    ) -> Result<ItemDecision, PlanError> {
        let pressure = match error {
            ResourceError::PreAcceptedLimit => super::Backpressure::TotalResources,
            ResourceError::RemoteLimit => super::Backpressure::RemoteResources,
            ResourceError::PeerLimit(_) => super::Backpressure::PeerResources,
            ResourceError::ComputeEnvelope => super::Backpressure::ComputeResources,
            ResourceError::Allocation => super::Backpressure::Allocation,
            ResourceError::ReplacementHistoryLimit
            | ResourceError::AcceptedLimit
            | ResourceError::Arithmetic
            | ResourceError::ExistingChargeMismatch
            | ResourceError::DuplicateChange
            | ResourceError::AttributionMismatch
            | ResourceError::CapacityBankFault => return Err(error.into()),
        };
        self.retained_pressure(kind, admission, pressure)
    }

    fn retained_pressure(
        &self,
        kind: RetainedIngressKind,
        admission: &ValidatedAdmission,
        pressure: super::Backpressure,
    ) -> Result<ItemDecision, PlanError> {
        let RetainedIngressKind::Remote(peer) = kind else {
            return match pressure {
                super::Backpressure::TotalResources
                | super::Backpressure::RemoteResources
                | super::Backpressure::PeerResources
                | super::Backpressure::ComputeResources
                | super::Backpressure::ProposalCollision
                | super::Backpressure::Allocation => Ok(ItemDecision { effect: None }),
                super::Backpressure::AcceptedResources
                | super::Backpressure::GenerationReplacement
                | super::Backpressure::EffectCapacity => {
                    Err(PlanError::Fault(AuthorityFault::ResourceProjection))
                }
            };
        };
        let pressure = match pressure {
            super::Backpressure::TotalResources => RemoteIngressPressure::TotalResources,
            super::Backpressure::RemoteResources => RemoteIngressPressure::RemoteResources,
            super::Backpressure::PeerResources => RemoteIngressPressure::PeerResources,
            super::Backpressure::ComputeResources => RemoteIngressPressure::ComputeResources,
            super::Backpressure::ProposalCollision => RemoteIngressPressure::ProposalCollision,
            super::Backpressure::Allocation => RemoteIngressPressure::Allocation,
            super::Backpressure::AcceptedResources
            | super::Backpressure::GenerationReplacement
            | super::Backpressure::EffectCapacity => {
                return Err(PlanError::Fault(AuthorityFault::ResourceProjection));
            }
        };
        let reason = crate::authority::rejection::CommittedPublicReject::new(
            crate::error::Reject::Full(pressure.reason().to_owned()),
        );
        Ok(ItemDecision {
            effect: Some(CommittedEffect::Rejected(CommittedRejection::Validation {
                tx: std::sync::Arc::clone(&admission.tx),
                audience: RejectionAudience::from_ingress(Some(peer)),
                reason,
            })),
        })
    }
}
