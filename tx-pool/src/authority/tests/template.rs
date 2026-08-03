use super::super::{
    chain::{ChainBlockChanges, ChainPackagingMode, ChainTransitionFacts, ProposalWindowPosition},
    packing::TemplatePackingLimits,
    plan::TxPoolAuthority,
    state::{AcceptedStatus, ChainRevision, ChainViewId, OwnedTx, ProposalId, RawTxHash},
    template::{
        TemplateComponent, TemplateConvergence, TemplateConvergenceError, TemplatePublication,
        TemplateSourceCut,
    },
};
use super::foundation::{
    accept_remote_transaction, accept_remote_transaction_with_payload, admit_remote,
    apply_without_work, assert_membership_reference, genesis_snapshot, limits, owner_version,
    resolved_payload_with_facts, tx,
};
use crate::block_assembler::{CandidateUncleSourceReceipt, CandidateUncles};
use ckb_types::{
    bytes::Bytes,
    core::{BlockBuilder, Capacity, TransactionBuilder, TransactionView},
    packed::{Byte32, CellDep, CellInput, CellOutput, OutPoint},
    prelude::{Builder, Entity, Pack},
};
use std::collections::HashSet;

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
    uncles.prepare_uncles(&snapshot, &epoch).into_parts().2
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
    apply_without_work(
        authority
            .plan_status_for_foundation(hash, version, status)
            .expect("fixture status transition plans"),
    );
}

#[test]
fn uak_apply_advances_exact_template_source_versions() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let initial_occ = authority.source_versions_for_reference();
    let initial = authority.template_source_versions_for_reference();

    let _queued = admit_remote(&mut authority, 1_801, 1);
    assert_eq!(authority.source_versions_for_reference(), initial_occ);
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
    let pending_occ = authority.source_versions_for_reference();
    let pending = authority.template_source_versions_for_reference();
    assert!(pending_occ.0 > initial_occ.0);
    assert_eq!(pending_occ.0, pending_occ.1);
    assert_eq!(pending_occ.0, pending.proposals);
    assert_eq!(pending_occ.0, pending.transactions);
    assert_eq!(pending.chain, initial.chain);

    set_status(&mut authority, &accepted, AcceptedStatus::Gap);
    let gap_occ = authority.source_versions_for_reference();
    let gap = authority.template_source_versions_for_reference();
    assert_eq!(gap_occ.0, pending_occ.0);
    assert!(gap_occ.1 > pending_occ.1);
    assert_eq!(gap.proposals, gap_occ.1);
    assert_eq!(gap.transactions, pending.transactions);

    set_status(&mut authority, &accepted, AcceptedStatus::Proposed);
    let proposed_occ = authority.source_versions_for_reference();
    let proposed = authority.template_source_versions_for_reference();
    assert_eq!(proposed_occ.0, pending_occ.0);
    assert!(proposed_occ.1 > gap_occ.1);
    assert_eq!(proposed.proposals, gap.proposals);
    assert_eq!(proposed.transactions, proposed_occ.1);

    set_status(&mut authority, &accepted, AcceptedStatus::Pending);
    let pending_again_occ = authority.source_versions_for_reference();
    let pending_again = authority.template_source_versions_for_reference();
    assert_eq!(pending_again_occ.0, pending_occ.0);
    assert_eq!(pending_again.proposals, pending_again_occ.1);
    assert_eq!(pending_again.transactions, pending_again_occ.1);
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
    let captured_cut = receipt.cut().next_apply_sequence();
    let captured_sources = receipt.source_cut(candidate_uncle_source(&candidate_uncles));
    let receipt = receipt
        .into_selection()
        .expect("ranking runs over the owned receipt outside authority");
    assert_eq!(receipt.candidates().len(), 3);
    assert_eq!(receipt.cut().next_apply_sequence(), captured_cut);
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
        Vec::new(),
        ChainPackagingMode::ObserveOnly,
    )
    .expect("the attached parent is one canonical chain fact");
    let receipt = authority
        .chain_validation_work(facts)
        .expect("the committed parent is selected")
        .validate_for_foundation(Vec::new())
        .expect("the chain cut needs no proposal positions");
    apply_without_work(
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
    assert_eq!(selected, vec![stronger]);
    assert!(!selected.contains(&weaker));
    assert!(
        !selected.contains(&weaker_child),
        "a causal descendant cannot survive a shed producer"
    );
    let packed = selection
        .pack_transactions(TemplatePackingLimits::new(usize::MAX, u64::MAX))
        .expect("the production packer shares the bounded cycle kernel")
        .entries()
        .iter()
        .map(|entry| entry.hash().clone())
        .collect::<Vec<_>>();
    assert_eq!(packed, selected);
}

