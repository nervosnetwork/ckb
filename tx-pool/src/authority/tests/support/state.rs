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
        Ok(FoundationResolution { payload })
    }
}

impl ReplacementHistoryEntry {
    pub(in crate::authority) fn charge(&self) -> ResourceVector {
        self.charge
    }
}

impl VerifiedFacts {
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
}

impl ObservedDependencies {
    pub(in crate::authority) fn len(&self) -> usize {
        self.observed.len()
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
