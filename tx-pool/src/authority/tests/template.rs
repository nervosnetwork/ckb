use super::super::{
    chain::{ChainBlockChanges, ProposalWindowPosition, test_support::ChainTransitionFacts},
    packing::TemplatePackingLimits,
    plan::TxPoolAuthority,
    state::{AcceptedStatus, ChainRevision, ChainViewId, OwnedTx, ProposalId, RawTxHash},
    template::{
        FullTemplateBuild, PartialTemplateBuild, ResetTemplateBuild, TemplateChainReadState,
        TemplateComponent, TemplateConvergence, TemplateConvergenceError, TemplatePublication,
        TemplateSourceCut,
    },
};
use super::foundation::{
    accept_remote_transaction, accept_remote_transaction_with_payload, admit_remote, apply_plan,
    assert_membership_reference, genesis_snapshot, limits, owner_version,
    resolved_payload_with_facts, tx,
};
use crate::block_assembler::{
    CandidateUncleSourceReceipt, CandidateUncles, ResetEpoch, TemplateRevision,
};
use ckb_types::{
    bytes::Bytes,
    core::{BlockBuilder, Capacity, TransactionBuilder, TransactionView},
    packed::{Byte32, CellDep, CellInput, CellOutput, OutPoint, ProposalShortId},
    prelude::{Builder, Entity, Pack},
};
use std::collections::{BTreeSet, HashMap, HashSet};

fn assert_model_accepted_order(selection: &super::super::template::TemplateSelectionReceipt) {
    use crate::mathematical_model::{
        EvictionRefinementMetrics,
        two_phase::{AcceptedOrderInput, accepted_order_refinement},
    };

    let inputs = selection
        .candidates()
        .iter()
        .map(|candidate| {
            let metrics = candidate.metrics();
            let ancestors = candidate.ancestors();
            let mut identity = [0; 32];
            identity.copy_from_slice(candidate.hash().0.as_slice());
            AcceptedOrderInput::new(
                EvictionRefinementMetrics::new(
                    metrics.fee.as_u64(),
                    u64::try_from(metrics.cost.serialized_bytes)
                        .expect("the captured serialized size fits u64"),
                    metrics.cost.cycles,
                ),
                EvictionRefinementMetrics::new(
                    ancestors.fee.as_u64(),
                    u64::try_from(ancestors.serialized_bytes)
                        .expect("the captured ancestor size fits u64"),
                    ancestors.cycles,
                ),
                candidate.order().arrival().0,
                identity,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        accepted_order_refinement(&inputs)
            .expect("the production receipt has unique raw identities"),
        (0..selection.candidates().len()).collect::<Vec<_>>(),
        "the immutable production receipt is already strongest-first under the independent model quotient"
    );
}

fn model_conditional_selection(
    inputs: Vec<crate::mathematical_model::two_phase::ConditionalSelectionInput>,
    identities: &[RawTxHash],
) -> (Vec<RawTxHash>, BTreeSet<RawTxHash>) {
    use crate::mathematical_model::two_phase::conditional_template_selection_refinement;

    let observation = conditional_template_selection_refinement(&inputs)
        .expect("the primitive production graph has one finite model observation");
    let ordered = observation
        .ordered
        .into_iter()
        .map(|index| identities[index].clone())
        .collect();
    let shed = observation
        .shed
        .into_iter()
        .map(|index| identities[index].clone())
        .collect();
    (ordered, shed)
}

fn exact_template_service_premise(
    selection: &super::super::template::TemplateSelectionReceipt,
    max_block_proposals: usize,
    serialized_bytes: usize,
    cycles: u64,
) -> crate::mathematical_model::two_phase::TemplateServicePremise {
    use crate::mathematical_model::{
        AcceptedStatus as ModelAcceptedStatus, EvictionRefinementMetrics,
        two_phase::{
            CurrentTemplateComposition, TemplateServiceCandidate, TemplateServicePremise,
            TemplateServiceSourceCut,
        },
    };

    let candidates = selection.candidates();
    assert!(candidates.len() <= usize::from(u8::MAX) + 1);
    let by_hash = selection
        .candidate_index()
        .expect("the captured production identities are unique");

    let mut spenders = HashMap::new();
    for (index, candidate) in candidates.iter().enumerate() {
        for input in candidate.resolved().transaction.input_pts_iter() {
            assert!(
                spenders.insert(input, index).is_none(),
                "the accepted authority cut has one spender per input"
            );
        }
    }
    let mut conditional_predecessors = vec![BTreeSet::new(); candidates.len()];
    for (reader, candidate) in candidates.iter().enumerate() {
        for dependency in candidate.resolved().related_dep_out_points() {
            if let Some(spender) = spenders.get(dependency).copied()
                && spender != reader
            {
                conditional_predecessors[spender].insert(reader);
            }
        }
    }

    let mut weakest_first = (0..candidates.len()).collect::<Vec<_>>();
    weakest_first.sort_unstable_by(|left, right| {
        candidates[*left]
            .eviction_order()
            .cmp(candidates[*right].eviction_order())
    });
    let mut eviction_rank = vec![0usize; candidates.len()];
    for (rank, index) in weakest_first.into_iter().enumerate() {
        eviction_rank[index] = rank;
    }

    let primitive_candidates = candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            let metrics = candidate.metrics();
            let mut identity = [0; 32];
            identity.copy_from_slice(candidate.hash().0.as_slice());
            TemplateServiceCandidate::from_primitive(
                u8::try_from(index).expect("the bounded fixture index fits u8"),
                match candidate.status() {
                    AcceptedStatus::Pending => ModelAcceptedStatus::Pending,
                    AcceptedStatus::Gap => ModelAcceptedStatus::Gap,
                    AcceptedStatus::Proposed => ModelAcceptedStatus::Proposed,
                },
                EvictionRefinementMetrics::new(
                    metrics.fee.as_u64(),
                    u64::try_from(metrics.cost.serialized_bytes)
                        .expect("the captured serialized size fits u64"),
                    metrics.cost.cycles,
                ),
                candidate
                    .parents()
                    .iter()
                    .map(|parent| by_hash[parent])
                    .collect(),
                conditional_predecessors[index].clone(),
                candidate.dependency_edge_count(),
                eviction_rank[index],
                candidate.order().arrival().0,
                identity,
            )
        })
        .collect::<Vec<_>>();
    let dependency_edge_bound = candidates
        .iter()
        .map(|candidate| candidate.dependency_edge_count())
        .sum();
    let proposal_bytes = ProposalShortId::serialized_size();
    let max_block_bytes = candidates
        .len()
        .min(max_block_proposals)
        .checked_mul(proposal_bytes)
        .and_then(|prefix| prefix.checked_add(serialized_bytes))
        .expect("the bounded fixture composition fits usize");
    TemplateServicePremise::compile_for_foundation(TemplateServiceSourceCut::new(
        primitive_candidates,
        CurrentTemplateComposition::new(
            max_block_proposals,
            proposal_bytes,
            Vec::new(),
            0,
            max_block_bytes,
            cycles,
        ),
        dependency_edge_bound,
    ))
    .expect("one exact production cut compiles into a sealed liveness premise")
}

