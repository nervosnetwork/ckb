use super::*;
use ckb_verification::cache::ScriptVerificationRules;

impl PayloadPolicy {
    pub(in crate::authority) const fn remote_for_foundation(
        declared: ckb_types::core::Cycle,
    ) -> Self {
        Self::RemoteDeclaredCycles(super::super::ingress::RemoteCycleLimit::for_foundation(
            declared,
        ))
    }
}

pub(in crate::authority) struct FoundationResolution {
    payload: ResolvedPayload,
    location: CellLocationReceipt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) enum FoundationInputEvidenceError {
    Evidence(InputEvidenceError),
    NotAnInput,
    NotADependency,
}

impl From<InputEvidenceError> for FoundationInputEvidenceError {
    fn from(error: InputEvidenceError) -> Self {
        Self::Evidence(error)
    }
}

impl FoundationResolution {
    pub(in crate::authority) fn into_payload(self) -> ResolvedPayload {
        self.payload
    }

    pub(in crate::authority) fn is_chain_input(&self, input: &OutPoint) -> bool {
        self.location.is_chain_input(input)
    }

    pub(in crate::authority) fn is_chain_dependency(&self, dependency: &OutPoint) -> bool {
        self.location.is_chain_dependency(dependency)
    }
}

impl std::ops::Deref for FoundationResolution {
    type Target = ResolvedPayload;

    fn deref(&self) -> &Self::Target {
        &self.payload
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) enum RejectionKind {
    Verification,
    Policy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::authority) enum DependencyObservationError {
    Empty,
}

impl ChainViewId {
    pub(in crate::authority) fn initial() -> Self {
        Self::new(ChainRevision(0), Byte32::zero())
    }
}

impl AcceptedAtMillis {
    pub(in crate::authority) const FOUNDATION: Self = Self(0);
}

impl RemoteResidencyLease {
    pub(in crate::authority) const fn for_foundation(peer: PeerIndex) -> Self {
        Self::new(peer, RemoteDeadline(u64::MAX))
    }
}

impl ResolvedPayload {
    pub(in crate::authority) fn for_foundation(
        tx: &TransactionView,
        mut expanded_dependencies: Vec<OutPoint>,
        max_edges: usize,
        fee: Capacity,
        resolved_resident_bytes: usize,
        chain_inputs: Vec<OutPoint>,
        chain_dependencies: Vec<OutPoint>,
    ) -> Result<FoundationResolution, FoundationInputEvidenceError> {
        let mut chain_inputs = chain_inputs;
        chain_inputs.sort_unstable();
        chain_inputs.dedup();
        let mut chain_dependencies = chain_dependencies;
        chain_dependencies.sort_unstable();
        chain_dependencies.dedup();
        let mut resolved = ResolvedTransaction::dummy_resolve(tx.clone());
        if chain_inputs.iter().any(|input| {
            resolved
                .resolved_inputs
                .iter()
                .all(|cell| &cell.out_point != input)
        }) {
            return Err(FoundationInputEvidenceError::NotAnInput);
        }
        for cell in &mut resolved.resolved_inputs {
            if chain_inputs.binary_search(&cell.out_point).is_ok() {
                cell.transaction_info = Some(ckb_types::core::TransactionInfo::new(
                    1,
                    ckb_types::core::EpochNumberWithFraction::new(1, 0, 1),
                    Byte32::zero(),
                    1,
                ));
            }
        }
        expanded_dependencies.extend(
            tx.cell_deps()
                .into_iter()
                .map(|dependency| dependency.out_point()),
        );
        expanded_dependencies.sort_unstable();
        expanded_dependencies.dedup();
        resolved.resolved_cell_deps = expanded_dependencies
            .into_iter()
            .map(|out_point| {
                let mut cell = ckb_types::core::cell::CellMetaBuilder::default()
                    .out_point(out_point)
                    .build();
                if chain_dependencies.binary_search(&cell.out_point).is_ok() {
                    cell.transaction_info = Some(ckb_types::core::TransactionInfo::new(
                        1,
                        ckb_types::core::EpochNumberWithFraction::new(1, 0, 1),
                        Byte32::zero(),
                        1,
                    ));
                }
                cell
            })
            .collect();
        resolved.resolved_dep_groups.clear();
        if chain_dependencies.iter().any(|dependency| {
            resolved
                .related_dep_out_points()
                .all(|resolved| resolved != dependency)
        }) {
            return Err(FoundationInputEvidenceError::NotADependency);
        }
        let payload =
            Self::from_resolved_parts(Arc::new(resolved), max_edges, fee, resolved_resident_bytes)?;
        let location = CellLocationReceipt::from_resolution(ChainViewId::initial(), &payload)
            .expect("foundation location scratch is available");
        Ok(FoundationResolution { payload, location })
    }
}

impl ReplacementHistoryEntry {
    pub(in crate::authority) fn charge(&self) -> ResourceVector {
        self.charge
    }
}

impl OwnedTx {
    pub(in crate::authority) fn preaccepted_charge(&self) -> Option<ResourceVector> {
        match self {
            Self::PreAccepted(entry) => Some(entry.charge),
            Self::Accepted(_) | Self::ReplacementHistory(_) => None,
        }
    }
}

impl ResolvedFacts {
    pub(in crate::authority) fn for_foundation(
        chain_revision: ChainRevision,
        dependency_cut: DependencyCut,
        payload: Arc<ResolvedPayload>,
        verify_class: VerifyCycleClass,
    ) -> Self {
        Self::for_foundation_view(
            ChainViewId::new(chain_revision, Byte32::zero()),
            dependency_cut,
            payload,
            verify_class,
        )
    }