#[test]
fn uak_template_cycle_shedding_preserves_descendant_aware_strength() {
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
    assert_eq!(selected, vec![package_parent, package_child]);
    assert!(!selected.contains(&standalone));
}

#[test]
fn uak_template_dependency_budget_cannot_censor_later_independent_work() {
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

    let selected = authority
        .read_view()
        .capture_template()
        .expect("the bounded selection shares one authority cut")
        .into_selection()
        .expect("the immutable selection receipt is coherent")
        .proposed_parent_first_for_foundation(0)
        .expect("the zero dependency budget is a deterministic selection bound")
        .into_iter()
        .map(|candidate| candidate.hash().clone())
        .collect::<Vec<_>>();
    assert_eq!(selected, vec![independent]);
    assert!(!selected.contains(&over_budget));
    assert!(!selected.contains(&over_budget_child));
}

#[test]
fn uak_template_receipts_repair_overwrite_and_delayed_delta() {
    let mut authority = TxPoolAuthority::for_foundation(limits());
    let mut candidate_uncles = CandidateUncles::new();
    let empty = source_cut(&authority, &candidate_uncles);
    let mut convergence = TemplateConvergence::new(empty);
    let old_full = convergence.begin_full(empty);

    let accepted = accept_remote_transaction(
        &mut authority,
        tx(1_806),
        6,
        AcceptedStatus::Pending,
        Vec::new(),
    );
    let pending = source_cut(&authority, &candidate_uncles);
    let newer_partial = convergence.begin_partial(TemplateComponent::Proposals, pending);
    assert_eq!(
        convergence
            .publish_partial(newer_partial)
            .expect("fixture revision has capacity"),
        TemplatePublication::Published
    );
    assert_eq!(
        convergence
            .publish_full(old_full)
            .expect("fixture revision has capacity"),
        TemplatePublication::Published,
        "full wins publication even after a newer partial"
    );
    assert!(convergence.is_pending(TemplateComponent::Proposals));
    assert!(convergence.is_pending(TemplateComponent::Transactions));
    assert!(convergence.is_pending(TemplateComponent::Uncles));

    let racing_proposals = convergence.begin_partial(TemplateComponent::Proposals, pending);
    let racing_transactions = convergence.begin_partial(TemplateComponent::Transactions, pending);
    assert_eq!(
        convergence
            .publish_partial(racing_proposals)
            .expect("fixture revision has capacity"),
        TemplatePublication::Published
    );
    assert_eq!(
        convergence
            .publish_partial(racing_transactions)
            .expect("fixture revision has capacity"),
        TemplatePublication::Stale,
        "partial publications use an exact shared revision"
    );
    assert!(!convergence.is_pending(TemplateComponent::Proposals));
    assert!(convergence.is_pending(TemplateComponent::Transactions));

    for component in [TemplateComponent::Transactions, TemplateComponent::Uncles] {
        let build = convergence.begin_partial(component, pending);
        assert_eq!(
            convergence
                .publish_partial(build)
                .expect("fixture revision has capacity"),
            TemplatePublication::Published
        );
    }
    assert!(convergence.is_converged());
    convergence.observe_sources(empty);
    assert!(
        convergence.is_converged(),
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
    for component in [TemplateComponent::Proposals, TemplateComponent::Uncles] {
        let build = convergence.begin_partial(component, gap);
        assert_eq!(
            convergence
                .publish_partial(build)
                .expect("fixture revision has capacity"),
            TemplatePublication::Published
        );
    }
    assert!(convergence.is_converged());

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
    let uncle_build = convergence.begin_partial(TemplateComponent::Uncles, uncle_changed);
    assert_eq!(
        convergence
            .publish_partial(uncle_build)
            .expect("fixture revision has capacity"),
        TemplatePublication::Published
    );

    let stale_full = convergence.begin_full(uncle_changed);
    let first_reset = convergence
        .mark_reset(uncle_changed)
        .expect("fixture reset epoch has capacity");
    assert_eq!(
        convergence
            .publish_reset(first_reset)
            .expect("fixture revision has capacity"),
        TemplatePublication::Published
    );
    assert_eq!(
        convergence
            .publish_full(stale_full)
            .expect("fixture revision has capacity"),
        TemplatePublication::Stale,
        "a full build cannot cross a published reset"
    );
    assert!(!convergence.is_converged());

    let superseded_reset = convergence
        .mark_reset(uncle_changed)
        .expect("fixture reset epoch has capacity");
    let latest_reset = convergence
        .mark_reset(uncle_changed)
        .expect("fixture reset epoch has capacity");
    assert_eq!(
        convergence
            .publish_reset(superseded_reset)
            .expect("fixture revision has capacity"),
        TemplatePublication::Stale
    );
    assert_eq!(
        convergence
            .publish_reset(latest_reset)
            .expect("fixture revision has capacity"),
        TemplatePublication::Published
    );
    let final_full = convergence.begin_full(uncle_changed);
    assert_eq!(
        convergence
            .publish_full(final_full)
            .expect("fixture revision has capacity"),
        TemplatePublication::Published
    );
    assert!(convergence.is_converged());
}

#[test]
fn uak_template_counter_exhaustion_is_typed_and_mutation_free() {
    let authority = TxPoolAuthority::for_foundation(limits());
    let candidate_uncles = CandidateUncles::new();
    let sources = source_cut(&authority, &candidate_uncles);

    let mut revision = TemplateConvergence::new(sources);
    revision.force_revision_for_foundation(u64::MAX);
    let full = revision.begin_full(sources);
    let before = revision.clone();
    assert_eq!(
        revision.publish_full(full),
        Err(TemplateConvergenceError::RevisionExhausted)
    );
    assert_eq!(revision, before);

    let mut reset = TemplateConvergence::new(sources);
    reset.force_reset_epoch_for_foundation(u64::MAX);
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
    let mut convergence = TemplateConvergence::new(sources);
    let initial = convergence.begin_full(sources);
    assert_eq!(
        convergence
            .publish_full(initial)
            .expect("fixture revision has capacity"),
        TemplatePublication::Published
    );
    assert!(convergence.is_converged());

    {
        let _dropped = convergence
            .mark_reset(sources)
            .expect("fixture reset epoch has capacity");
    }
    assert!(
        !convergence.is_converged(),
        "an unconsumed reset level cannot look quiescent"
    );
    let retry = convergence
        .begin_pending_reset()
        .expect("the exact pending reset capability is reconstructible");
    assert_eq!(
        convergence
            .publish_reset(retry)
            .expect("fixture revision has capacity"),
        TemplatePublication::Published
    );
    assert!(convergence.begin_pending_reset().is_none());
    assert!(
        !convergence.is_converged(),
        "blank reset content still needs a full component rebuild"
    );
    let rebuilt = convergence.begin_full(sources);
    assert_eq!(
        convergence
            .publish_full(rebuilt)
            .expect("fixture revision has capacity"),
        TemplatePublication::Published
    );
    assert!(convergence.is_converged());
}

#[test]
fn uak_requested_reset_fences_an_older_full_before_reset_publication() {
    let authority = TxPoolAuthority::for_foundation(limits());
    let candidate_uncles = CandidateUncles::new();
    let sources = source_cut(&authority, &candidate_uncles);
    let mut convergence = TemplateConvergence::new(sources);
    let initial = convergence.begin_full(sources);
    assert_eq!(
        convergence
            .publish_full(initial)
            .expect("fixture revision has capacity"),
        TemplatePublication::Published
    );

    let old_full = convergence.begin_full(sources);
    let reset = convergence
        .mark_reset(sources)
        .expect("fixture reset epoch has capacity");
    assert_eq!(
        convergence
            .publish_full(old_full)
            .expect("fixture revision has capacity"),
        TemplatePublication::Stale,
        "the reset request, not scheduler timing, fences an older full build"
    );

    let post_request_full = convergence.begin_full(sources);
    assert_eq!(
        convergence
            .publish_reset(reset)
            .expect("fixture revision has capacity"),
        TemplatePublication::Published
    );
    assert_eq!(
        convergence
            .publish_full(post_request_full)
            .expect("fixture revision has capacity"),
        TemplatePublication::Published,
        "a full built for the exact reset epoch may publish after that reset"
    );
    assert!(convergence.is_converged());
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
        Vec::new(),
        ChainPackagingMode::Package,
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
    apply_without_work(
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