fn source_cut(authority: &TxPoolAuthority, uncles: &CandidateUncles) -> TemplateSourceCut {
    let receipt = authority
        .read_view()
        .capture_template()
        .expect("fixture authority has a coherent template projection");
    receipt.source_cut(candidate_uncle_source(uncles))
}

fn candidate_uncle_source(uncles: &CandidateUncles) -> CandidateUncleSourceReceipt {
    let snapshot = genesis_snapshot();
    let epoch = snapshot.consensus().genesis_epoch_ext().clone();
    uncles
        .prepare_uncles(&snapshot, &epoch)
        .expect("bounded candidate fixture snapshot is allocatable")
        .into_parts()
        .2
}

/// Test projection paired with the pure convergence model. Production keeps
/// these tokens only on `CurrentTemplate`; this harness makes that external
/// ownership explicit instead of giving the model a shadow counter.
#[derive(Clone, Copy)]
struct TemplateProjection {
    revision: TemplateRevision,
    reset: ResetEpoch,
}

impl TemplateProjection {
    fn initial() -> Self {
        Self {
            revision: TemplateRevision::INITIAL,
            reset: ResetEpoch::INITIAL,
        }
    }

    fn begin_partial(
        self,
        convergence: &mut TemplateConvergence,
        component: TemplateComponent,
        sources: TemplateSourceCut,
    ) -> PartialTemplateBuild {
        convergence.begin_partial_for_foundation(component, sources, self.revision)
    }

    fn publish_full(
        &mut self,
        convergence: &mut TemplateConvergence,
        build: FullTemplateBuild,
    ) -> Result<TemplatePublication, TemplateConvergenceError> {
        let publication = convergence.publish_full(build, self.reset)?;
        self.advance_if_published(publication);
        Ok(publication)
    }

    fn publish_partial(
        &mut self,
        convergence: &mut TemplateConvergence,
        build: PartialTemplateBuild,
    ) -> Result<TemplatePublication, TemplateConvergenceError> {
        let publication = convergence.publish_partial(build, self.revision)?;
        self.advance_if_published(publication);
        Ok(publication)
    }

    fn publish_reset(
        &mut self,
        convergence: &mut TemplateConvergence,
        build: ResetTemplateBuild,
    ) -> Result<TemplatePublication, TemplateConvergenceError> {
        let reset = build.epoch();
        let publication = convergence.publish_reset(build, self.reset)?;
        if publication == TemplatePublication::Published {
            self.reset = reset;
        }
        self.advance_if_published(publication);
        Ok(publication)
    }

    fn pending_reset(self, convergence: &TemplateConvergence) -> Option<ResetTemplateBuild> {
        convergence.begin_pending_reset(self.reset)
    }

    fn is_converged(self, convergence: &TemplateConvergence) -> bool {
        convergence.is_converged(self.reset)
    }

    fn advance_if_published(&mut self, publication: TemplatePublication) {
        if publication == TemplatePublication::Published {
            self.revision = self
                .revision
                .next()
                .expect("fixture template revision has capacity");
        }
    }
}

fn output_transaction(version: u32) -> TransactionView {
    TransactionBuilder::default()
        .version(version)
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build()
}

fn child_transaction(version: u32, parent: &TransactionView) -> TransactionView {
    TransactionBuilder::default()
        .version(version)
        .input(CellInput::new(OutPoint::new(parent.hash(), 0), 0))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build()
}

fn conditional_transaction(version: u32, input: OutPoint, dependency: OutPoint) -> TransactionView {
    TransactionBuilder::default()
        .version(version)
        .input(CellInput::new(input, 0))
        .cell_dep(CellDep::new_builder().out_point(dependency).build())
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build()
}

fn set_status(authority: &mut TxPoolAuthority, hash: &RawTxHash, status: AcceptedStatus) {
    let version = owner_version(authority, hash);
    apply_plan(
        authority
            .plan_status_for_foundation(hash, version, status)
            .expect("fixture status transition plans"),
    );
}

fn causal_dag_transactions(edge_mask: u8) -> Vec<TransactionView> {
    let edges = [(0usize, 1usize), (0, 2), (1, 2)];
    let mut transactions = Vec::<TransactionView>::new();
    for child in 0..3 {
        let mut builder =
            TransactionBuilder::default().version(1_900 + u32::from(edge_mask) * 3 + child as u32);
        for (edge_index, (parent, edge_child)) in edges.into_iter().enumerate() {
            if edge_child == child && edge_mask & (1 << edge_index) != 0 {
                builder = builder.input(CellInput::new(
                    OutPoint::new(transactions[parent].hash(), child as u32),
                    0,
                ));
            }
        }
        for _ in 0..3 {
            builder = builder
                .output(CellOutput::default())
                .output_data(Bytes::new().pack());
        }
        transactions.push(builder.build());
    }
    transactions
}

#[test]
fn uak_template_causal_membership_refines_two_phase_model_exhaustively() {
    for edge_mask in 0u8..8 {
        let transactions = causal_dag_transactions(edge_mask);
        for status_encoding in 0u8..27 {
            let mut authority = TxPoolAuthority::for_foundation(limits());
            let mut digits = status_encoding;
            for (index, transaction) in transactions.iter().enumerate() {
                let status = match digits % 3 {
                    0 => AcceptedStatus::Pending,
                    1 => AcceptedStatus::Gap,
                    2 => AcceptedStatus::Proposed,
                    _ => unreachable!("base-three digit is total"),
                };
                digits /= 3;
                accept_remote_transaction_with_payload(
                    &mut authority,
                    transaction.clone(),
                    index + 1,
                    status,
                    resolved_payload_with_facts(
                        transaction,
                        Vec::new(),
                        Vec::new(),
                        Capacity::shannons(index as u64 + 1),
                    ),
                );
            }

            let selection = authority
                .read_view()
                .capture_template()
                .expect("the exhaustive fixture has one coherent authority cut")
                .into_selection()
                .expect("selection is derived outside the authority cut");
            assert_eq!(selection.candidates().len(), 3);
            let by_hash = selection
                .candidates()
                .iter()
                .enumerate()
                .map(|(index, candidate)| (candidate.hash().clone(), index))
                .collect::<HashMap<_, _>>();
            let statuses = selection
                .candidates()
                .iter()
                .map(|candidate| match candidate.status() {
                    AcceptedStatus::Pending => 0,
                    AcceptedStatus::Gap => 1,
                    AcceptedStatus::Proposed => 2,
                })
                .collect::<Vec<_>>();
            let parents = selection
                .candidates()
                .iter()
                .map(|candidate| {
                    candidate
                        .parents()
                        .iter()
                        .map(|parent| {
                            by_hash
                                .get(parent)
                                .copied()
                                .expect("every captured parent shares the receipt")
                        })
                        .collect::<BTreeSet<_>>()
                })
                .collect::<Vec<_>>();
            let model = crate::mathematical_model::two_phase::causal_membership_refinement(
                &statuses, &parents,
            )
            .expect("the production receipt is an admissible model input");
            let production = selection
                .proposed_parent_first()
                .expect("the enumerated causal graph is acyclic")
                .into_iter()
                .map(|candidate| {
                    by_hash
                        .get(candidate.hash())
                        .copied()
                        .expect("selected candidates come from the same receipt")
                })
                .collect::<BTreeSet<_>>();
            assert_eq!(
                production, model,
                "edge_mask={edge_mask} status_encoding={status_encoding}",
            );
        }
    }
}