    pub(in crate::authority) fn for_foundation_view(
        chain_view: ChainViewId,
        dependency_cut: DependencyCut,
        payload: Arc<ResolvedPayload>,
        verify_class: VerifyCycleClass,
    ) -> Self {
        let location = CellLocationReceipt::empty_for_foundation(&chain_view);
        Self {
            dependency_cut,
            content: CellContentReceipt::from_resolution(payload),
            location,
            verify_class,
        }
    }

    pub(in crate::authority) fn equivalent_after_atomic_stamp_compaction(
        &self,
        other: &Self,
        batch: ApplySequence,
        canonical_next: ApplySequence,
    ) -> bool {
        self.dependency_cut == compact_dependency_cut(other.dependency_cut, batch, canonical_next)
            && self.content == other.content
            && self.location == other.location
            && self.verify_class == other.verify_class
    }
}

impl VerifiedFacts {
    pub(in crate::authority) fn for_foundation(
        chain_revision: ChainRevision,
        dependency_cut: DependencyCut,
        payload: Arc<ResolvedPayload>,
        metrics: CandidateMetrics,
    ) -> Self {
        Self::for_foundation_view(
            ChainViewId::new(chain_revision, Byte32::zero()),
            dependency_cut,
            payload,
            metrics,
        )
    }

    pub(in crate::authority) fn for_foundation_view(
        chain_view: ChainViewId,
        dependency_cut: DependencyCut,
        payload: Arc<ResolvedPayload>,
        metrics: CandidateMetrics,
    ) -> Self {
        Self::for_foundation_view_with_cells(
            chain_view,
            dependency_cut,
            payload,
            Vec::new(),
            Vec::new(),
            metrics,
        )
    }

    pub(in crate::authority) fn for_foundation_view_with_cells(
        chain_view: ChainViewId,
        dependency_cut: DependencyCut,
        payload: Arc<ResolvedPayload>,
        chain_inputs: Vec<OutPoint>,
        chain_dependencies: Vec<OutPoint>,
        metrics: CandidateMetrics,
    ) -> Self {
        let rules = ScriptVerificationRules::V0;
        let context = VerificationContextReceipt::with_cells_for_foundation(
            chain_view,
            chain_inputs,
            chain_dependencies,
            rules,
        );
        Self {
            dependency_cut,
            content: CellContentReceipt::from_resolution(payload),
            context,
            script: ScriptReceipt::from_verification(rules),
            verify_class: VerifyCycleClass::Small,
            metrics,
            async_process_start: None,
        }
    }

    pub(in crate::authority) fn with_context_for_foundation(
        self,
        context: VerificationContextReceipt,
    ) -> Option<Self> {
        if !self.script.is_reusable_under(context.rules()) {
            return None;
        }
        Some(Self { context, ..self })
    }

    pub(in crate::authority) fn equivalent_after_atomic_stamp_compaction(
        &self,
        other: &Self,
        batch: ApplySequence,
        canonical_next: ApplySequence,
    ) -> bool {
        self.dependency_cut == compact_dependency_cut(other.dependency_cut, batch, canonical_next)
            && self.content == other.content
            && self.context == other.context
            && self.script == other.script
            && self.verify_class == other.verify_class
            && self.metrics == other.metrics
            && self.async_process_start == other.async_process_start
    }
}

impl ObservedDependencies {
    pub(in crate::authority) fn for_foundation(
        dependencies: Vec<DependencyKey>,
        dependency_cut: DependencyCut,
    ) -> Result<Self, DependencyObservationError> {
        let max = dependencies.len();
        let dependencies = KnownDependencies::canonicalize_nonempty(dependencies, max)
            .map_err(|_| DependencyObservationError::Empty)?;
        Ok(Self {
            dependency_cut,
            observed: dependencies.clone(),
            retained: dependencies,
        })
    }

    pub(in crate::authority) fn len(&self) -> usize {
        self.observed.len()
    }

