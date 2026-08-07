//! Finite input algebra for model-to-production membership refinement.
//!
//! Only case data and normalized observations cross the module boundary.
//! Production tests reconstruct every case with production transactions and
//! call the production planner; they do not reuse this model classifier.

use super::{
    composition::{CouplingReason, analyze_ready_prefix},
    kernel::{Admission, Completion, KernelCommand, KernelDisposition, KernelStep, WorkResult},
    state::{
        CellId, EffectBatchBound, EffectClass, HeaderId, InputOrigin, LogicalEffect,
        ModelInvariantError, ModelLimits, MonotonicTick, Omega, PeerId, ProposalId, RemoteDeadline,
        RemoteResidency, ResolvedEvidence, RetainedSource, RulesId, Transaction, TxId, ViewId,
        WitnessId,
    },
};
use std::collections::BTreeSet;

pub(crate) const REFINEMENT_MAX_READY: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CellRole {
    None,
    Input,
    Read,
    Output,
}

impl CellRole {
    pub(crate) const ALL: [Self; 4] = [Self::None, Self::Input, Self::Read, Self::Output];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FrontierTerminal {
    Complete,
    Coupled,
    Stale,
    /// Two distinct transaction identities cannot produce the same exact
    /// outpoint. Such a symbolic role product is a premise check, not a state
    /// that either implementation may synthesize.
    DuplicateOutputIdentity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SourceRole {
    Trusted,
    Remote,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EffectPressure {
    RemoteFull,
    OrdinaryFull,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EvidenceOriginRole {
    ChainInput,
    ChainRead,
    PoolInput,
    PoolRead,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ReadyOrderInput {
    pub(crate) fee: u64,
    pub(crate) serialized_bytes: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FrontierObservation {
    pub(crate) prefix_len: usize,
    pub(crate) terminal: FrontierTerminal,
}

pub(crate) fn candidate_role_observation(left: CellRole, right: CellRole) -> FrontierObservation {
    positioned_role_observation(2, 0, left, 1, right)
}

pub(crate) fn positioned_role_observation(
    owner_count: usize,
    left_index: usize,
    left: CellRole,
    right_index: usize,
    right: CellRole,
) -> FrontierObservation {
    if left == CellRole::Output && right == CellRole::Output {
        return FrontierObservation {
            prefix_len: 0,
            terminal: FrontierTerminal::DuplicateOutputIdentity,
        };
    }
    let mut roles = vec![CellRole::None; owner_count];
    let Some(left_role) = roles.get_mut(left_index) else {
        return invalid_domain();
    };
    *left_role = left;
    let Some(right_role) = roles.get_mut(right_index) else {
        return invalid_domain();
    };
    *right_role = right;
    let shared = CellId(200);
    let transactions = roles
        .into_iter()
        .enumerate()
        .map(|(index, role)| transaction_for_role(index, role, shared))
        .collect::<Vec<_>>();
    observe_candidates(transactions)
}

pub(crate) fn accepted_role_observation(
    candidate: CellRole,
    accepted: CellRole,
) -> FrontierObservation {
    if candidate == CellRole::Output && accepted == CellRole::Output {
        return FrontierObservation {
            prefix_len: 0,
            terminal: FrontierTerminal::DuplicateOutputIdentity,
        };
    }
    let shared = CellId(200);
    let accepted_transaction = transaction_for_role(1, accepted, shared);
    let accepted_id = accepted_transaction.id;
    let candidate_transaction = transaction_for_role(0, candidate, shared);
    let mut omega = model(2);
    make_ready(&mut omega, accepted_transaction);
    let accepted_step = omega.kernel_step(KernelCommand::FinalizeNext { wall_time: 1 });
    if !matches!(
        accepted_step,
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::Accepted(_) | KernelDisposition::AcceptedBatch(_),
            ..
        }
    ) {
        return invalid_domain();
    }
    let mut candidate_evidence = ResolvedEvidence::for_transaction(
        &candidate_transaction,
        omega.authority.chain,
        omega.authority.rules,
    );
    if accepted == CellRole::Output {
        match candidate {
            CellRole::Input => {
                candidate_evidence
                    .input_origins
                    .insert(shared, InputOrigin::Pool(accepted_id));
            }
            CellRole::Read => {
                candidate_evidence
                    .dep_origins
                    .insert(shared, InputOrigin::Pool(accepted_id));
            }
            CellRole::None | CellRole::Output => {}
        }
    }
    make_ready_with_evidence(
        &mut omega,
        candidate_transaction,
        RetainedSource::Proposal,
        candidate_evidence,
    );
    normalize(analyze_ready_prefix(&omega, 1), 1)
}

/// Enumerates one undirected shared-input graph over four candidates. Bit
/// order is lexicographic over `(0,1),(0,2),(0,3),(1,2),(1,3),(2,3)`.
pub(crate) fn candidate_graph_observation(edge_mask: u8) -> FrontierObservation {
    let owner_count = 4usize;
    let mut transactions = (0..owner_count)
        .map(|index| transaction_for_role(index, CellRole::None, CellId(0)))
        .collect::<Vec<_>>();
    let mut bit = 0u8;
    for left in 0..owner_count {
        for right in (left + 1)..owner_count {
            if edge_mask & (1u8 << bit) != 0 {
                let cell = CellId(100u8.saturating_add(bit));
                transactions[left].inputs.insert(cell);
                transactions[right].inputs.insert(cell);
            }
            bit = bit.saturating_add(1);
        }
    }
    observe_candidates(transactions)
}

pub(crate) fn source_observation(sources: &[SourceRole]) -> FrontierObservation {
    if sources.is_empty() || sources.len() > REFINEMENT_MAX_READY {
        return invalid_domain();
    }
    let mut omega = model(sources.len());
    for (index, source) in sources.iter().copied().enumerate() {
        let transaction = transaction_for_role(index, CellRole::None, CellId(0));
        let source = match source {
            SourceRole::Trusted => RetainedSource::Proposal,
            SourceRole::Remote => RetainedSource::Remote(RemoteResidency {
                peer: PeerId(u8::try_from(index + 1).expect("finite peer index fits u8")),
                expires_at: RemoteDeadline(100),
            }),
        };
        make_ready_from_source(&mut omega, transaction, source);
    }
    normalize(analyze_ready_prefix(&omega, sources.len()), sources.len())
}

pub(crate) fn accepted_capacity_observation(
    candidate_count: usize,
    accepted_entries: u16,
) -> FrontierObservation {
    if candidate_count == 0 || candidate_count > REFINEMENT_MAX_READY {
        return invalid_domain();
    }
    let mut omega = model(candidate_count);
    omega.authority.limits.accepted.entries = accepted_entries;
    for index in 0..candidate_count {
        make_ready(
            &mut omega,
            transaction_for_role(index, CellRole::None, CellId(0)),
        );
    }
    normalize(
        analyze_ready_prefix(&omega, candidate_count),
        candidate_count,
    )
}

pub(crate) fn source_pressure_observation(
    source: SourceRole,
    pressure: EffectPressure,
) -> FrontierObservation {
    let mut omega = model(1);
    let transaction = transaction_for_role(0, CellRole::None, CellId(0));
    let retained_source = match source {
        SourceRole::Trusted => RetainedSource::Proposal,
        SourceRole::Remote => RetainedSource::Remote(RemoteResidency {
            peer: PeerId(1),
            expires_at: RemoteDeadline(100),
        }),
    };
    make_ready_from_source(&mut omega, transaction, retained_source);
    while omega.append_effect_fixture(
        EffectClass::Remote,
        vec![LogicalEffect::IngressReleased(TxId(200))],
    ) {}
    if pressure == EffectPressure::OrdinaryFull {
        while omega.append_effect_fixture(
            EffectClass::Trusted,
            vec![LogicalEffect::IngressReleased(TxId(201))],
        ) {}
    }
    normalize(analyze_ready_prefix(&omega, 1), 1)
}

pub(crate) fn stale_observation() -> FrontierObservation {
    let mut omega = model(1);
    make_ready(
        &mut omega,
        transaction_for_role(0, CellRole::None, CellId(0)),
    );
    omega.authority.chain = omega
        .authority
        .chain
        .advance(ViewId(2))
        .expect("the finite stale cut has a successor revision");
    normalize(analyze_ready_prefix(&omega, 1), 1)
}

pub(crate) fn shared_header_observation(owner_count: usize) -> FrontierObservation {
    if owner_count == 0 || owner_count > REFINEMENT_MAX_READY {
        return invalid_domain();
    }
    let mut transactions = (0..owner_count)
        .map(|index| transaction_for_role(index, CellRole::None, CellId(0)))
        .collect::<Vec<_>>();
    for transaction in &mut transactions {
        transaction.header_deps.insert(HeaderId(7));
    }
    observe_candidates(transactions)
}

pub(crate) fn ready_order_observation(items: &[ReadyOrderInput]) -> Vec<usize> {
    if items.is_empty() || items.len() > REFINEMENT_MAX_READY {
        return Vec::new();
    }
    let mut omega = model(items.len());
    for (index, item) in items.iter().copied().enumerate() {
        let mut transaction = transaction_for_role(index, CellRole::None, CellId(0));
        transaction.fee = item.fee;
        transaction.bytes = item.serialized_bytes;
        make_ready(&mut omega, transaction);
    }
    omega
        .ready_order()
        .into_iter()
        .map(|id| usize::from(id.0.saturating_sub(1)))
        .collect()
}

pub(crate) fn evidence_origin_observation(origin: EvidenceOriginRole) -> FrontierObservation {
    let shared = CellId(200);
    let candidate_role = match origin {
        EvidenceOriginRole::ChainInput | EvidenceOriginRole::PoolInput => CellRole::Input,
        EvidenceOriginRole::ChainRead | EvidenceOriginRole::PoolRead => CellRole::Read,
    };
    let mut omega = model(2);
    let pool_parent = match origin {
        EvidenceOriginRole::ChainInput | EvidenceOriginRole::ChainRead => None,
        EvidenceOriginRole::PoolInput | EvidenceOriginRole::PoolRead => {
            let parent = transaction_for_role(1, CellRole::Output, shared);
            let parent_id = parent.id;
            make_ready(&mut omega, parent);
            let accepted = omega.kernel_step(KernelCommand::FinalizeNext { wall_time: 1 });
            if !matches!(
                accepted,
                KernelStep::AuthorityCommit {
                    disposition: KernelDisposition::Accepted(_)
                        | KernelDisposition::AcceptedBatch(_),
                    ..
                }
            ) {
                return invalid_domain();
            }
            Some(parent_id)
        }
    };
    let candidate = transaction_for_role(0, candidate_role, shared);
    let mut evidence =
        ResolvedEvidence::for_transaction(&candidate, omega.authority.chain, omega.authority.rules);
    if let Some(parent) = pool_parent {
        match candidate_role {
            CellRole::Input => {
                evidence
                    .input_origins
                    .insert(shared, InputOrigin::Pool(parent));
            }
            CellRole::Read => {
                evidence
                    .dep_origins
                    .insert(shared, InputOrigin::Pool(parent));
            }
            CellRole::None | CellRole::Output => return invalid_domain(),
        }
    }
    make_ready_with_evidence(&mut omega, candidate, RetainedSource::Proposal, evidence);
    normalize(analyze_ready_prefix(&omega, 1), 1)
}

fn invalid_domain() -> FrontierObservation {
    FrontierObservation {
        prefix_len: 0,
        terminal: FrontierTerminal::Coupled,
    }
}

fn model(owner_count: usize) -> Omega {
    let mut limits = ModelLimits::small();
    let entries = u16::try_from(REFINEMENT_MAX_READY.max(owner_count))
        .expect("the finite refinement domain fits u16");
    limits.owners.entries = entries;
    limits.retained.entries = entries;
    limits.accepted.entries = entries;
    for partition in [
        &mut limits.owners,
        &mut limits.retained,
        &mut limits.accepted,
        &mut limits.replacement_history,
        &mut limits.remote_per_peer,
    ] {
        partition.bytes = 1_048_576;
        partition.edges = 256;
    }
    let largest_batch = entries
        .checked_add(1)
        .expect("the finite refinement effect batch fits u16");
    limits.effects.remote.bytes = 4_194_304;
    limits.effects.trusted_headroom.bytes = 4_194_304;
    limits.effects.critical_headroom.bytes = 4_194_304;
    let bound_bytes = 4_194_304;
    let bound = EffectBatchBound::new(largest_batch, bound_bytes);
    limits.effects.remote_bound = bound;
    limits.effects.trusted_bound = bound;
    limits.effects.critical_bound = bound;
    Omega::new(
        limits
            .validate()
            .expect("the finite refinement model has a valid bounded configuration"),
        ViewId(1),
        RulesId(1),
    )
}

fn transaction_for_role(index: usize, role: CellRole, shared: CellId) -> Transaction {
    let id = u8::try_from(index + 1).expect("the finite owner index fits u8");
    let mut transaction = Transaction {
        id: TxId(id),
        witness: WitnessId(id),
        proposal: ProposalId(id),
        inputs: BTreeSet::new(),
        deps: BTreeSet::new(),
        header_deps: BTreeSet::<HeaderId>::new(),
        outputs: BTreeSet::new(),
        bytes: 4,
        cycles: 0,
        fee: 1_000_000u64.saturating_sub(u64::from(id) * 10_000),
        verify_class: super::state::VerifyCycleClass::Small,
    };
    match role {
        CellRole::None => {}
        CellRole::Input => {
            transaction.inputs.insert(shared);
        }
        CellRole::Read => {
            transaction.deps.insert(shared);
        }
        CellRole::Output => {
            transaction.outputs.insert(shared);
        }
    }
    transaction
}

fn observe_candidates(transactions: Vec<Transaction>) -> FrontierObservation {
    let requested = transactions.len();
    if requested == 0 || requested > REFINEMENT_MAX_READY {
        return invalid_domain();
    }
    let mut omega = model(requested);
    for transaction in transactions {
        make_ready(&mut omega, transaction);
    }
    normalize(analyze_ready_prefix(&omega, requested), requested)
}

fn make_ready(omega: &mut Omega, transaction: Transaction) {
    let evidence = ResolvedEvidence::for_transaction(
        &transaction,
        omega.authority.chain,
        omega.authority.rules,
    );
    make_ready_with_evidence(omega, transaction, RetainedSource::Proposal, evidence);
}

fn make_ready_from_source(omega: &mut Omega, transaction: Transaction, source: RetainedSource) {
    let evidence = ResolvedEvidence::for_transaction(
        &transaction,
        omega.authority.chain,
        omega.authority.rules,
    );
    make_ready_with_evidence(omega, transaction, source, evidence);
}

fn make_ready_with_evidence(
    omega: &mut Omega,
    transaction: Transaction,
    source: RetainedSource,
    evidence: ResolvedEvidence,
) {
    let admitted = omega.kernel_step(KernelCommand::Admit(Admission {
        transaction: transaction.clone(),
        source,
        observed_at: MonotonicTick(1),
    }));
    assert!(
        matches!(admitted, KernelStep::AuthorityCommit { .. }),
        "finite refinement admission failed for {:?}: {admitted:?}",
        transaction.id
    );
    let resolve = checkout(omega);
    assert!(matches!(
        omega.kernel_step(KernelCommand::Complete(Completion {
            capability: resolve,
            result: WorkResult::Resolved(evidence),
        })),
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::Continued(_),
            ..
        }
    ));
    let verify = checkout(omega);
    let completed = omega.kernel_step(KernelCommand::Complete(Completion {
        capability: verify,
        result: WorkResult::Verified,
    }));
    assert!(
        matches!(
            completed,
            KernelStep::AuthorityCommit {
                disposition: KernelDisposition::Ready(_),
                ..
            }
        ),
        "finite refinement verification failed for {:?}: {completed:?}",
        transaction.id
    );
}

fn checkout(omega: &mut Omega) -> super::state::CapabilityId {
    match omega.kernel_step(KernelCommand::Checkout) {
        KernelStep::AuthorityCommit {
            disposition: KernelDisposition::CheckedOut(capability),
            ..
        } => capability.id,
        other => panic!("finite refinement checkout failed: {other:?}"),
    }
}

fn normalize(
    analysis: super::composition::ReadyComposition,
    requested: usize,
) -> FrontierObservation {
    let terminal = match analysis.stopped_by {
        None if analysis.prefix.len() == requested => FrontierTerminal::Complete,
        Some(CouplingReason::StaleEvidence(_)) => FrontierTerminal::Stale,
        Some(CouplingReason::InvalidInitial(ModelInvariantError::StaleChainOrigin)) => {
            FrontierTerminal::Stale
        }
        Some(CouplingReason::InvalidInitial(_)) => FrontierTerminal::Coupled,
        Some(_) | None => FrontierTerminal::Coupled,
    };
    FrontierObservation {
        prefix_len: analysis.prefix.len(),
        terminal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finite_role_products_and_graph_masks_are_total() {
        let candidate_cases = CellRole::ALL
            .into_iter()
            .flat_map(|left| CellRole::ALL.into_iter().map(move |right| (left, right)))
            .count();
        let accepted_cases = CellRole::ALL
            .into_iter()
            .flat_map(|candidate| {
                CellRole::ALL
                    .into_iter()
                    .map(move |accepted| (candidate, accepted))
            })
            .count();
        assert_eq!((candidate_cases, accepted_cases), (16, 16));
        for left in CellRole::ALL {
            for right in CellRole::ALL {
                let _ = candidate_role_observation(left, right);
                let _ = accepted_role_observation(left, right);
            }
        }
        for mask in 0u8..64 {
            let _ = candidate_graph_observation(mask);
        }
    }

    #[test]
    fn adding_candidate_relations_never_extends_the_independent_prefix() {
        for subset in 0u8..64 {
            let baseline = candidate_graph_observation(subset);
            for superset in 0u8..64 {
                if subset & superset != subset {
                    continue;
                }
                let strengthened = candidate_graph_observation(superset);
                assert!(
                    strengthened.prefix_len <= baseline.prefix_len,
                    "edge superset {superset:06b} extended subset {subset:06b}"
                );
            }
        }
    }
}