#[test]
fn uak_apply_advances_exact_template_source_versions() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let initial = authority.template_source_versions_for_reference();

    let _queued = admit_remote(&mut authority, 1_801, 1);
    assert_eq!(
        authority.template_source_versions_for_reference(),
        initial,
        "preaccepted work is not a block-template fact"
    );

    let accepted = accept_remote_transaction(
        &mut authority,
        tx(1_802),
        2,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let pending = authority.template_source_versions_for_reference();
    assert!(pending.proposals > initial.proposals);
    assert_eq!(pending.proposals, pending.transactions);
    assert_eq!(pending.chain, initial.chain);

    set_status(&mut authority, &accepted, AcceptedStatus::Gap);
    let gap = authority.template_source_versions_for_reference();
    assert!(gap.proposals > pending.proposals);
    assert_eq!(gap.transactions, pending.transactions);

    set_status(&mut authority, &accepted, AcceptedStatus::Proposed);
    let proposed = authority.template_source_versions_for_reference();
    assert_eq!(proposed.proposals, gap.proposals);
    assert!(proposed.transactions > gap.transactions);

    set_status(&mut authority, &accepted, AcceptedStatus::Pending);
    let pending_again = authority.template_source_versions_for_reference();
    assert!(pending_again.proposals > proposed.proposals);
    assert_eq!(pending_again.proposals, pending_again.transactions);
}

#[test]
fn uak_template_read_receipt_shares_order_and_complete_resolved_payload() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let parent_tx = output_transaction(1_803);
    let parent = accept_remote_transaction_with_payload(
        &mut authority,
        parent_tx.clone(),
        3,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(&parent_tx, Vec::new(), Vec::new(), Capacity::shannons(2)),
    );
    let child_tx = child_transaction(1_804, &parent_tx);
    let child = accept_remote_transaction_with_payload(
        &mut authority,
        child_tx.clone(),
        4,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(&child_tx, Vec::new(), Vec::new(), Capacity::shannons(4)),
    );
    let gap_tx = tx(1_805);
    let gap_payload =
        resolved_payload_with_facts(&gap_tx, Vec::new(), Vec::new(), Capacity::zero());
    let gap = accept_remote_transaction_with_payload(
        &mut authority,
        gap_tx,
        5,
        AcceptedStatus::Gap,
        gap_payload,
    );

    let receipt = authority
        .read_view()
        .capture_template()
        .expect("accepted payload and source versions share one read cut");
    let candidate_uncles = CandidateUncles::new();
    assert_eq!(receipt.selected_len(), 3);
    let captured_sources = receipt.source_cut(candidate_uncle_source(&candidate_uncles));
    let receipt = receipt
        .into_selection()
        .expect("ranking runs over the owned receipt outside authority");
    assert_model_accepted_order(&receipt);
    assert_eq!(receipt.candidates().len(), 3);
    assert_eq!(
        receipt.source_cut(candidate_uncle_source(&candidate_uncles)),
        captured_sources
    );
    for candidate in receipt.candidates() {
        assert_eq!(candidate.hash().0, candidate.resolved().transaction.hash());
        assert_eq!(candidate.proposal_short_id(), &candidate.proposal().0);
        assert!(matches!(
            candidate.status(),
            AcceptedStatus::Pending | AcceptedStatus::Gap
        ));
        assert_eq!(candidate.accepted_at().0, 0);
        assert!(candidate.metrics().cost.serialized_bytes > 0);
        let _shared_order = candidate.order();
    }
    let child_candidate = receipt
        .candidates()
        .iter()
        .find(|candidate| candidate.hash() == &child)
        .expect("child is captured");
    assert_eq!(child_candidate.parents(), std::slice::from_ref(&parent));
    let Some(OwnedTx::Accepted(child_owner)) = authority.entry(&child) else {
        panic!("child remains accepted");
    };
    assert!(std::sync::Arc::ptr_eq(
        child_candidate.resolved(),
        child_owner.proof.payload().resolved_transaction()
    ));

    let proposals = receipt
        .proposals(10)
        .expect("proposal ranking allocates within the fixture");
    let proposal_set = proposals.iter().cloned().collect::<HashSet<_>>();
    assert_eq!(
        proposal_set,
        HashSet::from([
            authority
                .entry(&parent)
                .expect("parent exists")
                .record()
                .identity
                .proposal
                .clone(),
            authority
                .entry(&child)
                .expect("child exists")
                .record()
                .identity
                .proposal
                .clone(),
        ])
    );
    assert!(
        !proposal_set.contains(
            &authority
                .entry(&gap)
                .expect("Gap owner exists")
                .record()
                .identity
                .proposal
        )
    );
    for (index, proposal) in proposals.iter().enumerate() {
        let candidate = receipt
            .candidates()
            .iter()
            .find(|candidate| candidate.proposal() == proposal)
            .expect("selected proposal has one candidate");
        assert_eq!(
            receipt
                .pending_rank(candidate.hash())
                .expect("rank allocation fits"),
            index.checked_add(1)
        );
    }

    set_status(&mut authority, &parent, AcceptedStatus::Proposed);
    set_status(&mut authority, &child, AcceptedStatus::Proposed);
    let proposed = authority
        .read_view()
        .capture_template()
        .expect("proposed graph is coherent")
        .into_selection()
        .expect("proposed graph ranks outside authority");
    assert_eq!(
        proposed
            .proposed_parent_first()
            .expect("accepted causal graph is acyclic")
            .iter()
            .map(|candidate| candidate.hash())
            .collect::<Vec<_>>(),
        vec![&parent, &child]
    );
}