    fn equivalent_after_atomic_stamp_compaction(
        &self,
        other: &Self,
        batch: ApplySequence,
        canonical_next: ApplySequence,
    ) -> bool {
        self.dependency_cut == compact_dependency_cut(other.dependency_cut, batch, canonical_next)
            && self.observed == other.observed
            && self.retained == other.retained
    }
}

impl ActiveWork {
    fn equivalent_after_atomic_stamp_compaction(
        &self,
        other: &Self,
        batch: ApplySequence,
        canonical_next: ApplySequence,
    ) -> bool {
        self.chain_view == other.chain_view
            && self.permit == other.permit
            && self.grant == other.grant
            && self.attribution == other.attribution
            && self.payload_policy == other.payload_policy
            && self.dependency_cut
                == compact_dependency_cut(other.dependency_cut, batch, canonical_next)
            && self.dependencies == other.dependencies
    }
}

impl PreAcceptedPhase {
    pub(in crate::authority) fn equivalent_after_atomic_stamp_compaction(
        &self,
        other: &Self,
        batch: ApplySequence,
        canonical_next: ApplySequence,
    ) -> bool {
        match (self, other) {
            (Self::Queued(QueuedWork::Resolve), Self::Queued(QueuedWork::Resolve)) => true,
            (Self::Queued(QueuedWork::Verify(left)), Self::Queued(QueuedWork::Verify(right))) => {
                left.equivalent_after_atomic_stamp_compaction(right, batch, canonical_next)
            }
            (Self::Computing(left), Self::Computing(right)) => {
                left.equivalent_after_atomic_stamp_compaction(right, batch, canonical_next)
            }
            (Self::Waiting(left), Self::Waiting(right)) => {
                left.equivalent_after_atomic_stamp_compaction(right, batch, canonical_next)
            }
            (Self::Ready(left), Self::Ready(right)) => {
                left.equivalent_after_atomic_stamp_compaction(right, batch, canonical_next)
            }
            (
                Self::Queued(_) | Self::Computing(_) | Self::Waiting(_) | Self::Ready(_),
                Self::Queued(_) | Self::Computing(_) | Self::Waiting(_) | Self::Ready(_),
            ) => false,
        }
    }
}

fn compact_dependency_cut(
    cut: DependencyCut,
    batch: ApplySequence,
    canonical_next: ApplySequence,
) -> DependencyCut {
    if cut.0 >= batch && cut.0 < canonical_next {
        DependencyCut(batch)
    } else {
        cut
    }
}

impl ValidatedAdmission {
    pub(in crate::authority) fn charge_for_foundation(&self) -> ResourceVector {
        ResourceVector::new(1, self.payload_bytes, self.encoded_edges, 0)
    }

    pub(in crate::authority) fn remote(
        tx: TransactionView,
        peer: PeerIndex,
    ) -> Result<Self, RecoveryAdmissionError> {
        Self::remote_with_lease(tx, RemoteResidencyLease::for_foundation(peer), 0)
    }

    pub(in crate::authority) fn remote_with_lease(
        tx: TransactionView,
        residency: RemoteResidencyLease,
        declared_cycles: ckb_types::core::Cycle,
    ) -> Result<Self, RecoveryAdmissionError> {
        let tx = super::super::ingress::BoundedTransaction::try_new(tx).map_err(
            |error| match error {
                super::super::ingress::BoundedTransactionError::Allocation => {
                    RecoveryAdmissionError::ResourceUnavailable
                }
                super::super::ingress::BoundedTransactionError::TooLarge { .. } => {
                    RecoveryAdmissionError::InvalidTransaction
                }
            },
        )?;
        Self::new(
            tx,
            PreAcceptedSource::Remote(RemoteBase::ingress(
                residency,
                super::super::ingress::RemoteCycleLimit::for_foundation(declared_cycles),
            )),
        )
        .map_err(|_| RecoveryAdmissionError::ResourceUnavailable)
    }

    pub(in crate::authority) fn proposal(
        tx: TransactionView,
    ) -> Result<Self, RecoveryAdmissionError> {
        let tx = super::super::ingress::BoundedTransaction::try_new(tx).map_err(
            |error| match error {
                super::super::ingress::BoundedTransactionError::Allocation => {
                    RecoveryAdmissionError::ResourceUnavailable
                }
                super::super::ingress::BoundedTransactionError::TooLarge { .. } => {
                    RecoveryAdmissionError::InvalidTransaction
                }
            },
        )?;
        Self::new(
            tx,
            PreAcceptedSource::Proposal {
                base: ProposalBase::Trusted,
            },
        )
        .map_err(|_| RecoveryAdmissionError::ResourceUnavailable)
    }
}
impl KnownDependencies {
    pub(in crate::authority) fn from_keys_for_foundation(
        keys: Vec<DependencyKey>,
    ) -> Result<Self, DependencySetError> {
        let max = keys.len();
        Self::canonicalize(keys, max)
    }
}

impl MissingDependencies {
    pub(in crate::authority) fn from_keys_for_foundation(
        keys: Vec<DependencyKey>,
    ) -> Result<Self, DependencySetError> {
        let max = keys.len();
        Self::new(keys, max)
    }
}
