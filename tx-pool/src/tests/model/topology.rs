//! Complete-candidate cost and boundary model for the M3.6 topology choice.
//!
//! This module does not predict wall time. It counts serial authority cuts and
//! explicit topology resources for one barrier-released, chain-backed,
//! independent workload. Existing composition tests own semantic equivalence;
//! these equations prevent a candidate from winning through imprecise prose.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ExecutionTopology {
    CurrentUak,
    SelfFusedWorkers,
    BoundedSemanticExchange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct IndependentWaveInput {
    pub(super) owners: u32,
    pub(super) retained_worker_slots: u32,
    pub(super) mutation_batch_limit: u32,
    pub(super) current_ready_batch_limit: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CandidateApplyCost {
    pub(super) ingress: u64,
    pub(super) compute: u64,
    pub(super) ready_membership: u64,
    pub(super) effect_settlement: u64,
    pub(super) total: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CandidateTopologySurface {
    pub(super) compute_mutation_callers: u32,
    pub(super) compute_tasks: u32,
    pub(super) transient_channel_slots: u32,
    pub(super) linear_capability_bound: u32,
    pub(super) added_join_edges: u32,
    pub(super) amortizes_one_available_wave: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ExchangePermitState {
    FinishedCapabilityPresent,
    IdleFill,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ExchangePermitAcquisition {
    ImmediateOnly,
    MayQueueOne,
}

pub(super) const fn exchange_permit_acquisition(
    state: ExchangePermitState,
) -> ExchangePermitAcquisition {
    match state {
        // A finished capability must settle without waiting behind a Direct
        // holder. An immediately available grant may still fuse checkout.
        ExchangePermitState::FinishedCapabilityPresent => ExchangePermitAcquisition::ImmediateOnly,
        ExchangePermitState::IdleFill => ExchangePermitAcquisition::MayQueueOne,
    }
}

impl IndependentWaveInput {
    pub(super) fn compile(self, topology: ExecutionTopology) -> Option<CandidateApplyCost> {
        if self.owners == 0
            || self.retained_worker_slots == 0
            || self.mutation_batch_limit == 0
            || self.current_ready_batch_limit == 0
        {
            return None;
        }
        let owners = u64::from(self.owners);
        let workers = self.retained_worker_slots.min(self.owners);
        let current_ready_width = workers.min(self.current_ready_batch_limit);
        let exchange_width = workers.min(self.mutation_batch_limit);
        let current_ready_waves = ceil_div(self.owners, current_ready_width)?;
        let exchange_waves = ceil_div(self.owners, exchange_width)?;

        let (ingress, compute, ready_membership) = match topology {
            ExecutionTopology::CurrentUak => (
                owners,
                owners.checked_mul(2)?,
                u64::from(current_ready_waves),
            ),
            ExecutionTopology::SelfFusedWorkers => (
                owners,
                owners.checked_add(u64::from(workers))?,
                u64::from(current_ready_waves),
            ),
            ExecutionTopology::BoundedSemanticExchange => (
                u64::from(ceil_div(self.owners, self.mutation_batch_limit)?),
                u64::from(exchange_waves).checked_add(1)?,
                u64::from(exchange_waves),
            ),
        };
        let effect_settlement = ready_membership;
        let total = ingress
            .checked_add(compute)?
            .checked_add(ready_membership)?
            .checked_add(effect_settlement)?;
        Some(CandidateApplyCost {
            ingress,
            compute,
            ready_membership,
            effect_settlement,
            total,
        })
    }

    pub(super) fn surface(self, topology: ExecutionTopology) -> Option<CandidateTopologySurface> {
        if self.retained_worker_slots == 0 {
            return None;
        }
        let workers = self.retained_worker_slots;
        Some(match topology {
            ExecutionTopology::CurrentUak | ExecutionTopology::SelfFusedWorkers => {
                CandidateTopologySurface {
                    compute_mutation_callers: workers,
                    compute_tasks: workers,
                    transient_channel_slots: 0,
                    linear_capability_bound: workers,
                    added_join_edges: 0,
                    amortizes_one_available_wave: false,
                }
            }
            ExecutionTopology::BoundedSemanticExchange => CandidateTopologySurface {
                compute_mutation_callers: 1,
                compute_tasks: workers.checked_add(1)?,
                transient_channel_slots: workers.checked_mul(2)?,
                linear_capability_bound: workers,
                added_join_edges: 1,
                amortizes_one_available_wave: true,
            },
        })
    }
}

fn ceil_div(value: u32, divisor: u32) -> Option<u32> {
    if value == 0 || divisor == 0 {
        return None;
    }
    value.checked_sub(1)?.checked_div(divisor)?.checked_add(1)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum QueryTopology {
    CurrentGuarded,
    SemaphoreOnly { permits: u32 },
    PreparedScratch { permits: u32 },
    ResidentProjection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct QueryTopologyInput {
    pub(super) concurrent_requests: u32,
    pub(super) owner_rows: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct QueryTopologyCost {
    pub(super) concurrent_guard_scans: u32,
    pub(super) authority_row_visits: u64,
    pub(super) allocates_under_guard: bool,
    pub(super) sorts_under_guard: bool,
    pub(super) duplicate_resident_rows: u32,
    pub(super) per_apply_projection_work: bool,
    pub(super) bounded_capture_admission: bool,
}

impl QueryTopologyInput {
    pub(super) fn compile(self, topology: QueryTopology) -> Option<QueryTopologyCost> {
        let scans = match topology {
            QueryTopology::CurrentGuarded | QueryTopology::ResidentProjection => {
                self.concurrent_requests
            }
            QueryTopology::SemaphoreOnly { permits }
            | QueryTopology::PreparedScratch { permits } => {
                if permits == 0 {
                    return None;
                }
                self.concurrent_requests.min(permits)
            }
        };
        let row_visits = u64::from(scans).checked_mul(u64::from(self.owner_rows))?;
        let (guard_scans, rows, allocation, sorting, duplicate, projection, bounded) =
            match topology {
                QueryTopology::CurrentGuarded => (scans, row_visits, true, true, 0, false, false),
                QueryTopology::SemaphoreOnly { .. } => {
                    (scans, row_visits, true, true, 0, false, true)
                }
                QueryTopology::PreparedScratch { .. } => {
                    (scans, row_visits, false, false, 0, false, true)
                }
                QueryTopology::ResidentProjection => {
                    (0, 0, false, false, self.owner_rows, true, true)
                }
            };
        Some(QueryTopologyCost {
            concurrent_guard_scans: guard_scans,
            authority_row_visits: rows,
            allocates_under_guard: allocation,
            sorts_under_guard: sorting,
            duplicate_resident_rows: duplicate,
            per_apply_projection_work: projection,
            bounded_capture_admission: bounded,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct QueryScratch {
    pub(super) capacity: u32,
    pub(super) max_capacity: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum QueryScratchStep {
    Ready,
    Grow(QueryScratch),
    OrdinaryUnavailable,
    RequestExceedsBound,
}

impl QueryScratch {
    pub(super) fn remaining_rank(self) -> Option<u32> {
        self.max_capacity.checked_sub(self.capacity)
    }

    /// Decide one lock-external allocation step. Re-observation may request a
    /// larger buffer, but every successful growth strictly decreases the
    /// finite rank and no unchanged authority cut is retried.
    pub(super) fn prepare(
        self,
        observed_rows: u32,
        allocation_available: bool,
    ) -> QueryScratchStep {
        if self.capacity > self.max_capacity || observed_rows > self.max_capacity {
            return QueryScratchStep::RequestExceedsBound;
        }
        if observed_rows <= self.capacity {
            return QueryScratchStep::Ready;
        }
        if !allocation_available {
            return QueryScratchStep::OrdinaryUnavailable;
        }
        let doubled = self
            .capacity
            .checked_mul(2)
            .map_or(self.max_capacity, |value| value.min(self.max_capacity));
        let grown = doubled.max(1).max(observed_rows).min(self.max_capacity);
        QueryScratchStep::Grow(Self {
            capacity: grown,
            max_capacity: self.max_capacity,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CachePublicationTopology {
    InlineTryWrite,
    BoundedWriter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CachePublicationInput {
    pub(super) updates: u32,
    pub(super) channel_updates: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CachePublicationCost {
    pub(super) persistent_tasks: u32,
    pub(super) resident_updates: u32,
    pub(super) write_lock_attempts: u32,
    pub(super) accepted_update_has_releaser: bool,
    pub(super) worker_waits_for_cache: bool,
}

impl CachePublicationInput {
    pub(super) fn compile(self, topology: CachePublicationTopology) -> CachePublicationCost {
        match topology {
            CachePublicationTopology::InlineTryWrite => CachePublicationCost {
                persistent_tasks: 0,
                resident_updates: 0,
                write_lock_attempts: self.updates,
                accepted_update_has_releaser: false,
                worker_waits_for_cache: false,
            },
            CachePublicationTopology::BoundedWriter => CachePublicationCost {
                persistent_tasks: 1,
                resident_updates: self.channel_updates,
                write_lock_attempts: self.updates,
                accepted_update_has_releaser: true,
                worker_waits_for_cache: false,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RetainedIngressTopology {
    PerRequest,
    ExistingDispatcherDrain,
    DedicatedIngressActor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RetainedIngressSurface {
    pub(super) batches_immediately_available_requests: bool,
    pub(super) added_tasks: u32,
    pub(super) added_channels: u32,
    pub(super) timer_or_fill_wait: bool,
    pub(super) exact_per_request_completion: bool,
}

pub(super) const fn retained_ingress_surface(
    topology: RetainedIngressTopology,
) -> RetainedIngressSurface {
    match topology {
        RetainedIngressTopology::PerRequest => RetainedIngressSurface {
            batches_immediately_available_requests: false,
            added_tasks: 0,
            added_channels: 0,
            timer_or_fill_wait: false,
            exact_per_request_completion: true,
        },
        RetainedIngressTopology::ExistingDispatcherDrain => RetainedIngressSurface {
            batches_immediately_available_requests: true,
            added_tasks: 0,
            added_channels: 0,
            timer_or_fill_wait: false,
            exact_per_request_completion: true,
        },
        RetainedIngressTopology::DedicatedIngressActor => RetainedIngressSurface {
            batches_immediately_available_requests: true,
            added_tasks: 1,
            added_channels: 1,
            timer_or_fill_wait: false,
            exact_per_request_completion: true,
        },
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum OrderedBoundaryTopology {
    SharedReliableSender,
    TypedReorgAndBoundedAdmin,
}

/// Observable completion cut of one committed chain-tip publication.
///
/// Syntax does not matter here: an inline call, oneshot, join or another
/// mechanism belongs to the same representative iff returning to the chain
/// publisher implies that the unique authority has applied that exact view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum ChainCompletionTopology {
    ApplyAcknowledged,
    EnqueueOnly,
}

/// Observable freshness protocol of a block-template read after a chain-tip
/// publisher returns.  The selected representative requires the proposal,
/// transaction and uncle components to cover one coherent chain source, waits
/// only on the existing monotonic replacement level and turns a same-source
/// rebuild failure into a typed terminal outcome.  A scalar source gate is a
/// distinct counterexample: reset or one partial component can carry the new
/// source while the other published components still describe another cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum TemplateChainReadTopology {
    SourceGatedTerminal,
    SourceScalarGatedTerminal,
    UngatedLastPublished,
    SourceGatedUnboundedWait,
    TimerOrPollingFreshness,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ChainCompletionStep {
    AwaitingApply,
    ReturnedBeforeApply,
    AppliedAndReturned,
    AppliedAfterReturn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TemplateChainReadStep {
    Current(u16),
    Pending(u16),
    Unavailable(u16),
    Stale {
        required: u16,
        returned: u16,
    },
    Incoherent {
        required: u16,
        proposals: Option<u16>,
        transactions: Option<u16>,
        uncles: Option<u16>,
    },
    RetryWithoutChange(u16),
}

/// Finite counterexample model for the chain-publish/template-read cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ChainCompletionProtocol {
    pub(super) chain_view: u16,
    pub(super) authority_view: u16,
    template_components: [Option<u16>; 3],
    last_template_view: u16,
    pending: Option<u16>,
    publisher_returned: bool,
    failed_template_view: Option<u16>,
}

impl ChainCompletionProtocol {
    pub(super) const fn new(view: u16) -> Self {
        Self {
            chain_view: view,
            authority_view: view,
            template_components: [Some(view); 3],
            last_template_view: view,
            pending: None,
            publisher_returned: false,
            failed_template_view: None,
        }
    }

    pub(super) fn install(
        &mut self,
        next: u16,
        topology: ChainCompletionTopology,
    ) -> Option<ChainCompletionStep> {
        if self.pending.is_some() || next <= self.chain_view {
            return None;
        }
        self.chain_view = next;
        self.pending = Some(next);
        self.publisher_returned = matches!(topology, ChainCompletionTopology::EnqueueOnly);
        Some(if self.publisher_returned {
            ChainCompletionStep::ReturnedBeforeApply
        } else {
            ChainCompletionStep::AwaitingApply
        })
    }

    pub(super) fn apply(
        &mut self,
        topology: ChainCompletionTopology,
    ) -> Option<ChainCompletionStep> {
        self.authority_view = self.pending.take()?;
        if matches!(topology, ChainCompletionTopology::ApplyAcknowledged) {
            self.publisher_returned = true;
            Some(ChainCompletionStep::AppliedAndReturned)
        } else {
            Some(ChainCompletionStep::AppliedAfterReturn)
        }
    }

    pub(super) fn publish_template(&mut self) {
        self.template_components = [Some(self.authority_view); 3];
        self.last_template_view = self.authority_view;
        self.failed_template_view = None;
    }

    pub(super) fn publish_template_reset(&mut self) {
        self.template_components = [None; 3];
        self.last_template_view = self.authority_view;
        self.failed_template_view = None;
    }

    pub(super) fn publish_template_proposals(&mut self) {
        self.template_components = [Some(self.authority_view), None, None];
        self.last_template_view = self.authority_view;
    }

    pub(super) fn publish_template_uncles(&mut self) {
        self.template_components[0] = Some(self.authority_view);
        self.template_components[1] = None;
        self.template_components[2] = Some(self.authority_view);
        self.last_template_view = self.authority_view;
    }

    pub(super) fn publish_template_transactions(&mut self) {
        self.template_components[1] = Some(self.authority_view);
        self.last_template_view = self.authority_view;
    }

    pub(super) fn fail_template(&mut self) {
        self.failed_template_view = Some(self.authority_view);
    }

    fn coherent_template_view(self) -> Option<u16> {
        let [Some(proposals), Some(transactions), Some(uncles)] = self.template_components else {
            return None;
        };
        (proposals == transactions && transactions == uncles).then_some(proposals)
    }

    fn scalar_template_observation(self) -> TemplateChainReadStep {
        if self.coherent_template_view() == Some(self.chain_view) {
            TemplateChainReadStep::Current(self.chain_view)
        } else {
            TemplateChainReadStep::Incoherent {
                required: self.chain_view,
                proposals: self.template_components[0],
                transactions: self.template_components[1],
                uncles: self.template_components[2],
            }
        }
    }

    pub(super) fn template_read_after_return(
        self,
        topology: TemplateChainReadTopology,
    ) -> Option<TemplateChainReadStep> {
        if !self.publisher_returned {
            return None;
        }
        Some(match topology {
            TemplateChainReadTopology::SourceGatedTerminal => {
                if self.coherent_template_view() == Some(self.chain_view) {
                    TemplateChainReadStep::Current(self.chain_view)
                } else if self.failed_template_view == Some(self.chain_view) {
                    TemplateChainReadStep::Unavailable(self.chain_view)
                } else {
                    TemplateChainReadStep::Pending(self.chain_view)
                }
            }
            TemplateChainReadTopology::SourceScalarGatedTerminal => {
                if self.last_template_view == self.chain_view {
                    self.scalar_template_observation()
                } else if self.failed_template_view == Some(self.chain_view) {
                    TemplateChainReadStep::Unavailable(self.chain_view)
                } else {
                    TemplateChainReadStep::Pending(self.chain_view)
                }
            }
            TemplateChainReadTopology::UngatedLastPublished => {
                if self.last_template_view == self.chain_view {
                    self.scalar_template_observation()
                } else {
                    TemplateChainReadStep::Stale {
                        required: self.chain_view,
                        returned: self.last_template_view,
                    }
                }
            }
            TemplateChainReadTopology::SourceGatedUnboundedWait => {
                if self.coherent_template_view() == Some(self.chain_view) {
                    TemplateChainReadStep::Current(self.chain_view)
                } else {
                    TemplateChainReadStep::Pending(self.chain_view)
                }
            }
            TemplateChainReadTopology::TimerOrPollingFreshness => {
                if self.coherent_template_view() == Some(self.chain_view) {
                    TemplateChainReadStep::Current(self.chain_view)
                } else {
                    TemplateChainReadStep::RetryWithoutChange(self.chain_view)
                }
            }
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ProducerResidencyBound {
    UnboundedByProtocol,
    Bounded(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct OrderedBoundaryInput {
    pub(super) trusted_reorg_publishers: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct OrderedBoundaryCost {
    pub(super) waiting_payloads: ProducerResidencyBound,
    pub(super) reorg_is_lossless: bool,
    pub(super) excess_admin_is_fail_fast: bool,
    pub(super) accepted_admin_preserves_order: bool,
    pub(super) added_admin_gate: bool,
}

impl OrderedBoundaryInput {
    pub(super) fn compile(self, topology: OrderedBoundaryTopology) -> Option<OrderedBoundaryCost> {
        Some(match topology {
            OrderedBoundaryTopology::SharedReliableSender => OrderedBoundaryCost {
                waiting_payloads: ProducerResidencyBound::UnboundedByProtocol,
                reorg_is_lossless: true,
                excess_admin_is_fail_fast: false,
                accepted_admin_preserves_order: true,
                added_admin_gate: false,
            },
            OrderedBoundaryTopology::TypedReorgAndBoundedAdmin => OrderedBoundaryCost {
                waiting_payloads: ProducerResidencyBound::Bounded(
                    self.trusted_reorg_publishers.checked_add(1)?,
                ),
                reorg_is_lossless: true,
                excess_admin_is_fail_fast: true,
                accepted_admin_preserves_order: true,
                added_admin_gate: true,
            },
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CompleteTopology {
    pub(super) execution: ExecutionTopology,
    pub(super) ingress: RetainedIngressTopology,
    pub(super) query: QueryTopology,
    pub(super) cache: CachePublicationTopology,
    pub(super) ordered: OrderedBoundaryTopology,
    pub(super) chain_completion: ChainCompletionTopology,
    pub(super) template_chain_read: TemplateChainReadTopology,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum CompleteTopologyGap {
    PerOwnerAvailableWaveCuts,
    PerRequestRetainedIngress,
    UnboundedFullQueryAdmission,
    GuardHeldFallibleQueryWork,
    DuplicateResidentQueryProjection,
    CacheUpdateWithoutReleaser,
    UnboundedOrderedProducerResidency,
    ChainReturnBeforeAuthorityApply,
    IncoherentChainTemplateRead,
    StaleChainTemplateRead,
    UntotalChainTemplateRead,
    TimerOrPollingTemplateFreshness,
}

impl CompleteTopology {
    pub(super) fn gaps(
        self,
        execution: IndependentWaveInput,
        query: QueryTopologyInput,
        cache: CachePublicationInput,
        ordered: OrderedBoundaryInput,
    ) -> Option<Vec<CompleteTopologyGap>> {
        let mut gaps = Vec::new();
        if !execution
            .surface(self.execution)?
            .amortizes_one_available_wave
        {
            gaps.push(CompleteTopologyGap::PerOwnerAvailableWaveCuts);
        }
        if !retained_ingress_surface(self.ingress).batches_immediately_available_requests {
            gaps.push(CompleteTopologyGap::PerRequestRetainedIngress);
        }
        let query = query.compile(self.query)?;
        if !query.bounded_capture_admission {
            gaps.push(CompleteTopologyGap::UnboundedFullQueryAdmission);
        }
        if query.allocates_under_guard || query.sorts_under_guard {
            gaps.push(CompleteTopologyGap::GuardHeldFallibleQueryWork);
        }
        if query.duplicate_resident_rows != 0 || query.per_apply_projection_work {
            gaps.push(CompleteTopologyGap::DuplicateResidentQueryProjection);
        }
        if !cache.compile(self.cache).accepted_update_has_releaser {
            gaps.push(CompleteTopologyGap::CacheUpdateWithoutReleaser);
        }
        if matches!(
            ordered.compile(self.ordered)?.waiting_payloads,
            ProducerResidencyBound::UnboundedByProtocol
        ) {
            gaps.push(CompleteTopologyGap::UnboundedOrderedProducerResidency);
        }
        if matches!(self.chain_completion, ChainCompletionTopology::EnqueueOnly) {
            gaps.push(CompleteTopologyGap::ChainReturnBeforeAuthorityApply);
        }
        match self.template_chain_read {
            TemplateChainReadTopology::SourceGatedTerminal => {}
            TemplateChainReadTopology::SourceScalarGatedTerminal => {
                gaps.push(CompleteTopologyGap::IncoherentChainTemplateRead);
            }
            TemplateChainReadTopology::UngatedLastPublished => {
                gaps.push(CompleteTopologyGap::StaleChainTemplateRead);
            }
            TemplateChainReadTopology::SourceGatedUnboundedWait => {
                gaps.push(CompleteTopologyGap::UntotalChainTemplateRead);
            }
            TemplateChainReadTopology::TimerOrPollingFreshness => {
                gaps.push(CompleteTopologyGap::TimerOrPollingTemplateFreshness);
            }
        }
        gaps.sort_unstable();
        gaps.dedup();
        Some(gaps)
    }
}

// The global normal-form quotient is deliberately semantic rather than a list
// of Rust implementations. Every architecture in the declared release basis
// is classified by the observation that distinguishes each axis; syntax-only
// variants normalize to the same representative.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum AuthorityNormalForm {
    UniqueMinimumApply,
    UniqueOversizedApply,
    SplitLifecycleAuthorities,
    UniversalActor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum LifecycleNormalForm {
    SealedLinearExact,
    SealedLinearWithRollback,
    CopiedOrInferredEvidence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum CouplingNormalForm {
    ExactCanonicalAvailableCut,
    PerOwnerIndependentCuts,
    TimerOrApproximateBatch,
}

/// Consensus proposal eligibility and the tx-pool's proposal projection have
/// different trust roles.  The verifier must derive `committed subseteq
/// proposed` from primitive branch history; the tx-pool may keep only a
/// bounded, rebuildable view of the same history.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum ProposalHistoryNormalForm {
    /// Keep exact band reference counts in a structurally shared index with a
    /// deterministic worst-case bound.  A successor creates an authenticated
    /// sparse receipt and an O(1) snapshot clone; a non-successor rebuilds from
    /// primitive history. Consensus verification remains an independent
    /// branch-history fold.
    BoundedPersistentSparseExactViewIndependentVerifier,
    /// The same sparse transition shape backed only by an expected/probabilistic
    /// collision bound. Peer-chosen proposal ids make this inadmissible under
    /// the deterministic hostile-work constraint even if its mean is faster.
    ExpectedBoundPersistentSparseExactViewIndependentVerifier,
    /// Advance exact reference counts but clone or re-materialize the complete
    /// proposal index for the next immutable snapshot. This is semantically
    /// exact, but its successor work remains population-linear rather than
    /// primitive-delta-linear.
    MaterializedIncrementalExactViewIndependentVerifier,
    /// Recompute both exact proposal bands from the bounded history window on
    /// every tip while retaining the independent consensus fold.
    RecomputedExactViewIndependentVerifier,
    /// Let consensus verification consume a mutable tx-pool/chain projection.
    SharedMutableVerifierView,
    /// Retain only a scalar/current status and thereby alias at least two of
    /// Gap, Proposed and Outside across a legal continuation.
    ScalarOrCurrentHistory,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum ProgressNormalForm {
    SameApplyDerivedFiniteRank,
    SameApplyWithResidentDerivedDag,
    ResidentDecisionAuthority,
    PollingOrUnchangedRetry,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum ResourceNormalForm {
    UnifiedBoundedFallible,
    PartitionedBoundedCharged,
    FragmentedOrUncharged,
    InfallibleOrUnbounded,
}

/// Terminal topology after a fallible construction has acquired an
/// authority-owned obligation. Caller-owned and derived requests already
/// return an ordinary unavailable outcome; this axis distinguishes the
/// architectures that can close retained work without inventing allocator
/// progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum AllocationNormalForm {
    /// Replace the rebuildable soft-state generation in one allocation-free
    /// Apply. This is the minimum allocator-independent terminal cut.
    AllocationFreeGenerationTerminal,
    /// Retire only the invariant-closed implicated scope through an extra
    /// fixed-capacity carrier or resident component owner.
    BoundedScopedRecovery,
    /// Preserve the same obligation and try again without a changed source.
    UnchangedCutRetry,
    /// Lose a linear capability or stop the authority/service generation.
    FailStopOrCapabilityLeak,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum ProjectionNormalForm {
    PostCommitDerivedPrepared,
    PostCommitWithResidentDerivedProjection,
    ExternalVetoOrGuardIo,
    ResidentPolicyProjection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum TaskNormalForm {
    BoundedOwnedMinimalLanes,
    BoundedOwnedExtraActor,
    UniversalSerializedLoop,
    DetachedOrRepairTopology,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum CompatibilityNormalForm {
    ForwardLegacyIntentionalMajor,
    ForwardLegacyMajorWithNonAuthoritativeFacade,
    DropsSupportedLegacy,
    ReverseMigrationOrAuthorityFacade,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct GlobalNormalForm {
    pub(super) authority: AuthorityNormalForm,
    pub(super) lifecycle: LifecycleNormalForm,
    pub(super) coupling: CouplingNormalForm,
    pub(super) chain_completion: ChainCompletionTopology,
    pub(super) proposal_history: ProposalHistoryNormalForm,
    pub(super) template_chain_read: TemplateChainReadTopology,
    pub(super) progress: ProgressNormalForm,
    pub(super) resources: ResourceNormalForm,
    pub(super) allocation: AllocationNormalForm,
    pub(super) projections: ProjectionNormalForm,
    pub(super) tasks: TaskNormalForm,
    pub(super) compatibility: CompatibilityNormalForm,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FeasibilityLaw {
    HardConstraint,
    ConcurrencyLaw,
    CouplingLaw,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GlobalFeasibilityGap {
    NonUniqueAuthority,
    UnnecessaryIndependentSerialization,
    NonMinimumCouplingCut,
    ChainReturnBeforeAuthorityApply,
    ConsensusUsesMutableProposalProjection,
    IncompleteProposalHistory,
    IncoherentChainTemplateRead,
    StaleChainTemplateRead,
    UntotalChainTemplateRead,
    TimerOrPollingTemplateFreshness,
    InexactCapabilityOrEvidence,
    ApproximateOrTimedBatch,
    DuplicateDependencyPolicy,
    UnchangedCutRetry,
    ResourceConservationViolation,
    HostileResourceUnbounded,
    AllocationFailStopOrCapabilityLeak,
    ExternalEffectVetoOrGuardIo,
    DuplicateProjectionPolicy,
    UntotalTaskOrRepairTopology,
    CompatibilityBoundaryViolation,
}

impl GlobalFeasibilityGap {
    pub(super) const fn law(self) -> FeasibilityLaw {
        match self {
            Self::UnnecessaryIndependentSerialization => FeasibilityLaw::ConcurrencyLaw,
            Self::NonMinimumCouplingCut | Self::ChainReturnBeforeAuthorityApply => {
                FeasibilityLaw::CouplingLaw
            }
            Self::NonUniqueAuthority
            | Self::ConsensusUsesMutableProposalProjection
            | Self::IncompleteProposalHistory
            | Self::IncoherentChainTemplateRead
            | Self::StaleChainTemplateRead
            | Self::UntotalChainTemplateRead
            | Self::TimerOrPollingTemplateFreshness
            | Self::InexactCapabilityOrEvidence
            | Self::ApproximateOrTimedBatch
            | Self::DuplicateDependencyPolicy
            | Self::UnchangedCutRetry
            | Self::ResourceConservationViolation
            | Self::HostileResourceUnbounded
            | Self::AllocationFailStopOrCapabilityLeak
            | Self::ExternalEffectVetoOrGuardIo
            | Self::DuplicateProjectionPolicy
            | Self::UntotalTaskOrRepairTopology
            | Self::CompatibilityBoundaryViolation => FeasibilityLaw::HardConstraint,
        }
    }
}

impl FeasibilityLaw {
    const fn index(self) -> usize {
        match self {
            Self::HardConstraint => 0,
            Self::ConcurrencyLaw => 1,
            Self::CouplingLaw => 2,
        }
    }
}

impl GlobalNormalForm {
    pub(super) fn feasibility_gaps(self) -> Vec<GlobalFeasibilityGap> {
        use GlobalFeasibilityGap as Gap;

        let mut gaps = Vec::new();
        match self.authority {
            AuthorityNormalForm::UniqueMinimumApply => {}
            AuthorityNormalForm::UniqueOversizedApply => gaps.push(Gap::NonMinimumCouplingCut),
            AuthorityNormalForm::SplitLifecycleAuthorities => gaps.push(Gap::NonUniqueAuthority),
            AuthorityNormalForm::UniversalActor => {
                gaps.push(Gap::UnnecessaryIndependentSerialization);
            }
        }
        if matches!(
            self.lifecycle,
            LifecycleNormalForm::CopiedOrInferredEvidence
        ) {
            gaps.push(Gap::InexactCapabilityOrEvidence);
        }
        match self.coupling {
            CouplingNormalForm::ExactCanonicalAvailableCut => {}
            CouplingNormalForm::PerOwnerIndependentCuts => {
                gaps.push(Gap::UnnecessaryIndependentSerialization);
            }
            CouplingNormalForm::TimerOrApproximateBatch => {
                gaps.push(Gap::ApproximateOrTimedBatch);
            }
        }
        if matches!(self.chain_completion, ChainCompletionTopology::EnqueueOnly) {
            gaps.push(Gap::ChainReturnBeforeAuthorityApply);
        }
        match self.proposal_history {
            ProposalHistoryNormalForm::BoundedPersistentSparseExactViewIndependentVerifier
            | ProposalHistoryNormalForm::MaterializedIncrementalExactViewIndependentVerifier
            | ProposalHistoryNormalForm::RecomputedExactViewIndependentVerifier => {}
            ProposalHistoryNormalForm::ExpectedBoundPersistentSparseExactViewIndependentVerifier => {
                gaps.push(Gap::HostileResourceUnbounded);
            }
            ProposalHistoryNormalForm::SharedMutableVerifierView => {
                gaps.push(Gap::ConsensusUsesMutableProposalProjection);
            }
            ProposalHistoryNormalForm::ScalarOrCurrentHistory => {
                gaps.push(Gap::IncompleteProposalHistory);
            }
        }
        match self.template_chain_read {
            TemplateChainReadTopology::SourceGatedTerminal => {}
            TemplateChainReadTopology::SourceScalarGatedTerminal => {
                gaps.push(Gap::IncoherentChainTemplateRead);
            }
            TemplateChainReadTopology::UngatedLastPublished => {
                gaps.push(Gap::StaleChainTemplateRead);
            }
            TemplateChainReadTopology::SourceGatedUnboundedWait => {
                gaps.push(Gap::UntotalChainTemplateRead);
            }
            TemplateChainReadTopology::TimerOrPollingFreshness => {
                gaps.push(Gap::TimerOrPollingTemplateFreshness);
            }
        }
        match self.progress {
            ProgressNormalForm::SameApplyDerivedFiniteRank
            | ProgressNormalForm::SameApplyWithResidentDerivedDag => {}
            ProgressNormalForm::ResidentDecisionAuthority => {
                gaps.push(Gap::DuplicateDependencyPolicy);
            }
            ProgressNormalForm::PollingOrUnchangedRetry => gaps.push(Gap::UnchangedCutRetry),
        }
        match self.resources {
            ResourceNormalForm::UnifiedBoundedFallible
            | ResourceNormalForm::PartitionedBoundedCharged => {}
            ResourceNormalForm::FragmentedOrUncharged => {
                gaps.push(Gap::ResourceConservationViolation);
            }
            ResourceNormalForm::InfallibleOrUnbounded => {
                gaps.push(Gap::HostileResourceUnbounded);
            }
        }
        match self.allocation {
            AllocationNormalForm::AllocationFreeGenerationTerminal
            | AllocationNormalForm::BoundedScopedRecovery => {}
            AllocationNormalForm::UnchangedCutRetry => gaps.push(Gap::UnchangedCutRetry),
            AllocationNormalForm::FailStopOrCapabilityLeak => {
                gaps.push(Gap::AllocationFailStopOrCapabilityLeak);
            }
        }
        match self.projections {
            ProjectionNormalForm::PostCommitDerivedPrepared
            | ProjectionNormalForm::PostCommitWithResidentDerivedProjection => {}
            ProjectionNormalForm::ExternalVetoOrGuardIo => {
                gaps.push(Gap::ExternalEffectVetoOrGuardIo);
            }
            ProjectionNormalForm::ResidentPolicyProjection => {
                gaps.push(Gap::DuplicateProjectionPolicy);
            }
        }
        match self.tasks {
            TaskNormalForm::BoundedOwnedMinimalLanes | TaskNormalForm::BoundedOwnedExtraActor => {}
            TaskNormalForm::UniversalSerializedLoop => {
                gaps.push(Gap::UnnecessaryIndependentSerialization);
            }
            TaskNormalForm::DetachedOrRepairTopology => {
                gaps.push(Gap::UntotalTaskOrRepairTopology);
            }
        }
        if matches!(
            self.compatibility,
            CompatibilityNormalForm::DropsSupportedLegacy
                | CompatibilityNormalForm::ReverseMigrationOrAuthorityFacade
        ) {
            gaps.push(Gap::CompatibilityBoundaryViolation);
        }
        gaps
    }

    /// Ordered extra cost above the irreducible release-law lower bound. The
    /// coordinates follow `optimization_goal.static_objective` exactly.
    pub(super) const fn static_extra_cost(self) -> [u32; 7] {
        let mut cost = [0; 7];
        if matches!(
            self.lifecycle,
            LifecycleNormalForm::SealedLinearWithRollback
        ) {
            cost[0] += 1;
            cost[2] += 1;
            cost[3] += 1;
            cost[4] += 1;
            cost[6] += 1;
        }
        if matches!(
            self.progress,
            ProgressNormalForm::SameApplyWithResidentDerivedDag
        ) {
            cost[3] += 1;
            cost[4] += 1;
            cost[6] += 1;
        }
        if matches!(
            self.projections,
            ProjectionNormalForm::PostCommitWithResidentDerivedProjection
        ) {
            cost[3] += 1;
            cost[4] += 1;
            cost[6] += 1;
        }
        if matches!(self.tasks, TaskNormalForm::BoundedOwnedExtraActor) {
            cost[0] += 1;
            cost[4] += 1;
            cost[6] += 1;
        }
        if matches!(
            self.resources,
            ResourceNormalForm::PartitionedBoundedCharged
        ) {
            cost[4] += 1;
            cost[6] += 1;
        }
        if matches!(self.allocation, AllocationNormalForm::BoundedScopedRecovery) {
            // The cheapest credible scoped terminal uses the release-law
            // component bound as an inline/fixed-capacity retirement carrier.
            // It needs no second authority or cut, but must enumerate the
            // invariant-closed component under the guard and retain bounded
            // transient owner storage. A resident component index/recovery
            // lease is not cheaper on either coordinate.
            cost[3] += 1;
            cost[4] += 1;
        }
        match self.proposal_history {
            ProposalHistoryNormalForm::BoundedPersistentSparseExactViewIndependentVerifier => {
                // Exact persistent counts and an authenticated transition
                // receipt make both snapshot cloning and successor maintenance
                // independent of unchanged proposal population. The bounded
                // rebuildable projection is the release-law floor and adds no
                // authority, task, lock, failure domain or serial cut.
            }
            ProposalHistoryNormalForm::MaterializedIncrementalExactViewIndependentVerifier => {
                // The counts avoid re-folding history, but copying the complete
                // immutable view still incurs avoidable population-linear
                // allocation and work on every ordinary successor/refresh.
                cost[4] += 1;
            }
            ProposalHistoryNormalForm::RecomputedExactViewIndependentVerifier => {
                // Re-reading the unchanged interior and materializing the next
                // view are both avoidable beyond the primitive boundary delta.
                cost[3] += 1;
                cost[4] += 1;
            }
            ProposalHistoryNormalForm::ExpectedBoundPersistentSparseExactViewIndependentVerifier
            | ProposalHistoryNormalForm::SharedMutableVerifierView
            | ProposalHistoryNormalForm::ScalarOrCurrentHistory => {}
        }
        if matches!(
            self.compatibility,
            CompatibilityNormalForm::ForwardLegacyMajorWithNonAuthoritativeFacade
        ) {
            cost[0] += 1;
        }
        cost
    }

    /// Stable semantic signature in the declared normal-form axis order.  The
    /// optimizer computes the minimum signature; no branch asks whether a
    /// candidate has the selected topology's name.
    pub(super) const fn signature(self) -> [&'static str; 12] {
        [
            match self.authority {
                AuthorityNormalForm::UniqueMinimumApply => "unique_minimum_apply",
                AuthorityNormalForm::UniqueOversizedApply => "unique_oversized_apply",
                AuthorityNormalForm::SplitLifecycleAuthorities => "split_lifecycle_authorities",
                AuthorityNormalForm::UniversalActor => "universal_actor",
            },
            match self.lifecycle {
                LifecycleNormalForm::SealedLinearExact => "sealed_linear_exact",
                LifecycleNormalForm::SealedLinearWithRollback => "sealed_linear_with_rollback",
                LifecycleNormalForm::CopiedOrInferredEvidence => "copied_or_inferred_evidence",
            },
            match self.coupling {
                CouplingNormalForm::ExactCanonicalAvailableCut => "exact_canonical_available_cut",
                CouplingNormalForm::PerOwnerIndependentCuts => "per_owner_independent_cuts",
                CouplingNormalForm::TimerOrApproximateBatch => "timer_or_approximate_batch",
            },
            match self.chain_completion {
                ChainCompletionTopology::ApplyAcknowledged => "apply_acknowledged",
                ChainCompletionTopology::EnqueueOnly => "enqueue_only",
            },
            match self.proposal_history {
                ProposalHistoryNormalForm::BoundedPersistentSparseExactViewIndependentVerifier => {
                    "bounded_persistent_sparse_exact_view_independent_verifier"
                }
                ProposalHistoryNormalForm::ExpectedBoundPersistentSparseExactViewIndependentVerifier => {
                    "expected_bound_persistent_sparse_exact_view_independent_verifier"
                }
                ProposalHistoryNormalForm::MaterializedIncrementalExactViewIndependentVerifier => {
                    "materialized_incremental_exact_view_independent_verifier"
                }
                ProposalHistoryNormalForm::RecomputedExactViewIndependentVerifier => {
                    "recomputed_exact_view_independent_verifier"
                }
                ProposalHistoryNormalForm::SharedMutableVerifierView => {
                    "shared_mutable_verifier_view"
                }
                ProposalHistoryNormalForm::ScalarOrCurrentHistory => "scalar_or_current_history",
            },
            match self.template_chain_read {
                TemplateChainReadTopology::SourceGatedTerminal => "source_gated_terminal",
                TemplateChainReadTopology::SourceScalarGatedTerminal => {
                    "source_scalar_gated_terminal"
                }
                TemplateChainReadTopology::UngatedLastPublished => "ungated_last_published",
                TemplateChainReadTopology::SourceGatedUnboundedWait => {
                    "source_gated_unbounded_wait"
                }
                TemplateChainReadTopology::TimerOrPollingFreshness => "timer_or_polling_freshness",
            },
            match self.progress {
                ProgressNormalForm::SameApplyDerivedFiniteRank => "same_apply_derived_finite_rank",
                ProgressNormalForm::SameApplyWithResidentDerivedDag => {
                    "same_apply_with_resident_derived_dag"
                }
                ProgressNormalForm::ResidentDecisionAuthority => "resident_decision_authority",
                ProgressNormalForm::PollingOrUnchangedRetry => "polling_or_unchanged_retry",
            },
            match self.resources {
                ResourceNormalForm::UnifiedBoundedFallible => "unified_bounded_fallible",
                ResourceNormalForm::PartitionedBoundedCharged => "partitioned_bounded_charged",
                ResourceNormalForm::FragmentedOrUncharged => "fragmented_or_uncharged",
                ResourceNormalForm::InfallibleOrUnbounded => "infallible_or_unbounded",
            },
            match self.allocation {
                AllocationNormalForm::AllocationFreeGenerationTerminal => {
                    "allocation_free_generation_terminal"
                }
                AllocationNormalForm::BoundedScopedRecovery => "bounded_scoped_recovery",
                AllocationNormalForm::UnchangedCutRetry => "unchanged_cut_retry",
                AllocationNormalForm::FailStopOrCapabilityLeak => "fail_stop_or_capability_leak",
            },
            match self.projections {
                ProjectionNormalForm::PostCommitDerivedPrepared => "post_commit_derived_prepared",
                ProjectionNormalForm::PostCommitWithResidentDerivedProjection => {
                    "post_commit_with_resident_derived_projection"
                }
                ProjectionNormalForm::ExternalVetoOrGuardIo => "external_veto_or_guard_io",
                ProjectionNormalForm::ResidentPolicyProjection => "resident_policy_projection",
            },
            match self.tasks {
                TaskNormalForm::BoundedOwnedMinimalLanes => "bounded_owned_minimal_lanes",
                TaskNormalForm::BoundedOwnedExtraActor => "bounded_owned_extra_actor",
                TaskNormalForm::UniversalSerializedLoop => "universal_serialized_loop",
                TaskNormalForm::DetachedOrRepairTopology => "detached_or_repair_topology",
            },
            match self.compatibility {
                CompatibilityNormalForm::ForwardLegacyIntentionalMajor => {
                    "forward_legacy_intentional_major"
                }
                CompatibilityNormalForm::ForwardLegacyMajorWithNonAuthoritativeFacade => {
                    "forward_legacy_major_with_non_authoritative_facade"
                }
                CompatibilityNormalForm::DropsSupportedLegacy => "drops_supported_legacy",
                CompatibilityNormalForm::ReverseMigrationOrAuthorityFacade => {
                    "reverse_migration_or_authority_facade"
                }
            },
        ]
    }

    pub(super) const fn is_non_authoritative_facade_variant(self) -> bool {
        matches!(
            self.compatibility,
            CompatibilityNormalForm::ForwardLegacyMajorWithNonAuthoritativeFacade
        )
    }
}

const AUTHORITIES: [AuthorityNormalForm; 4] = [
    AuthorityNormalForm::UniqueMinimumApply,
    AuthorityNormalForm::UniqueOversizedApply,
    AuthorityNormalForm::SplitLifecycleAuthorities,
    AuthorityNormalForm::UniversalActor,
];
const LIFECYCLES: [LifecycleNormalForm; 3] = [
    LifecycleNormalForm::SealedLinearExact,
    LifecycleNormalForm::SealedLinearWithRollback,
    LifecycleNormalForm::CopiedOrInferredEvidence,
];
const COUPLINGS: [CouplingNormalForm; 3] = [
    CouplingNormalForm::ExactCanonicalAvailableCut,
    CouplingNormalForm::PerOwnerIndependentCuts,
    CouplingNormalForm::TimerOrApproximateBatch,
];
const CHAIN_COMPLETIONS: [ChainCompletionTopology; 2] = [
    ChainCompletionTopology::ApplyAcknowledged,
    ChainCompletionTopology::EnqueueOnly,
];
const PROPOSAL_HISTORIES: [ProposalHistoryNormalForm; 6] = [
    ProposalHistoryNormalForm::BoundedPersistentSparseExactViewIndependentVerifier,
    ProposalHistoryNormalForm::ExpectedBoundPersistentSparseExactViewIndependentVerifier,
    ProposalHistoryNormalForm::MaterializedIncrementalExactViewIndependentVerifier,
    ProposalHistoryNormalForm::RecomputedExactViewIndependentVerifier,
    ProposalHistoryNormalForm::SharedMutableVerifierView,
    ProposalHistoryNormalForm::ScalarOrCurrentHistory,
];
const TEMPLATE_CHAIN_READS: [TemplateChainReadTopology; 5] = [
    TemplateChainReadTopology::SourceGatedTerminal,
    TemplateChainReadTopology::SourceScalarGatedTerminal,
    TemplateChainReadTopology::UngatedLastPublished,
    TemplateChainReadTopology::SourceGatedUnboundedWait,
    TemplateChainReadTopology::TimerOrPollingFreshness,
];
const PROGRESS_FORMS: [ProgressNormalForm; 4] = [
    ProgressNormalForm::SameApplyDerivedFiniteRank,
    ProgressNormalForm::SameApplyWithResidentDerivedDag,
    ProgressNormalForm::ResidentDecisionAuthority,
    ProgressNormalForm::PollingOrUnchangedRetry,
];
const RESOURCES: [ResourceNormalForm; 4] = [
    ResourceNormalForm::UnifiedBoundedFallible,
    ResourceNormalForm::PartitionedBoundedCharged,
    ResourceNormalForm::FragmentedOrUncharged,
    ResourceNormalForm::InfallibleOrUnbounded,
];
const ALLOCATIONS: [AllocationNormalForm; 4] = [
    AllocationNormalForm::AllocationFreeGenerationTerminal,
    AllocationNormalForm::BoundedScopedRecovery,
    AllocationNormalForm::UnchangedCutRetry,
    AllocationNormalForm::FailStopOrCapabilityLeak,
];
const PROJECTIONS: [ProjectionNormalForm; 4] = [
    ProjectionNormalForm::PostCommitDerivedPrepared,
    ProjectionNormalForm::PostCommitWithResidentDerivedProjection,
    ProjectionNormalForm::ExternalVetoOrGuardIo,
    ProjectionNormalForm::ResidentPolicyProjection,
];
const TASKS: [TaskNormalForm; 4] = [
    TaskNormalForm::BoundedOwnedMinimalLanes,
    TaskNormalForm::BoundedOwnedExtraActor,
    TaskNormalForm::UniversalSerializedLoop,
    TaskNormalForm::DetachedOrRepairTopology,
];
const COMPATIBILITIES: [CompatibilityNormalForm; 4] = [
    CompatibilityNormalForm::ForwardLegacyIntentionalMajor,
    CompatibilityNormalForm::ForwardLegacyMajorWithNonAuthoritativeFacade,
    CompatibilityNormalForm::DropsSupportedLegacy,
    CompatibilityNormalForm::ReverseMigrationOrAuthorityFacade,
];

const AUTHORITY_VARIANT_NAMES: [&str; 4] = [
    "unique_minimum_apply",
    "unique_oversized_apply",
    "split_lifecycle_authorities",
    "universal_actor",
];
const LIFECYCLE_VARIANT_NAMES: [&str; 3] = [
    "sealed_linear_exact",
    "sealed_linear_with_rollback",
    "copied_or_inferred_evidence",
];
const COUPLING_VARIANT_NAMES: [&str; 3] = [
    "exact_canonical_available_cut",
    "per_owner_independent_cuts",
    "timer_or_approximate_batch",
];
const CHAIN_COMPLETION_VARIANT_NAMES: [&str; 2] = ["apply_acknowledged", "enqueue_only"];
const PROPOSAL_HISTORY_VARIANT_NAMES: [&str; 6] = [
    "bounded_persistent_sparse_exact_view_independent_verifier",
    "expected_bound_persistent_sparse_exact_view_independent_verifier",
    "materialized_incremental_exact_view_independent_verifier",
    "recomputed_exact_view_independent_verifier",
    "shared_mutable_verifier_view",
    "scalar_or_current_history",
];
const TEMPLATE_CHAIN_READ_VARIANT_NAMES: [&str; 5] = [
    "source_gated_terminal",
    "source_scalar_gated_terminal",
    "ungated_last_published",
    "source_gated_unbounded_wait",
    "timer_or_polling_freshness",
];
const PROGRESS_VARIANT_NAMES: [&str; 4] = [
    "same_apply_derived_finite_rank",
    "same_apply_with_resident_derived_dag",
    "resident_decision_authority",
    "polling_or_unchanged_retry",
];
const RESOURCE_VARIANT_NAMES: [&str; 4] = [
    "unified_bounded_fallible",
    "partitioned_bounded_charged",
    "fragmented_or_uncharged",
    "infallible_or_unbounded",
];
const ALLOCATION_VARIANT_NAMES: [&str; 4] = [
    "allocation_free_generation_terminal",
    "bounded_scoped_recovery",
    "unchanged_cut_retry",
    "fail_stop_or_capability_leak",
];
const PROJECTION_VARIANT_NAMES: [&str; 4] = [
    "post_commit_derived_prepared",
    "post_commit_with_resident_derived_projection",
    "external_veto_or_guard_io",
    "resident_policy_projection",
];
const TASK_VARIANT_NAMES: [&str; 4] = [
    "bounded_owned_minimal_lanes",
    "bounded_owned_extra_actor",
    "universal_serialized_loop",
    "detached_or_repair_topology",
];
const COMPATIBILITY_VARIANT_NAMES: [&str; 4] = [
    "forward_legacy_intentional_major",
    "forward_legacy_major_with_non_authoritative_facade",
    "drops_supported_legacy",
    "reverse_migration_or_authority_facade",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct GlobalOptimalitySummary {
    pub(super) axis_cardinalities: [usize; 12],
    pub(super) axis_variants: [&'static [&'static str]; 12],
    pub(super) total_normal_forms: usize,
    pub(super) feasible_normal_forms: usize,
    pub(super) rejected_normal_forms: usize,
    pub(super) rejected_by_law: [usize; 3],
    pub(super) minimum_static_extra_cost: [u32; 7],
    pub(super) static_minimizers: usize,
    pub(super) minimum_static_signature: [&'static str; 12],
    pub(super) minimum_facade_static_extra_cost: [u32; 7],
    pub(super) minimum_partitioned_resource_static_extra_cost: [u32; 7],
    pub(super) minimum_scoped_allocation_static_extra_cost: [u32; 7],
}

pub(super) fn global_optimality_summary() -> GlobalOptimalitySummary {
    let mut summary = GlobalOptimalitySummary {
        axis_cardinalities: [
            AUTHORITIES.len(),
            LIFECYCLES.len(),
            COUPLINGS.len(),
            CHAIN_COMPLETIONS.len(),
            PROPOSAL_HISTORIES.len(),
            TEMPLATE_CHAIN_READS.len(),
            PROGRESS_FORMS.len(),
            RESOURCES.len(),
            ALLOCATIONS.len(),
            PROJECTIONS.len(),
            TASKS.len(),
            COMPATIBILITIES.len(),
        ],
        axis_variants: [
            &AUTHORITY_VARIANT_NAMES,
            &LIFECYCLE_VARIANT_NAMES,
            &COUPLING_VARIANT_NAMES,
            &CHAIN_COMPLETION_VARIANT_NAMES,
            &PROPOSAL_HISTORY_VARIANT_NAMES,
            &TEMPLATE_CHAIN_READ_VARIANT_NAMES,
            &PROGRESS_VARIANT_NAMES,
            &RESOURCE_VARIANT_NAMES,
            &ALLOCATION_VARIANT_NAMES,
            &PROJECTION_VARIANT_NAMES,
            &TASK_VARIANT_NAMES,
            &COMPATIBILITY_VARIANT_NAMES,
        ],
        total_normal_forms: 0,
        feasible_normal_forms: 0,
        rejected_normal_forms: 0,
        rejected_by_law: [0; 3],
        minimum_static_extra_cost: [u32::MAX; 7],
        static_minimizers: 0,
        minimum_static_signature: [""; 12],
        minimum_facade_static_extra_cost: [u32::MAX; 7],
        minimum_partitioned_resource_static_extra_cost: [u32::MAX; 7],
        minimum_scoped_allocation_static_extra_cost: [u32::MAX; 7],
    };
    visit_global_normal_forms(|normal_form| {
        summary.total_normal_forms += 1;
        let gaps = normal_form.feasibility_gaps();
        if gaps.is_empty() {
            summary.feasible_normal_forms += 1;
            let cost = normal_form.static_extra_cost();
            if normal_form.is_non_authoritative_facade_variant() {
                summary.minimum_facade_static_extra_cost =
                    summary.minimum_facade_static_extra_cost.min(cost);
            }
            if matches!(
                normal_form.resources,
                ResourceNormalForm::PartitionedBoundedCharged
            ) {
                summary.minimum_partitioned_resource_static_extra_cost = summary
                    .minimum_partitioned_resource_static_extra_cost
                    .min(cost);
            }
            if matches!(
                normal_form.allocation,
                AllocationNormalForm::BoundedScopedRecovery
            ) {
                summary.minimum_scoped_allocation_static_extra_cost = summary
                    .minimum_scoped_allocation_static_extra_cost
                    .min(cost);
            }
            if cost < summary.minimum_static_extra_cost {
                summary.minimum_static_extra_cost = cost;
                summary.static_minimizers = 1;
                summary.minimum_static_signature = normal_form.signature();
            } else if cost == summary.minimum_static_extra_cost {
                summary.static_minimizers += 1;
            }
            return;
        }

        summary.rejected_normal_forms += 1;
        let mut laws = [false; 3];
        for gap in gaps {
            laws[gap.law().index()] = true;
        }
        for (count, observed) in summary.rejected_by_law.iter_mut().zip(laws) {
            *count += usize::from(observed);
        }
    });
    summary
}

pub(super) fn visit_global_normal_forms(mut visit: impl FnMut(GlobalNormalForm)) {
    for authority in AUTHORITIES {
        for lifecycle in LIFECYCLES {
            for coupling in COUPLINGS {
                for chain_completion in CHAIN_COMPLETIONS {
                    for proposal_history in PROPOSAL_HISTORIES {
                        for template_chain_read in TEMPLATE_CHAIN_READS {
                            for progress in PROGRESS_FORMS {
                                for resources in RESOURCES {
                                    for allocation in ALLOCATIONS {
                                        for projections in PROJECTIONS {
                                            for tasks in TASKS {
                                                for compatibility in COMPATIBILITIES {
                                                    visit(GlobalNormalForm {
                                                        authority,
                                                        lifecycle,
                                                        coupling,
                                                        chain_completion,
                                                        proposal_history,
                                                        template_chain_read,
                                                        progress,
                                                        resources,
                                                        allocation,
                                                        projections,
                                                        tasks,
                                                        compatibility,
                                                    });
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