#[test]
fn uak_chain_commit_updates_only_affected_template_package_scores() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let parent_tx = output_transaction(1_810);
    let parent = accept_remote_transaction_with_payload(
        &mut authority,
        parent_tx.clone(),
        10,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(&parent_tx, Vec::new(), Vec::new(), Capacity::zero()),
    );
    let child_tx = child_transaction(1_811, &parent_tx);
    let child = accept_remote_transaction_with_payload(
        &mut authority,
        child_tx.clone(),
        11,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &child_tx,
            Vec::new(),
            Vec::new(),
            Capacity::shannons(100_000),
        ),
    );
    let rival_input = OutPoint::new(Byte32::new([82; 32]), 0);
    let rival_tx = TransactionBuilder::default()
        .version(1_812u32)
        .input(CellInput::new(rival_input.clone(), 0))
        .output(CellOutput::default())
        .output_data(Bytes::new().pack())
        .build();
    let rival = accept_remote_transaction_with_payload(
        &mut authority,
        rival_tx.clone(),
        12,
        AcceptedStatus::Pending,
        resolved_payload_with_facts(
            &rival_tx,
            Vec::new(),
            vec![rival_input],
            Capacity::shannons(75_000),
        ),
    );

    let before = authority
        .read_view()
        .capture_template()
        .expect("the initial package order is coherent")
        .into_selection()
        .expect("the initial package order is already derived");
    assert_model_accepted_order(&before);
    let child_before = before
        .candidates()
        .iter()
        .find(|candidate| candidate.hash() == &child)
        .expect("child is captured");
    let rival_before = before
        .candidates()
        .iter()
        .find(|candidate| candidate.hash() == &rival)
        .expect("rival is captured");
    assert!(rival_before.order() > child_before.order());

    let facts = ChainTransitionFacts::for_foundation(
        ChainViewId::new(ChainRevision(1), Byte32::new([83; 32])),
        ChainBlockChanges::for_foundation(vec![parent_tx], Vec::new(), Vec::new(), Vec::new()),
        Vec::new(),
    )
    .expect("the attached parent is one canonical chain fact");
    let receipt = authority
        .chain_validation_work(facts)
        .expect("the committed parent is selected")
        .validate_for_foundation(Vec::new())
        .expect("the chain cut needs no proposal positions");
    apply_plan(
        authority
            .plan_chain_transition(receipt)
            .expect("owner removal and package-key replacement are atomic"),
    );
    assert!(authority.entry(&parent).is_none());
    assert_membership_reference(&authority);

    let after = authority
        .read_view()
        .capture_template()
        .expect("the post-commit package order is coherent")
        .into_selection()
        .expect("the post-commit package order is already derived");
    assert_model_accepted_order(&after);
    let child_after = after
        .candidates()
        .iter()
        .find(|candidate| candidate.hash() == &child)
        .expect("surviving child is captured");
    let rival_after = after
        .candidates()
        .iter()
        .find(|candidate| candidate.hash() == &rival)
        .expect("unrelated rival is captured");
    assert!(child_after.order() > rival_after.order());
}

#[test]
fn uak_template_orders_selected_dependency_reader_before_spender() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let shared = OutPoint::new(Byte32::new([84; 32]), 0);
    let reader_input = OutPoint::new(Byte32::new([85; 32]), 0);
    let reader_tx = conditional_transaction(1_813, reader_input.clone(), shared.clone());
    let reader = accept_remote_transaction_with_payload(
        &mut authority,
        reader_tx.clone(),
        13,
        AcceptedStatus::Proposed,
        resolved_payload_with_facts(
            &reader_tx,
            Vec::new(),
            vec![reader_input],
            Capacity::shannons(1),
        ),
    );
    let spender_tx = TransactionBuilder::default()
        .version(1_814u32)
        .input(CellInput::new(shared.clone(), 0))
        .build();
    let spender = accept_remote_transaction_with_payload(
        &mut authority,
        spender_tx.clone(),
        14,
        AcceptedStatus::Proposed,
        resolved_payload_with_facts(
            &spender_tx,
            Vec::new(),
            vec![shared],
            Capacity::shannons(100_000),
        ),
    );

    let selection = authority
        .read_view()
        .capture_template()
        .expect("the conditional pair shares one authority cut")
        .into_selection()
        .expect("the immutable selection receipt is coherent");
    let captured = selection
        .candidates()
        .iter()
        .map(|candidate| candidate.hash())
        .collect::<Vec<_>>();
    assert!(
        captured.iter().position(|hash| *hash == &spender)
            < captured.iter().position(|hash| *hash == &reader),
        "the fee order deliberately prefers the spender before conditional ordering"
    );
    assert_eq!(
        selection
            .proposed_parent_first()
            .expect("the selected-set conditional graph is acyclic")
            .into_iter()
            .map(|candidate| candidate.hash())
            .collect::<Vec<_>>(),
        vec![&reader, &spender]
    );
}

#[test]
fn uak_template_sheds_conditional_cycles_deterministically() {
    use crate::mathematical_model::two_phase::ConditionalSelectionInput;

    let mut authority = TxPoolAuthority::for_foundation(limits());
    let left_cell = OutPoint::new(Byte32::new([86; 32]), 0);
    let right_cell = OutPoint::new(Byte32::new([87; 32]), 0);
    let weaker_tx = conditional_transaction(1_815, right_cell.clone(), left_cell.clone());
    let weaker = accept_remote_transaction_with_payload(
        &mut authority,
        weaker_tx.clone(),
        15,
        AcceptedStatus::Proposed,
        resolved_payload_with_facts(
            &weaker_tx,
            Vec::new(),
            vec![right_cell.clone()],
            Capacity::shannons(1),
        ),
    );
    let stronger_tx = conditional_transaction(1_816, left_cell, right_cell);
    let stronger = accept_remote_transaction_with_payload(
        &mut authority,
        stronger_tx.clone(),
        16,
        AcceptedStatus::Proposed,
        resolved_payload_with_facts(
            &stronger_tx,
            Vec::new(),
            vec![OutPoint::new(Byte32::new([86; 32]), 0)],
            Capacity::shannons(100_000),
        ),
    );
    let weaker_child_tx = child_transaction(1_817, &weaker_tx);
    let weaker_child = accept_remote_transaction_with_payload(
        &mut authority,
        weaker_child_tx.clone(),
        17,
        AcceptedStatus::Proposed,
        resolved_payload_with_facts(
            &weaker_child_tx,
            Vec::new(),
            Vec::new(),
            Capacity::shannons(1),
        ),
    );
    assert_membership_reference(&authority);

    let selection = authority
        .read_view()
        .capture_template()
        .expect("the conditional cycle shares one authority cut")
        .into_selection()
        .expect("the immutable selection receipt is coherent");
    let selected = selection
        .proposed_parent_first()
        .expect("conditional cycles are a bounded packing condition")
        .into_iter()
        .map(|candidate| candidate.hash().clone())
        .collect::<Vec<_>>();
    assert_eq!(selected, vec![stronger.clone()]);
    assert!(!selected.contains(&weaker));
    assert!(
        !selected.contains(&weaker_child),
        "a causal descendant cannot survive a shed producer"
    );
    let (model_selected, model_shed) = model_conditional_selection(
        vec![
            ConditionalSelectionInput::new(BTreeSet::new(), BTreeSet::from([1]), 0, 0),
            ConditionalSelectionInput::new(BTreeSet::new(), BTreeSet::from([0]), 1, 2),
            ConditionalSelectionInput::new(BTreeSet::from([0]), BTreeSet::new(), 2, 1),
        ],
        &[weaker.clone(), stronger, weaker_child.clone()],
    );
    assert_eq!(selected, model_selected);
    assert_eq!(model_shed, BTreeSet::from([weaker, weaker_child]));
    let serialized_bytes = selection
        .candidates()
        .iter()
        .map(|candidate| candidate.metrics().cost.serialized_bytes)
        .sum();
    let cycles = selection
        .candidates()
        .iter()
        .map(|candidate| candidate.metrics().cost.cycles)
        .sum();
    let service_premise = exact_template_service_premise(
        &selection,
        selection.candidates().len(),
        serialized_bytes,
        cycles,
    );
    let model_retained = service_premise
        .retained_source_indices()
        .iter()
        .map(|index| selection.candidates()[*index].hash().clone())
        .collect::<Vec<_>>();
    let model_current_retained = service_premise
        .current_retained_source_indices()
        .expect("the sealed Proposed statuses compile from the same source cut")
        .into_iter()
        .map(|index| selection.candidates()[index].hash().clone())
        .collect::<Vec<_>>();
    let model_composition_shed = service_premise
        .shed_source_indices()
        .iter()
        .map(|index| selection.candidates()[*index].hash().clone())
        .collect::<BTreeSet<_>>();
    let packed = selection
        .pack_transactions(TemplatePackingLimits::new(serialized_bytes, cycles))
        .expect("the production packer shares the bounded cycle kernel")
        .entries()
        .iter()
        .map(|entry| entry.hash())
        .collect::<Vec<_>>();
    assert_eq!(packed, selected);
    assert_eq!(model_retained, packed);
    assert_eq!(model_current_retained, packed);
    assert_eq!(model_composition_shed, model_shed);
}

#[test]
fn uak_template_service_premise_separates_pending_proposals_from_current_pack() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let earlier = accept_remote_transaction(
        &mut authority,
        tx(1_818),
        18,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let later = accept_remote_transaction(
        &mut authority,
        tx(1_819),
        19,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let selection = authority
        .read_view()
        .capture_template()
        .expect("the Pending candidate shares one authority cut")
        .into_selection()
        .expect("the immutable selection receipt is coherent");
    let serialized_bytes = selection
        .candidates()
        .iter()
        .map(|candidate| candidate.metrics().cost.serialized_bytes)
        .sum();
    let cycles = selection
        .candidates()
        .iter()
        .map(|candidate| candidate.metrics().cost.cycles)
        .sum();
    let premise = exact_template_service_premise(&selection, 1, serialized_bytes, cycles);

    let production_proposals = selection.proposal_short_ids(1).unwrap();
    let model_proposals = premise
        .current_proposal_source_indices()
        .expect("the sealed Pending status compiles from the same source cut")
        .into_iter()
        .map(|index| selection.candidates()[index].proposal_short_id().clone())
        .collect::<Vec<_>>();
    assert_eq!(production_proposals, model_proposals);
    assert_eq!(production_proposals.len(), 1);
    assert_eq!(
        selection
            .pack_transactions(TemplatePackingLimits::new(serialized_bytes, cycles))
            .expect("the bounded current pack is valid")
            .entries()
            .len(),
        0,
        "Pending work belongs to the proposal prefix, not the current transaction pack"
    );
    assert!(
        premise
            .current_retained_source_indices()
            .expect("the sealed Pending status compiles from the same source cut")
            .is_empty(),
        "the model current compilation must not treat Pending as Proposed"
    );
    assert_eq!(
        selection
            .candidates()
            .iter()
            .map(|candidate| candidate.hash().clone())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([earlier, later])
    );
}

#[test]
fn uak_template_cycle_shedding_preserves_descendant_aware_strength() {
    use crate::mathematical_model::two_phase::ConditionalSelectionInput;

    let mut authority = TxPoolAuthority::for_foundation(limits());
    let left_cell = OutPoint::new(Byte32::new([90; 32]), 0);
    let right_cell = OutPoint::new(Byte32::new([91; 32]), 0);
    let package_parent_tx = conditional_transaction(1_821, right_cell.clone(), left_cell.clone());
    let package_parent = accept_remote_transaction_with_payload(
        &mut authority,
        package_parent_tx.clone(),
        21,
        AcceptedStatus::Proposed,
        resolved_payload_with_facts(
            &package_parent_tx,
            Vec::new(),
            vec![right_cell.clone()],
            Capacity::shannons(1),
        ),
    );
    let standalone_tx = conditional_transaction(1_822, left_cell, right_cell);
    let standalone = accept_remote_transaction_with_payload(
        &mut authority,
        standalone_tx.clone(),
        22,
        AcceptedStatus::Proposed,
        resolved_payload_with_facts(
            &standalone_tx,
            Vec::new(),
            vec![OutPoint::new(Byte32::new([90; 32]), 0)],
            Capacity::shannons(1_000),
        ),
    );
    let package_child_tx = child_transaction(1_823, &package_parent_tx);
    let package_child = accept_remote_transaction_with_payload(
        &mut authority,
        package_child_tx.clone(),
        23,
        AcceptedStatus::Proposed,
        resolved_payload_with_facts(
            &package_child_tx,
            Vec::new(),
            Vec::new(),
            Capacity::shannons(10_000_000),
        ),
    );
    assert_membership_reference(&authority);

    let selected = authority
        .read_view()
        .capture_template()
        .expect("the package-sensitive cycle shares one authority cut")
        .into_selection()
        .expect("the immutable selection receipt is coherent")
        .proposed_parent_first()
        .expect("the frozen descendant-aware fallback breaks the cycle")
        .into_iter()
        .map(|candidate| candidate.hash().clone())
        .collect::<Vec<_>>();
    assert_eq!(
        selected,
        vec![package_parent.clone(), package_child.clone()]
    );
    assert!(!selected.contains(&standalone));
    let (model_selected, model_shed) = model_conditional_selection(
        vec![
            ConditionalSelectionInput::new(BTreeSet::new(), BTreeSet::from([1]), 0, 2),
            ConditionalSelectionInput::new(BTreeSet::new(), BTreeSet::from([0]), 1, 0),
            ConditionalSelectionInput::new(BTreeSet::from([0]), BTreeSet::new(), 2, 1),
        ],
        &[package_parent, standalone.clone(), package_child],
    );
    assert_eq!(selected, model_selected);
    assert_eq!(model_shed, BTreeSet::from([standalone]));
}

#[test]
fn uak_template_complete_dependency_scan_preserves_causal_and_later_independent_work() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let shared = OutPoint::new(Byte32::new([88; 32]), 0);
    let input = OutPoint::new(Byte32::new([89; 32]), 0);
    let over_budget_tx = conditional_transaction(1_818, input.clone(), shared);
    let over_budget = accept_remote_transaction_with_payload(
        &mut authority,
        over_budget_tx.clone(),
        18,
        AcceptedStatus::Proposed,
        resolved_payload_with_facts(
            &over_budget_tx,
            Vec::new(),
            vec![input],
            Capacity::shannons(200_000),
        ),
    );
    let over_budget_child_tx = child_transaction(1_819, &over_budget_tx);
    let over_budget_child = accept_remote_transaction_with_payload(
        &mut authority,
        over_budget_child_tx.clone(),
        19,
        AcceptedStatus::Proposed,
        resolved_payload_with_facts(
            &over_budget_child_tx,
            Vec::new(),
            Vec::new(),
            Capacity::shannons(300_000),
        ),
    );
    let independent_tx = tx(1_820);
    let independent = accept_remote_transaction_with_payload(
        &mut authority,
        independent_tx.clone(),
        20,
        AcceptedStatus::Proposed,
        resolved_payload_with_facts(
            &independent_tx,
            Vec::new(),
            Vec::new(),
            Capacity::shannons(1),
        ),
    );

    let receipt = authority
        .read_view()
        .capture_template()
        .expect("the bounded selection shares one authority cut")
        .into_selection()
        .expect("the immutable selection receipt is coherent");
    let selected = receipt
        .proposed_parent_first_for_foundation()
        .expect("the complete dependency scan is bounded by the captured footprint")
        .into_iter()
        .map(|candidate| candidate.hash().clone())
        .collect::<Vec<_>>();
    let identities = receipt
        .candidates()
        .iter()
        .map(|candidate| candidate.hash().clone())
        .collect::<Vec<_>>();
    let by_hash = identities
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, hash)| (hash, index))
        .collect::<HashMap<_, _>>();
    let inputs = receipt
        .candidates()
        .iter()
        .map(|candidate| {
            crate::mathematical_model::two_phase::DependencyScanInput::new(
                candidate
                    .parents()
                    .iter()
                    .map(|parent| by_hash[parent])
                    .collect(),
                candidate.resolved().related_dep_out_points().count(),
            )
        })
        .collect::<Vec<_>>();
    let causally_selected = receipt
        .causally_eligible_proposed(
            &receipt
                .candidate_index()
                .expect("the immutable candidate identities are unique"),
        )
        .expect("the production causal selection is coherent");
    let captured_edge_bound = inputs
        .iter()
        .map(|input| input.dependency_edges_for_foundation())
        .sum();
    let model = crate::mathematical_model::two_phase::complete_dependency_scan_refinement(
        &inputs,
        &causally_selected,
        captured_edge_bound,
    )
    .expect("the exact captured dependency domain has one bounded model observation");
    assert_eq!(
        selected,
        model
            .retained
            .into_iter()
            .map(|index| identities[index].clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(selected, vec![over_budget, over_budget_child, independent]);
}

fn independent_dependency_scan_selection(fees: [u64; 2]) -> (Vec<RawTxHash>, [RawTxHash; 2]) {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let mut hashes = Vec::with_capacity(2);
    for (index, fee) in fees.into_iter().enumerate() {
        let tag = u8::try_from(index).expect("the two-candidate index fits u8");
        let input = OutPoint::new(Byte32::new([90 + tag; 32]), 0);
        let dependency = OutPoint::new(Byte32::new([100 + tag; 32]), 0);
        let transaction = conditional_transaction(1_821 + index as u32, input.clone(), dependency);
        hashes.push(accept_remote_transaction_with_payload(
            &mut authority,
            transaction.clone(),
            21 + index,
            AcceptedStatus::Proposed,
            resolved_payload_with_facts(
                &transaction,
                Vec::new(),
                vec![input],
                Capacity::shannons(fee),
            ),
        ));
    }

    let receipt = authority
        .read_view()
        .capture_template()
        .expect("the bounded selection shares one authority cut")
        .into_selection()
        .expect("the immutable selection receipt is coherent");
    let selected = receipt
        .proposed_parent_first_for_foundation()
        .expect("the complete scan preserves every independent selected candidate")
        .into_iter()
        .map(|candidate| candidate.hash().clone())
        .collect::<Vec<_>>();

    let identities = receipt
        .candidates()
        .iter()
        .map(|candidate| candidate.hash().clone())
        .collect::<Vec<_>>();
    let inputs = receipt
        .candidates()
        .iter()
        .map(|candidate| {
            crate::mathematical_model::two_phase::DependencyScanInput::new(
                BTreeSet::new(),
                candidate.resolved().related_dep_out_points().count(),
            )
        })
        .collect::<Vec<_>>();
    let causally_selected = receipt
        .causally_eligible_proposed(
            &receipt
                .candidate_index()
                .expect("the immutable candidate identities are unique"),
        )
        .expect("the production causal selection is coherent");
    let captured_edge_bound = inputs
        .iter()
        .map(|input| input.dependency_edges_for_foundation())
        .sum();
    let model = crate::mathematical_model::two_phase::complete_dependency_scan_refinement(
        &inputs,
        &causally_selected,
        captured_edge_bound,
    )
    .expect("the exact dependency count bounds every retained candidate");
    assert_eq!(
        selected,
        model
            .retained
            .into_iter()
            .map(|index| identities[index].clone())
            .collect::<Vec<_>>()
    );

    (
        selected,
        hashes.try_into().expect("the fixture has two hashes"),
    )
}

#[test]
fn uak_template_complete_dependency_scan_does_not_censor_independent_dependency_work() {
    let (first_selected, first_hashes) = independent_dependency_scan_selection([200_000, 100_000]);
    assert_eq!(
        first_selected,
        vec![first_hashes[0].clone(), first_hashes[1].clone()]
    );

    let (second_selected, second_hashes) =
        independent_dependency_scan_selection([100_000, 200_000]);
    assert_eq!(
        second_selected,
        vec![second_hashes[1].clone(), second_hashes[0].clone()]
    );
}

#[test]
fn uak_template_receipts_repair_overwrite_and_delayed_delta() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let mut candidate_uncles = CandidateUncles::new();
    let empty = source_cut(&authority, &candidate_uncles);
    let mut convergence = TemplateConvergence::for_foundation(empty);
    let mut projection = TemplateProjection::initial();
    let old_full = convergence.begin_full(empty);

    let accepted = accept_remote_transaction(
        &mut authority,
        tx(1_806),
        6,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let pending = source_cut(&authority, &candidate_uncles);
    let newer_partial =
        projection.begin_partial(&mut convergence, TemplateComponent::Proposals, pending);
    assert_eq!(
        projection
            .publish_partial(&mut convergence, newer_partial)
            .expect("fixture revision has capacity"),
        TemplatePublication::Published
    );
    assert_eq!(
        projection
            .publish_full(&mut convergence, old_full)
            .expect("fixture revision has capacity"),
        TemplatePublication::Published,
        "full wins publication even after a newer partial"
    );
    assert!(convergence.is_pending(TemplateComponent::Proposals));
    assert!(convergence.is_pending(TemplateComponent::Transactions));
    assert!(convergence.is_pending(TemplateComponent::Uncles));

    let racing_proposals =
        projection.begin_partial(&mut convergence, TemplateComponent::Proposals, pending);
    let racing_transactions =
        projection.begin_partial(&mut convergence, TemplateComponent::Transactions, pending);
    assert_eq!(
        projection
            .publish_partial(&mut convergence, racing_proposals)
            .expect("fixture revision has capacity"),
        TemplatePublication::Published
    );
    assert_eq!(
        projection
            .publish_partial(&mut convergence, racing_transactions)
            .expect("fixture revision has capacity"),
        TemplatePublication::Stale,
        "partial publications use an exact shared revision"
    );
    assert!(!convergence.is_pending(TemplateComponent::Proposals));
    assert!(convergence.is_pending(TemplateComponent::Transactions));

    for component in [TemplateComponent::Uncles, TemplateComponent::Transactions] {
        let build = projection.begin_partial(&mut convergence, component, pending);
        assert_eq!(
            projection
                .publish_partial(&mut convergence, build)
                .expect("fixture revision has capacity"),
            TemplatePublication::Published
        );
    }
    assert!(projection.is_converged(&convergence));
    convergence.observe_sources(empty);
    assert!(
        projection.is_converged(&convergence),
        "a delayed old source receipt cannot regress desired coverage"
    );

    // No notification is modeled here. The next level read still discovers
    // the exact Pending->Gap source changes and makes work visible.
    set_status(&mut authority, &accepted, AcceptedStatus::Gap);
    let gap = source_cut(&authority, &candidate_uncles);
    convergence.observe_sources(gap);
    assert!(convergence.is_pending(TemplateComponent::Proposals));
    assert!(!convergence.is_pending(TemplateComponent::Transactions));
    assert!(convergence.is_pending(TemplateComponent::Uncles));
    for component in [
        TemplateComponent::Proposals,
        TemplateComponent::Uncles,
        TemplateComponent::Transactions,
    ] {
        let build = projection.begin_partial(&mut convergence, component, gap);
        assert_eq!(
            projection
                .publish_partial(&mut convergence, build)
                .expect("fixture revision has capacity"),
            TemplatePublication::Published
        );
    }
    assert!(projection.is_converged(&convergence));

    assert!(
        candidate_uncles
            .try_insert(BlockBuilder::default().build().as_uncle())
            .expect("fixture candidate source version has capacity")
    );
    let uncle_changed = source_cut(&authority, &candidate_uncles);
    convergence.observe_sources(uncle_changed);
    assert!(!convergence.is_pending(TemplateComponent::Proposals));
    assert!(!convergence.is_pending(TemplateComponent::Transactions));
    assert!(convergence.is_pending(TemplateComponent::Uncles));
    let uncle_build =
        projection.begin_partial(&mut convergence, TemplateComponent::Uncles, uncle_changed);
    assert_eq!(
        projection
            .publish_partial(&mut convergence, uncle_build)
            .expect("fixture revision has capacity"),
        TemplatePublication::Published
    );

    let stale_full = convergence.begin_full(uncle_changed);
    let first_reset = convergence
        .mark_reset(uncle_changed)
        .expect("fixture reset epoch has capacity");
    assert_eq!(
        projection
            .publish_reset(&mut convergence, first_reset)
            .expect("fixture revision has capacity"),
        TemplatePublication::Published
    );
    assert_eq!(
        projection
            .publish_full(&mut convergence, stale_full)
            .expect("fixture revision has capacity"),
        TemplatePublication::Stale,
        "a full build cannot cross a published reset"
    );
    assert!(!projection.is_converged(&convergence));

    let superseded_reset = convergence
        .mark_reset(uncle_changed)
        .expect("fixture reset epoch has capacity");
    let latest_reset = convergence
        .mark_reset(uncle_changed)
        .expect("fixture reset epoch has capacity");
    assert_eq!(
        projection
            .publish_reset(&mut convergence, superseded_reset)
            .expect("fixture revision has capacity"),
        TemplatePublication::Stale
    );
    assert_eq!(
        projection
            .publish_reset(&mut convergence, latest_reset)
            .expect("fixture revision has capacity"),
        TemplatePublication::Published
    );
    let final_full = convergence.begin_full(uncle_changed);
    assert_eq!(
        projection
            .publish_full(&mut convergence, final_full)
            .expect("fixture revision has capacity"),
        TemplatePublication::Published
    );
    assert!(projection.is_converged(&convergence));
}

#[test]
fn uak_template_chain_read_requires_one_coherent_component_cut() {
    let authority = TxPoolAuthority::for_foundation(limits());
    let candidate_uncles = CandidateUncles::new();
    let sources = source_cut(&authority, &candidate_uncles);
    let required = sources.chain_source();
    let mut convergence = TemplateConvergence::for_foundation(sources);
    let mut projection = TemplateProjection::initial();

    assert_eq!(
        convergence.chain_read_state(required),
        TemplateChainReadState::Published,
        "the initial base template has one coherent source cut"
    );
    for component in [
        TemplateComponent::Proposals,
        TemplateComponent::Transactions,
        TemplateComponent::Uncles,
    ] {
        assert!(
            convergence.is_pending(component),
            "chain-only initial coverage cannot impersonate an exact component receipt"
        );
    }

    let reset = convergence
        .mark_reset(sources)
        .expect("fixture reset epoch has capacity");
    assert_eq!(
        projection
            .publish_reset(&mut convergence, reset)
            .expect("fixture revision has capacity"),
        TemplatePublication::Published
    );
    assert_eq!(
        convergence.chain_read_state(required),
        TemplateChainReadState::Pending,
        "a base reset does not fabricate proposal/transaction/uncle receipts"
    );

    convergence.record_replacement_failure(required);
    assert_eq!(
        convergence.chain_read_state(required),
        TemplateChainReadState::Failed
    );
    let retry_reset = convergence
        .mark_reset(sources)
        .expect("fixture reset epoch has capacity");
    assert_eq!(
        projection
            .publish_reset(&mut convergence, retry_reset)
            .expect("fixture revision has capacity"),
        TemplatePublication::Published
    );
    assert_eq!(
        convergence.chain_read_state(required),
        TemplateChainReadState::Pending,
        "successful replacement progress retires failure but is not readiness"
    );

    for component in [
        TemplateComponent::Proposals,
        TemplateComponent::Uncles,
        TemplateComponent::Transactions,
    ] {
        let build = projection.begin_partial(&mut convergence, component, sources);
        assert_eq!(
            projection
                .publish_partial(&mut convergence, build)
                .expect("fixture revision has capacity"),
            TemplatePublication::Published
        );
        let expected = if component == TemplateComponent::Transactions {
            TemplateChainReadState::Published
        } else {
            TemplateChainReadState::Pending
        };
        assert_eq!(convergence.chain_read_state(required), expected);
    }
}

#[test]
fn uak_template_reset_counter_exhaustion_is_typed_and_mutation_free() {
    let authority = TxPoolAuthority::for_foundation(limits());
    let candidate_uncles = CandidateUncles::new();
    let sources = source_cut(&authority, &candidate_uncles);

    let mut reset = TemplateConvergence::for_foundation(sources);
    reset.force_reset_epoch_exhaustion_for_foundation();
    let before = reset.clone();
    assert!(matches!(
        reset.mark_reset(sources),
        Err(TemplateConvergenceError::ResetEpochExhausted)
    ));
    assert_eq!(reset, before);
}

#[test]
fn uak_dropped_reset_build_remains_level_triggered_until_publication() {
    let authority = TxPoolAuthority::for_foundation(limits());
    let candidate_uncles = CandidateUncles::new();
    let sources = source_cut(&authority, &candidate_uncles);
    let mut convergence = TemplateConvergence::for_foundation(sources);
    let mut projection = TemplateProjection::initial();
    let initial = convergence.begin_full(sources);
    assert_eq!(
        projection
            .publish_full(&mut convergence, initial)
            .expect("fixture revision has capacity"),
        TemplatePublication::Published
    );
    assert!(projection.is_converged(&convergence));

    {
        let _dropped = convergence
            .mark_reset(sources)
            .expect("fixture reset epoch has capacity");
    }
    assert!(
        !projection.is_converged(&convergence),
        "an unconsumed reset level cannot look quiescent"
    );
    let retry = projection
        .pending_reset(&convergence)
        .expect("the exact pending reset capability is reconstructible");
    assert_eq!(
        projection
            .publish_reset(&mut convergence, retry)
            .expect("fixture revision has capacity"),
        TemplatePublication::Published
    );
    assert!(projection.pending_reset(&convergence).is_none());
    assert!(
        !projection.is_converged(&convergence),
        "blank reset content still needs a full component rebuild"
    );
    let rebuilt = convergence.begin_full(sources);
    assert_eq!(
        projection
            .publish_full(&mut convergence, rebuilt)
            .expect("fixture revision has capacity"),
        TemplatePublication::Published
    );
    assert!(projection.is_converged(&convergence));
}

#[test]
fn uak_requested_reset_fences_an_older_full_before_reset_publication() {
    let authority = TxPoolAuthority::for_foundation(limits());
    let candidate_uncles = CandidateUncles::new();
    let sources = source_cut(&authority, &candidate_uncles);
    let mut convergence = TemplateConvergence::for_foundation(sources);
    let mut projection = TemplateProjection::initial();
    let initial = convergence.begin_full(sources);
    assert_eq!(
        projection
            .publish_full(&mut convergence, initial)
            .expect("fixture revision has capacity"),
        TemplatePublication::Published
    );

    let old_full = convergence.begin_full(sources);
    let reset = convergence
        .mark_reset(sources)
        .expect("fixture reset epoch has capacity");
    assert_eq!(
        projection
            .publish_full(&mut convergence, old_full)
            .expect("fixture revision has capacity"),
        TemplatePublication::Stale,
        "the reset request, not scheduler timing, fences an older full build"
    );

    let post_request_full = convergence.begin_full(sources);
    assert_eq!(
        projection
            .publish_reset(&mut convergence, reset)
            .expect("fixture revision has capacity"),
        TemplatePublication::Published
    );
    assert_eq!(
        projection
            .publish_full(&mut convergence, post_request_full)
            .expect("fixture revision has capacity"),
        TemplatePublication::Published,
        "a full built for the exact reset epoch may publish after that reset"
    );
    assert!(projection.is_converged(&convergence));
}

#[test]
fn uak_recovered_tree_has_normal_template_proposal_path() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let parent_tx = output_transaction(1_807);
    let parent = accept_remote_transaction(
        &mut authority,
        parent_tx.clone(),
        7,
        AcceptedStatus::Gap,
        Vec::new(),
    );
    let child_tx = child_transaction(1_808, &parent_tx);
    let child = accept_remote_transaction(
        &mut authority,
        child_tx.clone(),
        8,
        AcceptedStatus::Gap,
        Vec::new(),
    );
    let grandchild_tx = child_transaction(1_809, &child_tx);
    let grandchild = accept_remote_transaction(
        &mut authority,
        grandchild_tx.clone(),
        9,
        AcceptedStatus::Gap,
        Vec::new(),
    );
    let proposals = [
        ProposalId(parent_tx.proposal_short_id()),
        ProposalId(child_tx.proposal_short_id()),
        ProposalId(grandchild_tx.proposal_short_id()),
    ];
    let facts = ChainTransitionFacts::for_foundation(
        ChainViewId::new(ChainRevision(1), Byte32::new([81; 32])),
        ChainBlockChanges::for_foundation(Vec::new(), Vec::new(), Vec::new(), Vec::new()),
        proposals.to_vec(),
    )
    .expect("reorg proposal facts are canonical");
    let receipt = authority
        .chain_validation_work(facts)
        .expect("the Gap tree is selected by the proposal index")
        .validate_for_foundation(
            proposals
                .iter()
                .cloned()
                .map(|proposal| (proposal, ProposalWindowPosition::Outside))
                .collect(),
        )
        .expect("every changed proposal has one new-window position");
    apply_plan(
        authority
            .plan_chain_transition(receipt)
            .expect("Gap demotion and chain cut apply atomically"),
    );

    let template = authority
        .read_view()
        .capture_template()
        .expect("the demoted tree has one template receipt")
        .into_selection()
        .expect("the demoted tree ranks outside authority");
    let selected = template
        .proposals(10)
        .expect("proposal selection fits the fixture")
        .into_iter()
        .collect::<HashSet<_>>();
    assert_eq!(selected, proposals.into_iter().collect());

    for hash in [&parent, &child, &grandchild] {
        set_status(&mut authority, hash, AcceptedStatus::Proposed);
    }
    let committed = authority
        .read_view()
        .capture_template()
        .expect("the proposed tree has one template receipt")
        .into_selection()
        .expect("the proposed tree ranks outside authority")
        .proposed_parent_first()
        .expect("the recovered causal tree is acyclic")
        .into_iter()
        .map(|candidate| candidate.hash().clone())
        .collect::<Vec<_>>();
    assert_eq!(committed, vec![parent, child, grandchild]);
}
