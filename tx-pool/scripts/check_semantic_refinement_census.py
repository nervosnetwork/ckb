#!/usr/bin/env python3
"""Validate the bidirectional semantic-axis and model-input producer census."""

from __future__ import annotations

import copy
import hashlib
import json
from pathlib import Path
import re
import sys

import check_model_refinement


REPO_ROOT = Path(__file__).resolve().parents[2]
CENSUS_PATH = (
    REPO_ROOT
    / "tx-pool"
    / "optimization-evidence"
    / "semantic-refinement-census.json"
)
CONTRACT_PATH = REPO_ROOT / "tx-pool" / "architecture-contract.json"
BEHAVIOR_REGISTRY_PATH = REPO_ROOT / "tx-pool" / "review-behaviors.json"

REQUIRED_AXIS_IDS = {
    "SA-IDENTITY-EVIDENCE",
    "SA-NONCONTEXTUAL-VALIDITY",
    "SA-CONTEXTUAL-ELIGIBILITY",
    "SA-INGRESS-SOURCE-DUPLICATE",
    "SA-OWNER-CAPABILITY-ATOMICITY",
    "SA-DEPENDENCY-CONDITIONAL-GRAPH",
    "SA-CONFLICT-RBF-HISTORY",
    "SA-RESOURCE-ADMISSION-EVICTION-EXPIRY",
    "SA-SCHEDULING-COMPUTE-CONCURRENCY",
    "SA-PROPOSAL-WINDOW-TWO-STEP",
    "SA-CHAIN-REORG-RECOVERY",
    "SA-TEMPLATE-SELECTION-UNCLES",
    "SA-EFFECTS-QUERY-RELAY",
    "SA-CONFIG-PERSISTENCE-API-OPERATIONS",
    "SA-TASK-FAULT-SHUTDOWN",
    "SA-LIVENESS-FAIRNESS-PROGRESS",
}

PHASE4_CLOSED_MODEL_RELATION_STATUS = (
    "proved_current_candidate_cut_pending_architecture_optimality_synthesis"
)
PHASE4_RELEASE_BOUNDARY_MODEL_RELATION_STATUS = (
    "proved_model_relation_current_candidate_cut_pending_release_boundary_adjudication"
)
REQUIRED_PHASE4_MODEL_RELATIONS = {
    axis_id: {
        "status": PHASE4_CLOSED_MODEL_RELATION_STATUS,
        "owner": "architecture_optimality_synthesis",
    }
    for axis_id in REQUIRED_AXIS_IDS
}
REQUIRED_PHASE4_MODEL_RELATIONS["SA-CONFIG-PERSISTENCE-API-OPERATIONS"] = {
    "status": PHASE4_RELEASE_BOUNDARY_MODEL_RELATION_STATUS,
    "owner": "release_boundary_adjudication",
}

REQUIRED_SOURCE_IDS = {
    "NS-CONSENSUS-PROPOSAL-WINDOW",
    "NS-OPTIMIZATION-CONTRACT",
    "PS-TRANSACTION-VERIFIER",
    "PS-CONTEXTUAL-BLOCK",
    "PS-PROPOSAL-TABLE",
    "OS-VERIFICATION-SWITCH",
    "OS-IMPORT-VERIFY-MODE",
    "OS-REPLAY-VERIFY-MODE",
    "CS-RPC-POOL",
    "CS-JSONRPC-POOL",
    "CS-TXPOOL-CONFIG",
    "CS-LEGACY-CONFIG",
    "CS-PERSISTENCE",
    "ES-BEHAVIOR-REGISTRY",
    "ES-INTEGRATION-IMPACT",
    "ES-DIFFERENTIAL-CENSUS",
}
REQUIRED_EPISTEMIC_SOURCE_ROLES = {
    "NS-CONSENSUS-PROPOSAL-WINDOW": (
        "protocol_proposal_commit_distance_parameter_definition"
    ),
    "PS-TRANSACTION-VERIFIER": (
        "production_consensus_transaction_validity_and_time_relative_enforcement"
    ),
    "PS-CONTEXTUAL-BLOCK": (
        "production_consensus_two_step_commit_enforcement_over_main_and_uncle_proposals"
    ),
    "PS-PROPOSAL-TABLE": (
        "candidate_primitive_history_to_exact_persistent_projection_under_refinement"
    ),
    "OS-VERIFICATION-SWITCH": (
        "production_verification_mode_and_explicit_operator_bypass_definition"
    ),
    "OS-IMPORT-VERIFY-MODE": (
        "operator_owned_import_mode_selection_boundary"
    ),
    "OS-REPLAY-VERIFY-MODE": (
        "operator_owned_replay_mode_selection_boundary"
    ),
}
OPTIMIZATION_CONTRACT_SEMANTIC_FIELDS = [
    "optimization_goal",
    "refactor_admissibility",
    "proof_policy",
    "root_families",
    "target_invariants",
    "release_surface",
    "residual_risks",
    "historical_convergence",
    "model_boundary_algebra",
    "model_family_evidence",
    "refinement_inventory",
]

REQUIRED_SOURCE_SET_PATTERNS = {
    "production": [
        "tx-pool/src/*.rs",
        "tx-pool/src/authority/**/*.rs",
        "tx-pool/src/block_assembler/**/*.rs",
        "tx-pool/src/component/**/*.rs",
        "tx-pool/src/service/**/*.rs",
    ],
    "model": ["tx-pool/src/tests/model/*.rs"],
    "integration": ["@integration-impact.groups"],
}

ALLOWED_PRODUCER_KINDS = {
    "protocol_primitive",
    "typed_environment",
    "authority_state",
    "pure_derivation",
    "cost_summary",
}
ALLOWED_RELATION_STATUS = {
    "proved",
    "typed_assumption",
    "invalid_free_derived",
    "incomplete_evidence_identity",
    "missing_congruence_proof",
    "missing_production_provenance",
}
OPEN_RELATION_STATUS = ALLOWED_RELATION_STATUS - {"proved", "typed_assumption"}
FORBIDDEN_FREE_DERIVED = {
    "MI-RESOLVED-EVIDENCE.proposal_status",
    "MI-CHAIN-TRANSITION.proposed",
    "MI-CHAIN-TRANSITION.gap",
    "MI-QUERY-SUBJECT.proposal_window_status",
}
REQUIRED_PROPOSAL_INPUT_STATUS = {
    "MI-CHAIN-TRANSITION.proposals": "typed_assumption",
    "MI-PROPOSAL-WINDOW.closest": "typed_assumption",
    "MI-PROPOSAL-WINDOW.farthest": "typed_assumption",
    "MI-PROPOSAL-BLOCK.height": "typed_assumption",
    "MI-PROPOSAL-BLOCK.main": "typed_assumption",
    "MI-PROPOSAL-BLOCK.uncles": "typed_assumption",
    "MI-PROPOSAL-CONTEXT.tip_height": "typed_assumption",
    "MI-PROPOSAL-CONTEXT.window": "typed_assumption",
    "MI-PROPOSAL-CONTEXT.blocks": "typed_assumption",
    "MI-PROPOSAL-CONTEXT.admission": "typed_assumption",
    "MI-EVICTION-REFINEMENT.status": "proved",
}
REQUIRED_PRODUCTION_PROVENANCE_STATUS = {
    "MI-EVICTION-PRODUCTION-STATUS-RECEIPT.receipt": "proved",
}
REQUIRED_TWO_PHASE_INPUT_STATUS = {
    "MI-ACCEPTED-ORDER-INPUT.own": "proved",
    "MI-ACCEPTED-ORDER-INPUT.ancestors": "proved",
    "MI-ACCEPTED-ORDER-INPUT.arrival": "proved",
    "MI-ACCEPTED-ORDER-INPUT.identity": "proved",
    "MI-CONDITIONAL-SELECTION-INPUT.causal_parents": "proved",
    "MI-CONDITIONAL-SELECTION-INPUT.conditional_predecessors": "proved",
    "MI-CONDITIONAL-SELECTION-INPUT.priority": "proved",
    "MI-CONDITIONAL-SELECTION-INPUT.eviction_order": "proved",
    "MI-TEMPLATE-PACKING-INPUT.own": "proved",
    "MI-TEMPLATE-PACKING-INPUT.causal_parents": "proved",
    "MI-TEMPLATE-PACKING-INPUT.proposed": "proved",
    "MI-TEMPLATE-PACKING-INPUT.arrival": "proved",
    "MI-TEMPLATE-PACKING-INPUT.identity": "proved",
    "MI-TEMPLATE-PACKING-LIMITS.serialized_bytes": "typed_assumption",
    "MI-TEMPLATE-PACKING-LIMITS.cycles": "typed_assumption",
    "MI-DEPENDENCY-SCAN-INPUT.causal_parents": "proved",
    "MI-DEPENDENCY-SCAN-INPUT.dependency_edges": "proved",
    "MI-TEMPLATE-SERVICE-CANDIDATE.proposal": "proved",
    "MI-TEMPLATE-SERVICE-CANDIDATE.status": "proved",
    "MI-TEMPLATE-SERVICE-CANDIDATE.own": "proved",
    "MI-TEMPLATE-SERVICE-CANDIDATE.causal_parents": "proved",
    "MI-TEMPLATE-SERVICE-CANDIDATE.conditional_predecessors": "proved",
    "MI-TEMPLATE-SERVICE-CANDIDATE.dependency_edges": "proved",
    "MI-TEMPLATE-SERVICE-CANDIDATE.eviction_order": "proved",
    "MI-TEMPLATE-SERVICE-CANDIDATE.arrival": "proved",
    "MI-TEMPLATE-SERVICE-CANDIDATE.identity": "proved",
    "MI-CANDIDATE-UNCLE-INPUT.id": "typed_assumption",
    "MI-CANDIDATE-UNCLE-INPUT.proposals": "typed_assumption",
    "MI-CANDIDATE-UNCLE-INPUT.serialized_bytes": "typed_assumption",
    "MI-CURRENT-TEMPLATE-COMPOSITION.max_block_proposals": "typed_assumption",
    "MI-CURRENT-TEMPLATE-COMPOSITION.proposal_id_bytes": "typed_assumption",
    "MI-CURRENT-TEMPLATE-COMPOSITION.candidate_uncles": "typed_assumption",
    "MI-CURRENT-TEMPLATE-COMPOSITION.base_bytes": "typed_assumption",
    "MI-CURRENT-TEMPLATE-COMPOSITION.max_block_bytes": "typed_assumption",
    "MI-CURRENT-TEMPLATE-COMPOSITION.max_block_cycles": "typed_assumption",
    "MI-TEMPLATE-SERVICE-SOURCE-CUT.candidates": "proved",
    "MI-TEMPLATE-SERVICE-SOURCE-CUT.composition": "typed_assumption",
    "MI-TEMPLATE-SERVICE-SOURCE-CUT.captured_dependency_edge_bound": "proved",
    "MI-TEMPLATE-SERVICE-PREMISE.source": "proved",
    "MI-TEMPLATE-SERVICE-PREMISE.cohort": "proved",
    "MI-TEMPLATE-SERVICE-PREMISE.packed_source_indices": "proved",
    "MI-TEMPLATE-SERVICE-PREMISE.retained_source_indices": "proved",
    "MI-TEMPLATE-SERVICE-PREMISE.shed_source_indices": "proved",
    "MI-CANONICAL-SERVICE-PREMISE.proposal_service_bound": "typed_assumption",
    "MI-CANONICAL-SERVICE-PREMISE.window": "typed_assumption",
}
REQUIRED_COMPLETION_RELATIONS = {
    "MI-WORK-RESULT.Resolved",
    "MI-WORK-RESULT.Verified",
    "MI-WORK-RESULT.Missing",
    "MI-WORK-RESULT.Rejected",
    "MI-WORK-RESULT.VerificationRejected",
    "MI-WORK-RESULT.Retry",
    "MI-SETTLEMENT-REJECTION.ChainBound",
    "MI-SETTLEMENT-REJECTION.ResourceBound",
}
REQUIRED_DEPENDENCY_INPUT_STATUS = {
    "MI-TRANSACTION.inputs": "proved",
    "MI-TRANSACTION.cell_deps": "proved",
    "MI-TRANSACTION.dep_groups": "proved",
    "MI-TRANSACTION.header_deps": "proved",
    "MI-RESOLVED-EVIDENCE.input_origins": "proved",
    "MI-RESOLVED-EVIDENCE.dep_group_members": "proved",
    "MI-RESOLVED-EVIDENCE.dep_origins": "proved",
    "MI-RESOLVED-EVIDENCE.header_deps": "proved",
    "MI-WORK-RESULT.Resolved": "proved",
    "MI-WORK-RESULT.Missing": "proved",
}
REQUIRED_COST_RELATIONS = {
    "MI-TRANSACTION.cost",
    "MI-TRANSACTION-COST.payload_bytes",
    "MI-TRANSACTION-COST.fee",
    "MI-TRANSACTION-COST.cycles",
    "MI-RESOLVED-EVIDENCE.verify_class",
    "MI-PRODUCTION-COST-RECEIPT.metrics",
}
OPEN_PRODUCTION_REFINEMENT_STATUS = "missing_production_provenance"
PROVED_PRODUCTION_REFINEMENT_STATUS = "proved"
PRODUCTION_REFINEMENT_FIELDS = {
    "id",
    "axis_ids",
    "dynamic_composition_ref",
    "model_anchors",
    "production_producers",
    "production_consumers",
    "required_relation",
    "status",
    "falsifier",
    "negative_canary",
}
REQUIRED_PRODUCTION_REFINEMENT_EDGES = {
    "PR-SCRIPT-PROOF-QUOTIENT": {
        "axis_ids": {
            "SA-IDENTITY-EVIDENCE",
            "SA-CONTEXTUAL-ELIGIBILITY",
        },
        "dynamic_composition_ref": "DR-COST-RECEIPT-OBSERVATION",
        "model_anchors": {
            "tx-pool/src/tests/model/boundaries.rs::ModelScriptProof",
            "tx-pool/src/tests/model/properties.rs::model_script_proof_requires_vm_success_and_rechecks_the_current_max",
            "tx-pool/src/tests/model/properties.rs::model_script_cache_hot_and_cold_observations_are_equal_for_every_current_limit",
        },
        "producer_anchors": {
            "verification/src/cache.rs::TxVerificationCacheKey",
            "verification/src/cache.rs::ScriptVerificationProof",
        },
        "consumer_anchors": {
            "tx-pool/src/util.rs::verify_rtx",
        },
        "required_relation": (
            "cache_key_is_exactly_witness_hash_plus_script_rules_and_cache_value_is_a_"
            "sealed_successful_VM_script_proof_containing_only_cycles_while_every_reuse_"
            "rechecks_the_current_max_in_O1_without_adding_max_to_identity"
        ),
        "falsifier": (
            "a_cache_value_carries_fee_or_non_script_authority_can_be_constructed_without_"
            "VM_success_or_a_hot_result_over_current_max_is_accepted"
        ),
        "negative_canary": "cache_completed_value_still_contains_fee_or_hot_path_omits_current_max",
    },
    "PR-REMOTE-CYCLE-LIMIT": {
        "axis_ids": {
            "SA-IDENTITY-EVIDENCE",
            "SA-CONTEXTUAL-ELIGIBILITY",
            "SA-INGRESS-SOURCE-DUPLICATE",
            "SA-SCHEDULING-COMPUTE-CONCURRENCY",
        },
        "dynamic_composition_ref": "DR-COST-RECEIPT-OBSERVATION",
        "model_anchors": {
            "tx-pool/src/tests/model/boundaries.rs::ModelRemoteCycleLimit",
            "tx-pool/src/tests/model/boundaries.rs::ModelRemoteCycleObservation",
            "tx-pool/src/tests/model/boundaries.rs::remote_cycle_observation",
            "tx-pool/src/tests/model/properties.rs::model_remote_cycle_limit_is_sealed_at_every_ingress_route",
        },
        "producer_anchors": {
            "sync/src/relayer/transactions_process.rs::impl<'a> TransactionsProcess<'a>",
            "tx-pool/src/service/controller.rs::pub async fn submit_remote_tx",
            "tx-pool/src/authority/ingress.rs::pub(super) fn remote",
            "tx-pool/src/authority/state.rs::enum PayloadPolicy",
            "tx-pool/src/authority/resolver.rs::struct TxPoolVerificationRequest",
        },
        "consumer_anchors": {
            "tx-pool/src/util.rs::verify_rtx",
            "tx-pool/src/authority/work.rs::fn verified",
        },
        "required_relation": (
            "network_relay_and_direct_tx_pool_submission_both_terminate_at_the_tx_pool_"
            "ingress_checked_d_at_most_M_constructor_before_ownership_or_VM_work_then_use_"
            "the_same_sealed_d_as_the_script_limit_and_compare_actual_cycles_to_d_after_DAO"
        ),
        "falsifier": (
            "a_direct_controller_submission_can_retain_d_above_M_or_any_downstream_sibling_"
            "can_construct_an_unchecked_remote_limit_or_hot_and_cold_paths_use_d_and_M_differently"
        ),
        "negative_canary": (
            "remove_the_ingress_check_or_replace_the_sealed_limit_with_a_raw_cycle"
        ),
    },
    "PR-LOCATION-REFRESH-METRICS": {
        "axis_ids": {
            "SA-CONTEXTUAL-ELIGIBILITY",
            "SA-RESOURCE-ADMISSION-EVICTION-EXPIRY",
        },
        "dynamic_composition_ref": "DR-COST-RECEIPT-OBSERVATION",
        "model_anchors": {
            "tx-pool/src/tests/model/evidence_transition.rs::ModelLocationRefreshObservation",
            "tx-pool/src/tests/model/evidence_transition.rs::location_refresh_observation",
            "tx-pool/src/tests/model/evidence_transition_properties.rs::model_location_refresh_atomically_reseals_all_location_dependent_metrics",
        },
        "producer_anchors": {
            "tx-pool/src/authority/validation.rs::fn refresh_locations",
            "tx-pool/src/authority/state.rs::fn with_refreshed_locations",
            "tx-pool/src/authority/state.rs::fn with_final_validation",
            "tx-pool/src/component/entry.rs::accepted_transaction_charge_bytes",
        },
        "consumer_anchors": {
            "tx-pool/src/authority/plan/membership.rs::prepare_membership_candidate",
        },
        "required_relation": (
            "a_location_change_recomputes_fee_resolved_and_accepted_resident_bytes_from_the_"
            "same_refreshed_resolution_cut_rechecks_the_configured_minimum_fee_and_atomically_"
            "commits_payload_context_and_candidate_metrics_before_membership_planning"
        ),
        "falsifier": (
            "any_pre_refresh_fee_or_resident_charge_survives_a_changed_location_or_committed_"
            "metrics_differ_from_an_independent_recomputation_or_a_fresh_low_fee_is_admitted"
        ),
        "negative_canary": (
            "copy_the_old_fee_or_resident_bytes_or_remove_the_post_refresh_minimum_fee_check"
        ),
    },
    "PR-PROPOSAL-VERIFIER-PROJECTION": {
        "axis_ids": {
            "SA-CONTEXTUAL-ELIGIBILITY",
            "SA-PROPOSAL-WINDOW-TWO-STEP",
            "SA-LIVENESS-FAIRNESS-PROGRESS",
        },
        "dynamic_composition_ref": "DR-PROPOSAL-REORG-TEMPLATE-SERVICE",
        "model_anchors": {
            "tx-pool/src/tests/model/two_phase.rs::fn verify_candidate_block",
            "tx-pool/src/tests/model/time_context.rs::ModelProposalTimeRelation",
            "tx-pool/src/tests/model/time_context.rs::model_proposal_time_observation",
            "tx-pool/src/tests/model/two_phase.rs::model_txpool_projection_excludes_genesis_history",
        },
        "producer_anchors": {
            "util/proposal-table/src/lib.rs::pub struct ProposalTable",
            "verification/contextual/src/contextual_block_verifier.rs::pub struct TwoPhaseCommitVerifier",
        },
        "consumer_anchors": {
            "tx-pool/src/authority/validation.rs::pub(super) fn verification_environment",
        },
        "required_relation": (
            "proposal_table_Proposed_excludes_genesis_and_equals_the_primitive_history_set_"
            "read_by_TwoPhaseCommitVerifier_for_the_next_candidate_across_every_legal_window_"
            "tip_and_reorg_while_Proposed_time_is_exactly_tip_plus_one_and_the_lossy_Gap_"
            "time_projection_remains_conservative"
        ),
        "falsifier": (
            "height_zero_enters_Proposed_or_a_legal_window_or_reorg_makes_Proposed_differ_"
            "from_the_next_block_verifier_set_or_either_status_time_bound_is_premature"
        ),
        "negative_canary": (
            "remove_the_genesis_guard_reintroduce_a_fixed_Proposed_age_or_relabel_the_"
            "lossy_Gap_bound_as_an_exact_primitive_occurrence"
        ),
    },
    "PR-FEE-POLICY-CONFIGURATION": {
        "axis_ids": {
            "SA-CONTEXTUAL-ELIGIBILITY",
            "SA-CONFLICT-RBF-HISTORY",
            "SA-RESOURCE-ADMISSION-EVICTION-EXPIRY",
        },
        "dynamic_composition_ref": "DR-COST-RECEIPT-OBSERVATION",
        "model_anchors": {
            "tx-pool/src/tests/model/boundaries.rs::ModelMinimumFeeObservation",
            "tx-pool/src/tests/model/state.rs::ModelFeeRate",
            "tx-pool/src/tests/model/state.rs::ModelReplacementPolicy",
            "tx-pool/src/tests/model/properties.rs::model_fee_policy_matches_configured_production_arithmetic",
        },
        "producer_anchors": {
            "tx-pool/src/authority/runtime.rs::struct ResolutionPolicy",
            "tx-pool/src/authority/plan/membership.rs::enum ReplacementPolicy",
            "tx-pool/src/authority/plan/membership/rbf.rs::fn validate_replacement_fee",
        },
        "consumer_anchors": {
            "tx-pool/src/authority/resolver.rs::fn finish_resolution",
        },
        "required_relation": (
            "minimum_fee_and_RBF_increment_use_the_configured_FeeRate_saturating_rate_times_"
            "block_serialized_bytes_divided_by_1000_and_replacement_disabled_is_a_distinct_"
            "policy_observation"
        ),
        "falsifier": (
            "the_model_uses_a_fixed_increment_or_payload_bytes_or_cannot_represent_disabled_"
            "replacement_while_production_uses_the_configured_policy"
        ),
        "negative_canary": "model_replaces_configured_FeeRate_with_a_fixed_raw_byte_increment",
    },
    "PR-DEPENDENCY-SCAN-NONCENSORSHIP": {
        "axis_ids": {
            "SA-DEPENDENCY-CONDITIONAL-GRAPH",
            "SA-TEMPLATE-SELECTION-UNCLES",
            "SA-LIVENESS-FAIRNESS-PROGRESS",
        },
        "dynamic_composition_ref": "DR-PROPOSAL-REORG-TEMPLATE-SERVICE",
        "model_anchors": {
            "tx-pool/src/tests/model/two_phase.rs::fn complete_dependency_scan_refinement",
            "tx-pool/src/tests/model/properties.rs::model_complete_dependency_scan_preserves_causal_and_independent_work",
            "tx-pool/src/tests/model/properties.rs::model_complete_dependency_scan_is_order_invariant_for_independent_work",
        },
        "producer_anchors": {
            "tx-pool/src/authority/template.rs::pub(super) struct AuthorityTemplateReadReceipt",
            "tx-pool/src/authority/template.rs::pub(super) struct TemplateSelectionReceipt",
        },
        "consumer_anchors": {
            "tx-pool/src/authority/template.rs::fn order_conditionally_safe",
        },
        "required_relation": (
            "the_immutable_template_receipt_checked_sums_every_deduplicated_dependency_edge_"
            "and_the_selected_subset_scans_its_complete_finite_domain_without_dropping_"
            "semantically_selected_work_before_parent_first_packing"
        ),
        "falsifier": (
            "a_prefix_or_position_budget_drops_selected_causal_or_independent_work_or_the_"
            "captured_edge_bound_is_not_the_exact_checked_sum_of_the_immutable_footprints"
        ),
        "negative_canary": "production_reintroduces_dependency_budget_truncation_or_a_false_captured_bound",
    },
    "PR-SHUTDOWN-EFFECT-DRAIN": {
        "axis_ids": {
            "SA-EFFECTS-QUERY-RELAY",
            "SA-TASK-FAULT-SHUTDOWN",
            "SA-LIVENESS-FAIRNESS-PROGRESS",
        },
        "dynamic_composition_ref": "DR-AUTHORITY-LIFECYCLE-OBSERVATION",
        "model_anchors": {
            "tx-pool/src/tests/model/protocol.rs::ModelCoordinatorCompletionReadiness",
            "tx-pool/src/tests/model/protocol.rs::ModelEffectBlockedShutdownCut",
            "tx-pool/src/tests/model/protocol.rs::completion_ingress_shutdown_step",
            "tx-pool/src/tests/model/properties.rs::model_completion_ingress_closure_strictly_lowers_the_effect_drain_rank",
            "tx-pool/formal/PermitEffect.tla::DisconnectIngress",
        },
        "producer_anchors": {
            "tx-pool/src/authority/compute_coordinator.rs::struct ComputeCoordinator",
            "tx-pool/src/authority/compute_coordinator.rs::enum CompletionIngress",
        },
        "consumer_anchors": {
            "tx-pool/src/authority/compute_coordinator.rs::async fn run",
            "tx-pool/src/authority/compute_coordinator.rs::fn is_drained",
        },
        "required_relation": (
            "during_shutdown_the_first_disconnected_completion_observation_atomically_"
            "terminalizes_the_completion_ingress_and_removes_that_permanently_ready_arm_so_"
            "each_later_effect_capacity_notification_strictly_lowers_the_finite_waiter_rank"
        ),
        "falsifier": (
            "a_disconnected_receive_remains_enabled_after_returning_None_or_ingress_closure_"
            "outside_shutdown_is_silently_treated_as_a_clean_drain"
        ),
        "negative_canary": "production_restores_a_noop_shutdown_None_arm_or_drops_the_command_owner_in_the_abnormal_close_fixture",
    },
}


def script_proof_refinement_errors(
    cache_source: str,
    verifier_source: str,
    txpool_source: str,
    production_test_source: str,
) -> list[str]:
    """Prove the current tx-pool script-cache quotient from independent cuts."""

    errors: list[str] = []
    cache_compact = "".join(cache_source.split())
    verifier_compact = "".join(verifier_source.split())
    if (
        "structVmSuccessSeal{cycles:Cycle,}" not in cache_compact
        or "pubstructScriptVerificationProof{key:TxVerificationCacheKey,seal:VmSuccessSeal,}"
        not in cache_compact
        or "inner:lru::LruCache<TxVerificationCacheKey,VmSuccessSeal>" not in cache_compact
    ):
        errors.append("script proof cache is not a sealed cycles-only quotient")
    if (
        "pub(crate)constfnfrom_vm_success(" not in cache_compact
        or cache_compact.count("seal:VmSuccessSeal{cycles}") != 1
        or cache_compact.count(".map(|seal|ScriptVerificationProof{key:*key,seal})") != 1
    ):
        errors.append("script proof construction is not confined to the verification crate")
    if (
        "cached.filter(|proof|proof.key()==self.cache_key)" not in verifier_compact
        or "ifproof.cycles()>max_cycles" not in verifier_compact
        or verifier_source.count("ScriptVerificationProof::from_vm_success") != 2
    ):
        errors.append("script verifier does not bind identity, current max and VM-success origin")
    for token in (
        "cache_entry: Option<ScriptVerificationProof>",
        "max_tx_verify_cycles: Cycle",
        ".verify_scripts(max_tx_verify_cycles, cache_entry)",
        ".verify_with_pause(max_tx_verify_cycles, cache_entry, command_rx)",
    ):
        if token not in txpool_source:
            errors.append(f"tx-pool script proof consumer lacks {token}")
    for token in (
        "fn uak_verification_request_binds_environment_rules_and_witness_cache_key",
        "fn uak_verification_cache_lookup_cannot_substitute_a_nearby_request",
        "a real cache miss publishes VM-success evidence",
    ):
        if token not in production_test_source:
            errors.append(f"script proof production refinement evidence lacks {token}")
    return errors


def validate_script_proof_refinement_sources() -> list[str]:
    try:
        return script_proof_refinement_errors(
            (REPO_ROOT / "verification/src/cache.rs").read_text(),
            (REPO_ROOT / "verification/src/transaction_verifier.rs").read_text(),
            (REPO_ROOT / "tx-pool/src/util.rs").read_text(),
            (REPO_ROOT / "tx-pool/src/authority/tests/resolver.rs").read_text(),
        )
    except OSError as error:
        return [f"cannot inspect script proof refinement: {error}"]


def current_candidate_gap_errors(
    edge_id: str,
    model_source: str,
    production_source: str,
    production_test_source: str,
    boundary_source: str = "",
    boundary_test_source: str = "",
    relay_source: str = "",
    relay_test_source: str = "",
) -> list[str]:
    """Require each registered relation to match its live production shape."""

    errors: list[str] = []
    if edge_id == "PR-REMOTE-CYCLE-LIMIT":
        for token in (
            "enum ModelRemoteCycleObservation",
            "fn remote_cycle_observation",
            "fn model_remote_cycle_limit_is_sealed_at_every_ingress_route",
            "ModelRemoteCycleLimit::checked(declared, consensus_max)",
        ):
            if token not in model_source:
                errors.append(f"remote-cycle sealed model lacks {token}")
        compact = "".join(production_source.split())
        for token in (
            "pub(super)structRemoteCycleLimit(Cycle);",
            "fnchecked(declared:Cycle,consensus:&Consensus)->Option<Self>",
            "declared<=consensus.max_block_cycles()",
            "RemoteDeclaredCycles(super::ingress::RemoteCycleLimit)",
            "RemoteCycleLimit::checked(declared_cycles,consensus)",
            "limit.declared()==outcome.cycles()",
        ):
            if token not in compact:
                errors.append(f"remote-cycle sealed production lacks {token}")
        for token in (
            "fn uak_remote_ingress_rejects_a_declaration_above_consensus_max_before_ownership",
            ".max_block_cycles()",
            "attempt.is_malformed_remote()",
        ):
            if token not in production_test_source:
                errors.append(f"remote-cycle ingress observation lacks {token}")
        for token in (
            "pub async fn submit_remote_tx",
            "RemoteTxSubmission::new(transaction, declared_cycles, peer)",
        ):
            if token not in boundary_source:
                errors.append(f"remote-cycle controller boundary lacks {token}")
        for token in (
            "fn remote_submit_transports_an_unchecked_cycle_declaration",
            "assert_eq!(arguments.declared_cycles, u64::MAX)",
            ".submit_remote_tx(transaction, u64::MAX",
        ):
            if token not in boundary_test_source:
                errors.append(f"remote-cycle controller observation lacks {token}")
        for token in (
            "let max_block_cycles = self.relayer.shared().consensus().max_block_cycles()",
            "declared_cycles > &max_block_cycles",
            "relay declared cycles greater than max_block_cycles",
        ):
            if token not in relay_source:
                errors.append(f"remote-cycle relay precheck lacks {token}")
        for token in (
            "fn relay_rejects_a_cycle_declaration_above_consensus_before_tx_pool_handoff",
            "context.banned_peer_reasons()",
            "!state.already_known_tx(&hash)",
        ):
            if token not in relay_test_source:
                errors.append(f"remote-cycle relay observation lacks {token}")
    elif edge_id == "PR-LOCATION-REFRESH-METRICS":
        for token in (
            "struct ModelLocationRefreshObservation",
            "fn location_refresh_observation",
            "fn is_atomically_resealed",
            "fn model_location_refresh_atomically_reseals_all_location_dependent_metrics",
        ):
            if token not in model_source:
                errors.append(f"location-refresh sealed model lacks {token}")
        compact = "".join(production_source.split())
        for token in (
            "check_tx_fee_with_min_fee_rate(",
            "&refreshed,payload.serialized_bytes(),min_fee_rate",
            "resolved_transaction_charge_bytes(self.serialized_bytes,&resolved)",
            "accepted_transaction_charge_bytes(payload.serialized_bytes(),payload.resolved_transaction(),",
            "CandidateMetrics{fee:payload.fee(),cost:AcceptedCost::new(",
        ):
            if token not in compact:
                errors.append(f"location-refresh atomic reseal lacks {token}")
        for token in (
            "fn uak_pool_origin_refresh_is_coupled_and_retires_the_old_payload_outside_apply",
            "fn uak_location_refresh_rechecks_the_configured_minimum_fee",
            "current_candidate.is_atomically_resealed()",
            "accepted_transaction_charge_bytes(",
        ):
            if token not in production_test_source:
                errors.append(f"location-refresh production observation lacks {token}")
    elif edge_id == "PR-PROPOSAL-VERIFIER-PROJECTION":
        for token in (
            "pub(super) fn verify_candidate_block",
            "enum ModelProposalTimeRelation",
            "fn model_proposal_time_observation",
            "fn model_txpool_projection_excludes_genesis_history",
            "fn model_status_only_time_quotient_is_exact_or_conservative_for_legal_fibers",
        ):
            if token not in model_source:
                errors.append(f"proposal projection model lacks {token}")
        for token in (
            "pub struct ProposalTable",
            "pub struct TwoPhaseCommitVerifier",
            "if height == 0",
            "if header.is_genesis()",
            "break;",
            "pub(super) fn verification_environment",
        ):
            if token not in production_source:
                errors.append(f"proposal projection production observation lacks {token}")
        compact = "".join(production_source.split())
        if (
            ".tx_proposal_window().closest().saturating_sub(1)" not in compact
            or "self.table.split_off(&proposal_start.max(1))" not in compact
        ):
            errors.append("proposal projection corrected producer relation is absent")
        for token in (
            "fn two_phase_commit_verifier_and_live_proposal_view_agree_pointwise",
            "fn two_phase_commit_verifier_and_live_proposal_view_agree_after_reorg",
            "fn gap_status_does_not_claim_an_exact_primitive_occurrence",
            "fn uak_verification_environment_refines_phase_owned_commit_bounds",
            "for closest in 1..=4",
            "fn two_phase_commit_verifier_does_not_read_genesis_proposals",
        ):
            if token not in production_test_source:
                errors.append(f"proposal projection bridge lacks {token}")
    elif edge_id == "PR-FEE-POLICY-CONFIGURATION":
        for token in (
            "struct ModelFeeRate",
            "enum ModelReplacementPolicy",
            "enum ModelMinimumFeeObservation",
            "fn model_fee_policy_matches_configured_production_arithmetic",
        ):
            if token not in model_source:
                errors.append(f"fee-policy model lacks {token}")
        for token in (
            "struct ResolutionPolicy",
            "enum ReplacementPolicy",
            "fn validate_replacement_fee",
            "minimum_rate.fee(serialized_bytes)",
            "fn finish_resolution",
            "check_tx_fee_with_min_fee_rate",
        ):
            if token not in production_source:
                errors.append(f"fee-policy production observation lacks {token}")
        for token in (
            "fn uak_configured_fee_rate_refines_production_arithmetic_pointwise",
            "FeeRate::from_u64(rate)",
            "ModelFeeRate::from_u64(rate)",
        ):
            if token not in production_test_source:
                errors.append(f"fee-policy production bridge lacks {token}")
    elif edge_id == "PR-DEPENDENCY-SCAN-NONCENSORSHIP":
        for token in (
            "fn complete_dependency_scan_refinement",
            "fn model_complete_dependency_scan_preserves_causal_and_independent_work",
            "fn model_complete_dependency_scan_is_order_invariant_for_independent_work",
        ):
            if token not in model_source:
                errors.append(f"complete dependency-scan model lacks {token}")
        for token in (
            "struct AuthorityTemplateReadReceipt",
            "struct TemplateSelectionReceipt",
            "dependency_edge_bound",
            "footprint().dependencies().len()",
            "if dependency_count > self.dependency_edge_bound",
            "fn order_conditionally_safe",
        ):
            if token not in production_source:
                errors.append(f"complete dependency-scan production observation lacks {token}")
        if "retain_with_dependency_budget" in production_source:
            errors.append("complete dependency scan reintroduced semantic budget truncation")
        for token in (
            "fn uak_template_complete_dependency_scan_preserves_causal_and_later_independent_work",
            "fn uak_template_complete_dependency_scan_does_not_censor_independent_dependency_work",
            "independent_dependency_scan_selection([200_000, 100_000])",
            "independent_dependency_scan_selection([100_000, 200_000])",
            "complete_dependency_scan_refinement(",
            "captured_edge_bound",
        ):
            if token not in production_test_source:
                errors.append(f"complete dependency-scan production bridge lacks {token}")
    elif edge_id == "PR-SHUTDOWN-EFFECT-DRAIN":
        for token in (
            "enum ModelCoordinatorCompletionReadiness",
            "struct ModelEffectBlockedShutdownCut",
            "fn completion_ingress_shutdown_step",
            "CompletionIngressClosed",
            "fn model_completion_ingress_closure_strictly_lowers_the_effect_drain_rank",
            "DisconnectIngress ==",
        ):
            if token not in model_source:
                errors.append(f"shutdown-effect model lacks {token}")
        for token in (
            "struct ComputeCoordinator",
            "enum CompletionIngress",
            "completion_ingress: CompletionIngress",
            "async fn run",
            "biased;",
            "received = completion.recv(), if self.completion_ingress == CompletionIngress::Open",
            "self.completion_ingress = CompletionIngress::Closed",
            "_ = effect_notified.as_mut(), if wait_effect",
            "fn is_drained",
            "self.exchange_after_effect.is_empty()",
        ):
            if token not in production_source:
                errors.append(f"shutdown-effect production observation lacks {token}")
        if "None if self.shutting_down => {}" in production_source:
            errors.append("shutdown completion ingress restored the permanently ready no-op arm")
        for token in (
            "fn uak_shutdown_drains_effect_blocked_completion_after_ingress_closes",
            "fn uak_completion_ingress_close_outside_shutdown_is_a_lifecycle_fault",
            "fn isolated_coordinator",
        ):
            if token not in production_test_source:
                errors.append(f"shutdown completion-ingress bridge lacks {token}")
    return errors


def validate_current_candidate_refinement_sources(edge_id: str) -> list[str]:
    try:
        if edge_id == "PR-REMOTE-CYCLE-LIMIT":
            model = (
                (REPO_ROOT / "tx-pool/src/tests/model/boundaries.rs").read_text()
                + (REPO_ROOT / "tx-pool/src/tests/model/properties.rs").read_text()
            )
            return current_candidate_gap_errors(
                edge_id,
                model,
                (
                    (REPO_ROOT / "tx-pool/src/authority/ingress.rs").read_text()
                    + (REPO_ROOT / "tx-pool/src/authority/state.rs").read_text()
                    + (REPO_ROOT / "tx-pool/src/authority/resolver.rs").read_text()
                    + (REPO_ROOT / "tx-pool/src/authority/work.rs").read_text()
                ),
                (REPO_ROOT / "tx-pool/src/authority/tests/ingress.rs").read_text(),
                (REPO_ROOT / "tx-pool/src/service/controller.rs").read_text(),
                (REPO_ROOT / "tx-pool/src/service/tests/controller.rs").read_text(),
                (REPO_ROOT / "sync/src/relayer/transactions_process.rs").read_text(),
                (REPO_ROOT / "sync/src/relayer/tests/transactions_process.rs").read_text(),
            )
        if edge_id == "PR-LOCATION-REFRESH-METRICS":
            model = (
                (REPO_ROOT / "tx-pool/src/tests/model/evidence_transition.rs").read_text()
                + (
                    REPO_ROOT
                    / "tx-pool/src/tests/model/evidence_transition_properties.rs"
                ).read_text()
            )
            return current_candidate_gap_errors(
                edge_id,
                model,
                (
                    (REPO_ROOT / "tx-pool/src/authority/state.rs").read_text()
                    + (REPO_ROOT / "tx-pool/src/authority/validation.rs").read_text()
                    + (REPO_ROOT / "tx-pool/src/component/entry.rs").read_text()
                ),
                (REPO_ROOT / "tx-pool/src/authority/tests/validation.rs").read_text(),
            )
        if edge_id == "PR-PROPOSAL-VERIFIER-PROJECTION":
            model = (
                (REPO_ROOT / "tx-pool/src/tests/model/proposal.rs").read_text()
                + (REPO_ROOT / "tx-pool/src/tests/model/time_context.rs").read_text()
                + (REPO_ROOT / "tx-pool/src/tests/model/time_context_properties.rs").read_text()
                + (REPO_ROOT / "tx-pool/src/tests/model/two_phase.rs").read_text()
            )
            production = (
                (REPO_ROOT / "util/proposal-table/src/lib.rs").read_text()
                + (REPO_ROOT / "verification/contextual/src/contextual_block_verifier.rs").read_text()
                + (REPO_ROOT / "tx-pool/src/authority/validation.rs").read_text()
            )
            tests = (
                (REPO_ROOT / "util/proposal-table/src/tests.rs").read_text()
                + (REPO_ROOT / "tx-pool/src/authority/tests/validation.rs").read_text()
                + (REPO_ROOT / "verification/contextual/src/tests/contextual_block_verifier.rs").read_text()
            )
            return current_candidate_gap_errors(edge_id, model, production, tests)
        if edge_id == "PR-FEE-POLICY-CONFIGURATION":
            model = (
                (REPO_ROOT / "tx-pool/src/tests/model/state.rs").read_text()
                + (REPO_ROOT / "tx-pool/src/tests/model/boundaries.rs").read_text()
                + (REPO_ROOT / "tx-pool/src/tests/model/properties.rs").read_text()
            )
            production = (
                (REPO_ROOT / "tx-pool/src/authority/runtime.rs").read_text()
                + (REPO_ROOT / "tx-pool/src/authority/plan/membership.rs").read_text()
                + (REPO_ROOT / "tx-pool/src/authority/plan/membership/rbf.rs").read_text()
                + (REPO_ROOT / "tx-pool/src/authority/resolver.rs").read_text()
            )
            return current_candidate_gap_errors(
                edge_id,
                model,
                production,
                (REPO_ROOT / "tx-pool/src/authority/tests/refinement.rs").read_text(),
            )
        if edge_id == "PR-DEPENDENCY-SCAN-NONCENSORSHIP":
            model = (
                (REPO_ROOT / "tx-pool/src/tests/model/two_phase.rs").read_text()
                + (REPO_ROOT / "tx-pool/src/tests/model/properties.rs").read_text()
            )
            return current_candidate_gap_errors(
                edge_id,
                model,
                (REPO_ROOT / "tx-pool/src/authority/template.rs").read_text(),
                (REPO_ROOT / "tx-pool/src/authority/tests/template.rs").read_text(),
            )
        if edge_id == "PR-SHUTDOWN-EFFECT-DRAIN":
            model = (
                (REPO_ROOT / "tx-pool/src/tests/model/protocol.rs").read_text()
                + (REPO_ROOT / "tx-pool/src/tests/model/properties.rs").read_text()
                + (REPO_ROOT / "tx-pool/formal/PermitEffect.tla").read_text()
            )
            return current_candidate_gap_errors(
                edge_id,
                model,
                (REPO_ROOT / "tx-pool/src/authority/compute_coordinator.rs").read_text(),
                (REPO_ROOT / "tx-pool/src/authority/tests/compute_coordinator.rs").read_text()
                + (
                    REPO_ROOT
                    / "tx-pool/src/authority/tests/support/compute_coordinator.rs"
                ).read_text(),
            )
        return []
    except OSError as error:
        return [f"cannot inspect current-candidate refinement {edge_id}: {error}"]
REQUIRED_DYNAMIC_COMPOSITIONS = {
    "DR-PROPOSAL-REORG-TEMPLATE-SERVICE": {
        "axis_ids": {
            "SA-PROPOSAL-WINDOW-TWO-STEP",
            "SA-CHAIN-REORG-RECOVERY",
            "SA-RESOURCE-ADMISSION-EVICTION-EXPIRY",
            "SA-TEMPLATE-SELECTION-UNCLES",
            "SA-LIVENESS-FAIRNESS-PROGRESS",
        },
        "ordered_stages": [
            "primitive_main_uncle_history",
            "reorg_recovery",
            "proposal_view",
            "sparse_atomic_apply",
            "sealed_status_and_eviction_projection",
            "mandatory_top_level_proposal_vs_optional_uncle",
            "dynamic_causal_package_packing",
            "complete_captured_dependency_edge_scan",
            "bounded_conditional_cycle_selection",
            "causal_template_inclusion",
            "post_stability_progress",
        ],
        "premise_ids": {
            "P-SEALED-PROPOSAL-EVICTION-PROJECTION",
            "P-SEALED-TEMPLATE-SERVICE-PREMISE",
            "P-LOCAL-CURRENT-TEMPLATE-OFFER",
            "P-CANONICAL-WINDOW-SERVICE",
            "C-BOUNDED-CANONICAL-OUTAGE",
        },
        "counterexample_ids": {
            "CE-FREE-EVICTION-STATUS",
            "CE-REORG-MIXED-CUT",
            "CE-OPTIONAL-UNCLE-CENSORSHIP",
            "CE-RAW-CAPACITY-PROXY",
            "CE-NONRETAINED-PROPOSAL-PREFIX",
            "CE-STATUSLESS-CURRENT-PACK",
            "CE-PROPOSAL-PREFIX-RESIDENCE",
            "CE-UNSEALED-UNCLE-COMPOSITION",
            "CE-PROPOSAL-ONLY-TEMPLATE-CAPACITY",
            "CE-CONDITIONAL-TEMPLATE-CYCLE-LIVENESS",
            "CE-QUALITATIVE-SERVICE-PHASE-MISS",
        },
        "status": PHASE4_CLOSED_MODEL_RELATION_STATUS,
    },
    "DR-DEPENDENCY-RESOLUTION-SETTLEMENT": {
        "axis_ids": {
            "SA-IDENTITY-EVIDENCE",
            "SA-OWNER-CAPABILITY-ATOMICITY",
            "SA-DEPENDENCY-CONDITIONAL-GRAPH",
            "SA-CONFLICT-RBF-HISTORY",
            "SA-RESOURCE-ADMISSION-EVICTION-EXPIRY",
            "SA-CHAIN-REORG-RECOVERY",
        },
        "ordered_stages": [
            "primitive_transaction_dependency_declarations",
            "sealed_resolution_group_expansion",
            "canonical_expanded_dependency_footprint",
            "checkout_baseline_and_linear_capability",
            "atomic_dependency_event_and_owner_loss",
            "baseline_first_settlement_classification",
            "causal_reader_spender_and_wake_projection",
        ],
        "premise_ids": {
            "P-DECLARED-RESOLVED-DEPENDENCY-QUOTIENT",
            "P-ACTIVE-DEPENDENCY-CUT-SETTLEMENT",
            "P-CELL-ONLY-MISSING-FRONTIER",
            "P-ATOMIC-DEPENDENCY-EVENT-COORDINATE",
        },
        "counterexample_ids": {
            "CE-FREE-DEP-GROUP-MEMBER",
            "CE-INPUT-READ-ROLE-ALIAS",
            "CE-DEPENDENCY-EVENT-REVOKES-CAPABILITY",
            "CE-MISSING-HEADER-WAIT",
            "CE-APPLY-COORDINATE-OMISSION",
        },
        "status": PHASE4_CLOSED_MODEL_RELATION_STATUS,
    },
    "DR-COST-RECEIPT-OBSERVATION": {
        "axis_ids": {
            "SA-IDENTITY-EVIDENCE",
            "SA-CONTEXTUAL-ELIGIBILITY",
            "SA-CONFLICT-RBF-HISTORY",
            "SA-RESOURCE-ADMISSION-EVICTION-EXPIRY",
            "SA-SCHEDULING-COMPUTE-CONCURRENCY",
        },
        "ordered_stages": [
            "primitive_transaction_payload",
            "sealed_resolution_fee_and_location_dependent_resident_charge",
            "sealed_verification_cycles",
            "derived_block_serialized_size",
            "resolved_evidence_verify_class",
            "ready_rbf_and_eviction_observations",
            "retained_payload_and_effect_charge",
        ],
        "premise_ids": {
            "P-COST-COORDINATE-OWNERSHIP",
            "P-DERIVED-BLOCK-SERIALIZED-SIZE",
            "P-COST-OBSERVATION-SEPARATION",
            "P-VERIFY-CLASS-THRESHOLD",
        },
        "counterexample_ids": {
            "CE-PAYLOAD-SERIALIZED-COST-ALIAS",
            "CE-FREE-VERIFY-CLASS",
            "CE-LOCATION-REFRESH-METRIC-CARRYOVER",
        },
        "status": PHASE4_CLOSED_MODEL_RELATION_STATUS,
    },
    "DR-AUTHORITY-LIFECYCLE-OBSERVATION": {
        "axis_ids": {
            "SA-IDENTITY-EVIDENCE",
            "SA-NONCONTEXTUAL-VALIDITY",
            "SA-CONTEXTUAL-ELIGIBILITY",
            "SA-INGRESS-SOURCE-DUPLICATE",
            "SA-OWNER-CAPABILITY-ATOMICITY",
            "SA-CONFLICT-RBF-HISTORY",
            "SA-RESOURCE-ADMISSION-EVICTION-EXPIRY",
            "SA-SCHEDULING-COMPUTE-CONCURRENCY",
            "SA-EFFECTS-QUERY-RELAY",
            "SA-CONFIG-PERSISTENCE-API-OPERATIONS",
            "SA-TASK-FAULT-SHUTDOWN",
            "SA-LIVENESS-FAIRNESS-PROGRESS",
        },
        "ordered_stages": [
            "validated_configuration_and_lifecycle_start",
            "primitive_ingress_identity_and_source",
            "noncontextual_validation",
            "unique_direct_or_retained_ownership",
            "sealed_contextual_resolution_and_verification",
            "baseline_first_settlement",
            "atomic_membership_RBF_resource_apply",
            "post_commit_effect_query_and_relay",
            "capability_drain_and_task_join",
            "persistence_and_restart_observation",
        ],
        "premise_ids": {
            "P-LIFECYCLE-CONFIGURATION-INGRESS",
            "P-LIFECYCLE-IDENTITY-CONTEXT-SETTLEMENT",
            "P-LIFECYCLE-ATOMIC-MEMBERSHIP-PUBLICATION",
            "P-LIFECYCLE-TASK-SHUTDOWN-RECOVERY",
        },
        "counterexample_ids": {
            "CE-DUPLICATE-ROUTE-SECOND-OWNER",
            "CE-STALE-COMPLETION-CHANGES-OWNER",
            "CE-FAILED-RBF-PARTIAL-PUBLICATION",
            "CE-ENDPOINT-VETOES-COMMIT",
            "CE-SHUTDOWN-PERSISTS-LIVE-CAPABILITY",
        },
        "status": (
            "proved_model_composition_current_candidate_cut_pending_release_boundary_adjudication"
        ),
    },
}
ALLOWED_DYNAMIC_PREMISE_KINDS = {
    "proved_local",
    "typed_environment",
    "proved_corollary",
}
ALLOWED_DIFFERENCE_DISPOSITIONS = {
    "preserve",
    "correct_owned_defect",
    "intentional_delta_with_owner",
}


def canonical_sha256(value: object) -> str:
    encoded = json.dumps(
        value, sort_keys=True, separators=(",", ":"), ensure_ascii=True
    ).encode()
    return hashlib.sha256(encoded).hexdigest()


def raw_sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def optimization_contract_semantic_sha256(path: Path) -> str:
    """Hash Phase-4 semantic inputs without hashing certificate descendants."""

    contract = json.loads(path.read_text())
    missing = [
        field for field in OPTIMIZATION_CONTRACT_SEMANTIC_FIELDS if field not in contract
    ]
    if missing:
        raise ValueError(f"optimization contract semantic fields are absent: {missing}")
    return canonical_sha256(
        {field: contract[field] for field in OPTIMIZATION_CONTRACT_SEMANTIC_FIELDS}
    )


def line_set_sha256(lines: list[str]) -> str:
    return hashlib.sha256("".join(f"{line}\n" for line in sorted(lines)).encode()).hexdigest()


def content_set_sha256(paths: list[Path]) -> str:
    rows = [
        f"{path.relative_to(REPO_ROOT).as_posix()}\0{raw_sha256(path)}"
        for path in paths
    ]
    return line_set_sha256(rows)


def source_set_paths(set_id: str) -> list[Path]:
    patterns = REQUIRED_SOURCE_SET_PATTERNS[set_id]
    if patterns == ["@integration-impact.groups"]:
        impact = json.loads((REPO_ROOT / "tx-pool/integration-impact.json").read_text())
        paths = [REPO_ROOT / path for path in impact["groups"]]
    else:
        paths = []
        for pattern in patterns:
            paths.extend(REPO_ROOT.glob(pattern))
    return sorted(
        {
            path.resolve()
            for path in paths
            if path.is_file()
            and (
                set_id != "production"
                or "tests" not in path.relative_to(REPO_ROOT).parts[:-1]
            )
        }
    )


def type_body(source: str, symbol: str) -> str | None:
    match = re.search(
        rf"pub\((?:super|crate)\)\s+(?:struct|enum)\s+{re.escape(symbol)}\b[^{{]*{{",
        source,
    )
    if match is None:
        return None
    start = source.find("{", match.start())
    depth = 0
    for index in range(start, len(source)):
        if source[index] == "{":
            depth += 1
        elif source[index] == "}":
            depth -= 1
            if depth == 0:
                return source[start + 1 : index]
    return None


def top_level_members(body: str, kind: str) -> list[str]:
    body = re.sub(r"/\*.*?\*/", "", body, flags=re.DOTALL)
    body = re.sub(r"//[^\n]*", "", body)
    members: list[str] = []
    depth = 0
    item = ""
    for char in body:
        if char in "({[<":
            depth += 1
        elif char in ")}]>":
            depth -= 1
        if char == "," and depth == 0:
            token = item.strip()
            item = ""
            if token:
                match = re.match(
                    r"(?:pub\((?:super|crate)\)\s+)?([A-Za-z_][A-Za-z0-9_]*)",
                    token,
                )
                if match is not None:
                    members.append(match.group(1))
        else:
            item += char
    token = item.strip()
    if token:
        match = re.match(
            r"(?:pub\((?:super|crate)\)\s+)?([A-Za-z_][A-Za-z0-9_]*)", token
        )
        if match is not None:
            members.append(match.group(1))
    if kind == "struct":
        return [member for member in members if member != "where"]
    return members


def discovered_input_carriers() -> list[str]:
    carriers: list[str] = []
    pattern = re.compile(
        r"pub\((super|crate)\)\s+(struct|enum)\s+([A-Za-z0-9_]+)"
    )
    for path in sorted((REPO_ROOT / "tx-pool/src/tests/model").glob("*.rs")):
        for _, _, name in pattern.findall(path.read_text()):
            if name.endswith(("Input", "Inputs", "Event", "Command")):
                carriers.append(f"{path.relative_to(REPO_ROOT).as_posix()}::{name}")
    return sorted(carriers)


def validate_source_identities(census: dict) -> list[str]:
    errors: list[str] = []
    identities = census.get("source_identities")
    if not isinstance(identities, list):
        return ["source identities must be a list"]
    seen: set[str] = set()
    for identity in identities:
        if not isinstance(identity, dict):
            errors.append(f"invalid source identity {identity!r}")
            continue
        source_id = identity.get("id")
        expected_fields = {"id", "path", "role", "sha256"}
        if source_id == "NS-OPTIMIZATION-CONTRACT":
            expected_fields.update({"hash_mode", "semantic_fields"})
        if set(identity) != expected_fields:
            errors.append(f"invalid source identity {identity!r}")
            continue
        if not isinstance(source_id, str) or source_id in seen:
            errors.append(f"invalid or duplicate source ID {source_id!r}")
            continue
        seen.add(source_id)
        path = REPO_ROOT / identity["path"]
        if not path.is_file():
            errors.append(f"source {source_id} path is absent")
        elif source_id == "NS-OPTIMIZATION-CONTRACT":
            if identity.get("hash_mode") != "canonical_json_field_projection":
                errors.append("optimization contract source hash mode differs")
            if identity.get("semantic_fields") != OPTIMIZATION_CONTRACT_SEMANTIC_FIELDS:
                errors.append("optimization contract semantic field universe differs")
            try:
                expected_hash = optimization_contract_semantic_sha256(path)
            except (OSError, ValueError, json.JSONDecodeError) as error:
                errors.append(f"cannot hash optimization contract semantics: {error}")
            else:
                if identity["sha256"] != expected_hash:
                    errors.append(f"source {source_id} hash differs")
        elif identity["sha256"] != raw_sha256(path):
            errors.append(f"source {source_id} hash differs")
        if not isinstance(identity["role"], str) or not identity["role"]:
            errors.append(f"source {source_id} has no role")
        expected_role = REQUIRED_EPISTEMIC_SOURCE_ROLES.get(source_id)
        if expected_role is not None and identity.get("role") != expected_role:
            errors.append(f"source {source_id} epistemic role differs")
    if seen != REQUIRED_SOURCE_IDS:
        errors.append("source identity universe differs")
    return errors


def validate_source_sets(census: dict) -> list[str]:
    errors: list[str] = []
    source_sets = census.get("source_sets")
    if not isinstance(source_sets, dict) or set(source_sets) != set(
        REQUIRED_SOURCE_SET_PATTERNS
    ):
        return ["source-set universe differs"]
    for set_id, expected_patterns in REQUIRED_SOURCE_SET_PATTERNS.items():
        value = source_sets[set_id]
        if not isinstance(value, dict) or set(value) != {
            "patterns",
            "path_count",
            "path_set_sha256",
            "content_set_sha256",
        }:
            errors.append(f"source set {set_id} fields differ")
            continue
        if value["patterns"] != expected_patterns:
            errors.append(f"source set {set_id} patterns differ")
        paths = source_set_paths(set_id)
        relative = [path.relative_to(REPO_ROOT).as_posix() for path in paths]
        if value["path_count"] != len(paths):
            errors.append(f"source set {set_id} path count differs")
        if value["path_set_sha256"] != line_set_sha256(relative):
            errors.append(f"source set {set_id} path hash differs")
        if value["content_set_sha256"] != content_set_sha256(paths):
            errors.append(f"source set {set_id} content hash differs")
    return errors


def validate_axes(census: dict) -> tuple[list[str], set[str]]:
    errors: list[str] = []
    axes = census.get("semantic_axes")
    if not isinstance(axes, list):
        return ["semantic axes must be a list"], set()
    seen: set[str] = set()
    source_ids = {
        identity.get("id")
        for identity in census.get("source_identities", [])
        if isinstance(identity, dict)
    }
    surface_ids = {
        surface.get("id")
        for surface in census.get("bottom_up_surfaces", [])
        if isinstance(surface, dict)
    }
    required_fields = {
        "id",
        "rule",
        "falsifier",
        "observation",
        "producer",
        "model_relation",
        "evidence",
        "cost",
        "bottom_up_surface_ids",
    }
    for axis in axes:
        if not isinstance(axis, dict) or set(axis) != required_fields:
            errors.append(f"invalid semantic axis fields {axis!r}")
            continue
        axis_id = axis["id"]
        if not isinstance(axis_id, str) or axis_id in seen:
            errors.append(f"invalid or duplicate semantic axis ID {axis_id!r}")
            continue
        seen.add(axis_id)
        for field in ("falsifier", "observation", "producer", "model_relation", "evidence", "cost"):
            if not isinstance(axis[field], dict) or not axis[field]:
                errors.append(f"axis {axis_id} has invalid {field}")
        rule = axis["rule"]
        if not isinstance(rule, dict) or set(rule) != {"predicate", "source_refs"}:
            errors.append(f"axis {axis_id} rule fields differ")
        else:
            refs = rule["source_refs"]
            if (
                not isinstance(refs, list)
                or not refs
                or not all(ref in source_ids for ref in refs)
            ):
                errors.append(f"axis {axis_id} has invalid rule sources")
            if not isinstance(rule["predicate"], str) or not rule["predicate"]:
                errors.append(f"axis {axis_id} has no rule predicate")
        refs = axis["bottom_up_surface_ids"]
        if (
            not isinstance(refs, list)
            or not refs
            or len(refs) != len(set(refs))
            or not set(refs).issubset(surface_ids)
        ):
            errors.append(f"axis {axis_id} bottom-up surface references differ")
        for key in ("trace", "distinguishing_observation"):
            if not isinstance(axis["falsifier"].get(key), str) or not axis["falsifier"][key]:
                errors.append(f"axis {axis_id} falsifier lacks {key}")
        if not isinstance(axis["observation"].get("boundary_vector"), list) or not axis["observation"]["boundary_vector"]:
            errors.append(f"axis {axis_id} has no boundary observation vector")
        if axis["producer"].get("kind") not in ALLOWED_PRODUCER_KINDS:
            errors.append(f"axis {axis_id} producer kind differs")
        if not isinstance(axis["producer"].get("ref"), str) or not axis["producer"]["ref"]:
            errors.append(f"axis {axis_id} producer reference is absent")
        relation = axis["model_relation"]
        if set(relation) != {"status", "owner", "dynamic_composition_refs"}:
            errors.append(f"axis {axis_id} model relation fields differ")
        expected_relation = REQUIRED_PHASE4_MODEL_RELATIONS.get(axis_id)
        if expected_relation is not None and (
            relation.get("status") != expected_relation["status"]
            or relation.get("owner") != expected_relation["owner"]
        ):
            errors.append(f"axis {axis_id} model relation phase-exit status differs")
        if not isinstance(axis["evidence"].get("behavior_ids"), list) or not axis["evidence"]["behavior_ids"]:
            errors.append(f"axis {axis_id} has no behavior evidence")
        if not isinstance(axis["evidence"].get("integration_specs"), list) or not axis["evidence"]["integration_specs"]:
            errors.append(f"axis {axis_id} has no integration evidence")
        if not isinstance(axis["cost"].get("objective_coordinates"), list) or not axis["cost"]["objective_coordinates"]:
            errors.append(f"axis {axis_id} has no cost coordinates")
        if not isinstance(axis["cost"].get("bound"), str) or not axis["cost"]["bound"]:
            errors.append(f"axis {axis_id} has no cost bound")
    if seen != REQUIRED_AXIS_IDS:
        errors.append("semantic axis universe differs")
    return errors, seen


def derive_semantic_grains(
    contract: dict, registry: dict
) -> tuple[list[dict], list[str]]:
    """Project exact axis/behavior grains and behavior-scoped role relations."""

    errors: list[str] = []
    quotient = contract.get("optimality_protocol", {}).get(
        "observational_quotient", {}
    )
    partition = quotient.get("semantic_partition", {})
    semantic_axis_map = partition.get("semantic_axis_to_normal_form_axis")
    if not isinstance(semantic_axis_map, dict) or set(semantic_axis_map) != REQUIRED_AXIS_IDS:
        return [], ["semantic grain authority axis universe differs"]
    bindings = contract.get("refinement_inventory", {}).get("semantic_bindings")
    production_roots = contract.get("refinement_inventory", {}).get(
        "production_roots"
    )
    model_roots = contract.get("refinement_inventory", {}).get("model_roots")
    if not isinstance(bindings, dict) or not bindings:
        return [], ["semantic grain authority has no semantic bindings"]
    if not isinstance(production_roots, dict) or not isinstance(model_roots, dict):
        return [], ["semantic grain authority has no production/model roots"]

    for side, roots in (("production", production_roots), ("model", model_roots)):
        root_paths = sorted(
            {
                REPO_ROOT / coordinate.rsplit("::", 1)[0]
                for coordinate in roots
                if isinstance(coordinate, str) and "::" in coordinate
            }
        )
        declarations, parser_errors = check_model_refinement.declarations(root_paths)
        _, root_errors = check_model_refinement.validate_roots(
            roots, declarations, f"{side}_roots"
        )
        errors.extend(parser_errors)
        errors.extend(root_errors)
    production_root_roles = set(production_roots.values())
    model_root_roles = set(model_roots.values())

    behaviors = registry.get("behaviors")
    unit_evidence = registry.get("unit_evidence")
    workspace_evidence = registry.get("workspace_evidence")
    integration_evidence = registry.get("integration_evidence")
    if not all(
        isinstance(value, list)
        for value in (behaviors, unit_evidence, workspace_evidence, integration_evidence)
    ):
        return [], ["semantic grain behavior/evidence registry is invalid"]

    rows: list[dict] = []
    seen_behaviors: set[str] = set()
    seen_pairs: set[tuple[str, str]] = set()
    for behavior in behaviors:
        if not isinstance(behavior, dict):
            errors.append("semantic grain authority found an invalid behavior")
            continue
        behavior_id = behavior.get("id")
        axis_ids = behavior.get("semantic_axis_ids")
        if not isinstance(behavior_id, str) or behavior_id in seen_behaviors:
            errors.append(f"semantic grain behavior ID differs: {behavior_id!r}")
            continue
        seen_behaviors.add(behavior_id)
        if (
            not isinstance(axis_ids, list)
            or not axis_ids
            or axis_ids != sorted(axis_ids)
            or len(axis_ids) != len(set(axis_ids))
            or not set(axis_ids).issubset(REQUIRED_AXIS_IDS)
        ):
            errors.append(f"semantic grain {behavior_id} axis projection differs")
            continue
        binding_ids = sorted(
            binding_id
            for binding_id, binding in bindings.items()
            if isinstance(binding, dict)
            and behavior_id in binding.get("behavior_ids", [])
        )
        if not binding_ids:
            errors.append(f"semantic grain {behavior_id} has no semantic binding")
            continue
        production_roles = sorted(
            {
                role
                for binding_id in binding_ids
                for role in bindings[binding_id].get("production_roles", [])
                if isinstance(role, str)
            }
        )
        model_roles = sorted(
            {
                role
                for binding_id in binding_ids
                for role in bindings[binding_id].get("model_roles", [])
                if isinstance(role, str)
            }
        )
        if not production_roles or not set(production_roles).issubset(
            production_root_roles
        ):
            errors.append(f"semantic grain {behavior_id} production role closure differs")
        if not model_roles or not set(model_roles).issubset(model_root_roles):
            errors.append(f"semantic grain {behavior_id} model role closure differs")

        model_tests = sorted(
            row["test"]
            for row in unit_evidence
            if isinstance(row, dict)
            and row.get("behavior_id") == behavior_id
            and isinstance(row.get("test"), str)
            and row["test"].startswith("mathematical_model::")
        )
        production_tests = sorted(
            row["test"]
            for row in unit_evidence
            if isinstance(row, dict)
            and row.get("behavior_id") == behavior_id
            and isinstance(row.get("test"), str)
            and not row["test"].startswith("mathematical_model::")
        )
        workspace_tests = sorted(
            row["test"]
            for row in workspace_evidence
            if isinstance(row, dict)
            and behavior_id in row.get("behavior_ids", [])
            and row.get("evidence_kind") != "counterexample"
            and isinstance(row.get("test"), str)
        )
        integration_anchors = sorted(
            row["anchor"]
            for row in integration_evidence
            if isinstance(row, dict)
            and behavior_id in row.get("behavior_ids", [])
            and isinstance(row.get("anchor"), str)
        )
        if not model_tests:
            errors.append(f"semantic grain {behavior_id} has no model falsifier evidence")
        if not production_tests and not workspace_tests and not integration_anchors:
            errors.append(
                f"semantic grain {behavior_id} has no production observation evidence"
            )
        owners = behavior.get("implementation_owners")
        if not isinstance(owners, list) or not owners:
            errors.append(f"semantic grain {behavior_id} has no production owner")
            owners = []
        evidence_sha256 = canonical_sha256(
            {
                "rule": behavior.get("required_behavior"),
                "falsifier": behavior.get("hostile_case"),
                "cost": behavior.get("performance_bound"),
                "owners": owners,
                "semantic_binding_ids": binding_ids,
                "production_roles": production_roles,
                "model_roles": model_roles,
                "model_tests": model_tests,
                "production_tests": production_tests,
                "workspace_tests": workspace_tests,
                "integration_anchors": integration_anchors,
            }
        )
        behavior_role_relation_sha256 = canonical_sha256(
            {
                "behavior_id": behavior_id,
                "semantic_binding_ids": binding_ids,
                "production_roles": production_roles,
                "model_roles": model_roles,
            }
        )
        for axis_id in axis_ids:
            pair = (axis_id, behavior_id)
            if pair in seen_pairs:
                errors.append(f"duplicate semantic grain pair {axis_id}::{behavior_id}")
                continue
            seen_pairs.add(pair)
            rows.append(
                {
                    "id": f"{axis_id}::{behavior_id}",
                    "axis_id": axis_id,
                    "behavior_id": behavior_id,
                    "semantic_binding_ids": binding_ids,
                    "production_role_refs": production_roles,
                    "model_role_refs": model_roles,
                    "behavior_role_relation_sha256": behavior_role_relation_sha256,
                    "evidence_sha256": evidence_sha256,
                }
            )
    rows.sort(key=lambda row: row["id"])
    covered_axes = {row["axis_id"] for row in rows}
    if covered_axes != REQUIRED_AXIS_IDS:
        errors.append(
            "semantic grain axis coverage differs: "
            f"missing={sorted(REQUIRED_AXIS_IDS - covered_axes)}"
        )
    return rows, errors


def validate_semantic_grains(census: dict, axis_ids: set[str]) -> tuple[list[str], list[str]]:
    errors: list[str] = []
    try:
        contract = json.loads(CONTRACT_PATH.read_text())
        registry = json.loads(BEHAVIOR_REGISTRY_PATH.read_text())
    except (OSError, json.JSONDecodeError) as error:
        return [f"cannot derive semantic grains: {error}"], []
    expected, authority_errors = derive_semantic_grains(contract, registry)
    errors.extend(authority_errors)
    observed = census.get("semantic_grains")
    if observed != expected:
        errors.append("semantic grain relation projection differs")
    relation_by_axis: dict[str, set[str]] = {axis_id: set() for axis_id in axis_ids}
    if isinstance(observed, list):
        for row in observed:
            if not isinstance(row, dict) or set(row) != {
                "id",
                "axis_id",
                "behavior_id",
                "semantic_binding_ids",
                "production_role_refs",
                "model_role_refs",
                "behavior_role_relation_sha256",
                "evidence_sha256",
            }:
                errors.append("semantic grain row fields differ")
                continue
            axis_id = row.get("axis_id")
            behavior_id = row.get("behavior_id")
            if axis_id in relation_by_axis and isinstance(behavior_id, str):
                relation_by_axis[axis_id].add(behavior_id)
    for axis in census.get("semantic_axes", []):
        if not isinstance(axis, dict) or axis.get("id") not in relation_by_axis:
            continue
        expected_behaviors = set(axis.get("evidence", {}).get("behavior_ids", []))
        if relation_by_axis[axis["id"]] != expected_behaviors:
            errors.append(f"axis {axis['id']} semantic grain projection differs")
    return errors, [row["id"] for row in expected]


def resolve_contract_ref(reference: str) -> object | None:
    prefix = "tx-pool/architecture-contract.json#"
    if not reference.startswith(prefix):
        return None
    try:
        value: object = json.loads(
            (REPO_ROOT / "tx-pool/architecture-contract.json").read_text()
        )
        pointer = reference.removeprefix(prefix)
        if not pointer.startswith("/"):
            return None
        for token in pointer[1:].split("/"):
            token = token.replace("~1", "/").replace("~0", "~")
            if not isinstance(value, dict) or token not in value:
                return None
            value = value[token]
        return value
    except (OSError, json.JSONDecodeError):
        return None


def template_service_premise_errors(source: str) -> list[str]:
    """Bind liveness to one exact local-template compilation, never a raw proxy."""

    errors: list[str] = []
    expected_structs = {
        "TemplateServiceCandidate": [
            "proposal",
            "status",
            "own",
            "causal_parents",
            "conditional_predecessors",
            "dependency_edges",
            "eviction_order",
            "arrival",
            "identity",
        ],
        "CurrentTemplateComposition": [
            "max_block_proposals",
            "proposal_id_bytes",
            "candidate_uncles",
            "base_bytes",
            "max_block_bytes",
            "max_block_cycles",
        ],
        "TemplateServiceSourceCut": [
            "candidates",
            "composition",
            "captured_dependency_edge_bound",
        ],
        "TemplateServicePremise": [
            "source",
            "cohort",
            "packed_source_indices",
            "retained_source_indices",
            "shed_source_indices",
        ],
    }
    for name, members in expected_structs.items():
        body = type_body(source, name)
        if body is None:
            errors.append(f"template service premise lacks {name}")
        elif top_level_members(body, "struct") != members:
            errors.append(f"template service premise {name} coordinate universe differs")

    if "PositiveTemplateCapacity" in source:
        errors.append("raw positive template capacity proxy was retained")
    liveness_body = type_body(source, "TwoPhaseLiveness")
    if liveness_body is None or not re.search(
        r"\bpremise\s*:\s*TemplateServicePremise\b", liveness_body
    ):
        errors.append("two-phase theorem is not sealed to TemplateServicePremise")
    for required in (
        "pub(super) struct StableAcceptedCohort",
        "pub(super) fn from_authority(authority: &Omega)",
        "fn compile_template_service_cut(",
        "TemplateServicePremise::compile",
        "source.captured_dependency_edge_bound",
        "source.composition.max_block_proposals",
        "pending_source_indices.len(),\n        source.composition.max_block_proposals,",
        "source.composition.candidate_uncles.iter().cloned()",
        "if initial.retained_source_indices.is_empty()",
        "retained_source_indices: initial.retained_source_indices,",
        "proposal_source_indices: Vec<usize>",
        "compatible_uncles: Vec<CandidateUncleInput>",
        "AcceptedStatus::Pending => ProposalWindowPosition::Outside",
        "current_proposal_source_indices",
        "let proposals = compilation\n            .proposal_source_indices",
        "let residence_span = usize::from(",
        "if proposal_span > residence_span",
        "TemplateServicePremiseError::NoProposalCapacity",
        "TemplateServicePremiseError::NoCommitCapacity",
        "ConditionalTemplateCycle",
        "evidence.conditional_reads()",
        "accepted_order_refinement(&order_inputs)",
        "template_packing_refinement(",
        "complete_dependency_scan_refinement(",
        "conditional_template_selection_refinement",
        "CONDITIONAL_CYCLE_ROUND_BOUND",
        "CurrentProposalOfferWithoutOptionalUncles",
        "CurrentProposalOfferWithCompatibleUncles",
        ".realizes_proposal_offer()",
        ".realizes_commit_offer()",
        "current_template_capacity_refinement",
    ):
        if required not in source:
            errors.append(f"two-phase theorem applicability lacks {required}")
    return errors


def conditional_cycle_refinement_errors(
    model_source: str, production_source: str, production_test_source: str
) -> list[str]:
    """Bind one production cycle compiler to the independent model relation."""

    errors: list[str] = []
    if not re.search(
        r"CONDITIONAL_CYCLE_ROUND_BOUND\s*:\s*usize\s*=\s*64\s*;",
        model_source,
    ):
        errors.append("model conditional-cycle round bound differs")
    if not re.search(
        r"MAX_CONDITIONAL_CYCLE_ROUNDS\s*:\s*usize\s*=\s*64\s*;",
        production_source,
    ):
        errors.append("production conditional-cycle round bound differs")
    for token in (
        "fn conditional_template_selection_refinement",
        "fn conditional_cyclic_components",
        "fn drop_model_causal_descendants",
    ):
        if token not in model_source:
            errors.append(f"conditional-cycle model lacks {token}")
    for token in (
        "fn order_conditionally_safe",
        "fn cycle_representative",
        "fn strongly_connected_active",
        "fn drop_causal_descendants",
    ):
        if token not in production_source:
            errors.append(f"production conditional-cycle compiler lacks {token}")
    for token in (
        "fn model_conditional_selection",
        "fn uak_template_sheds_conditional_cycles_deterministically",
        "fn uak_template_cycle_shedding_preserves_descendant_aware_strength",
        "assert_eq!(selected, model_selected)",
    ):
        if token not in production_test_source:
            errors.append(f"conditional-cycle refinement evidence lacks {token}")
    return errors


def validate_conditional_cycle_refinement_sources() -> list[str]:
    try:
        return conditional_cycle_refinement_errors(
            (REPO_ROOT / "tx-pool/src/tests/model/two_phase.rs").read_text(),
            (REPO_ROOT / "tx-pool/src/authority/template.rs").read_text(),
            (REPO_ROOT / "tx-pool/src/authority/tests/template.rs").read_text(),
        )
    except OSError as error:
        return [f"cannot inspect conditional-cycle refinement: {error}"]


def template_packing_refinement_errors(
    model_source: str,
    packing_source: str,
    template_source: str,
    packing_test_source: str,
    template_test_source: str,
) -> list[str]:
    """Bind package packing and the complete dependency scan to independent relations."""

    errors: list[str] = []
    for model_name, production_name, expected in (
        ("TEMPLATE_PACKING_FAILURE_BOUND", "MAX_CONSECUTIVE_PACKING_FAILURES", 4_000),
        (
            "TEMPLATE_DESCENDANT_CACHE_MEMBER_BOUND",
            "DESCENDANTS_CACHE_MEMBER_BUDGET",
            200_000,
        ),
    ):
        if not re.search(rf"{model_name}\s*:\s*usize\s*=\s*{expected:_}\s*;", model_source):
            errors.append(f"model template bound {model_name} differs")
        production_source = packing_source
        if not re.search(
            rf"{production_name}\s*:\s*usize\s*=\s*{expected:_}\s*;",
            production_source,
        ):
            errors.append(f"production template bound {production_name} differs")
    for token in (
        "fn template_packing_refinement",
        "fn complete_dependency_scan_refinement",
        "struct TemplatePackingInput",
        "struct DependencyScanInput",
        "struct DependencyScanObservation",
    ):
        if token not in model_source:
            errors.append(f"template packing model lacks {token}")
    for token in (
        "fn pack_transactions_with_failure_bound",
        "struct PackageAggregate",
        "struct PackageOrderKey",
    ):
        if token not in packing_source:
            errors.append(f"production package packer lacks {token}")
    for token in (
        "fn order_conditionally_safe",
        "dependency_edge_bound",
        "footprint().dependencies().len()",
        "if dependency_count > self.dependency_edge_bound",
    ):
        if token not in template_source:
            errors.append(f"production dependency compiler lacks {token}")
    if "retain_with_dependency_budget" in template_source:
        errors.append("production dependency compiler reintroduced semantic budget truncation")
    for token in (
        "fn model_packing_matches_production",
        "fn assert_model_rejects_every_distinct_reordering",
        "production_hashes: &[RawTxHash]",
        "model_hashes.as_slice() == production_hashes",
        "fn uak_template_packer_selects_an_exact_fit_cpfp_package_parent_first",
        "fn uak_template_packer_rescores_descendants_after_shared_parent_selection",
        "fn uak_template_packer_aggregates_multi_parent_descendant_adjustments",
        "fn uak_template_packer_bounds_non_fitting_work_without_changing_the_policy",
        "if permutation != production_hashes",
        "!model_packing_matches_production(",
        "assert_model_rejects_every_distinct_reordering(",
    ):
        if token not in packing_test_source:
            errors.append(f"package packing refinement evidence lacks {token}")
    if "fn model_packed_hashes" in packing_test_source:
        errors.append("package packing model was retained as a production expected-value source")
    if packing_test_source.count("model_packing_matches_production(") != 9:
        errors.append(
            "package packing refinement does not cover all seven production "
            "observations plus one rejecting falsifier"
        )

    exact_test_start = packing_test_source.find(
        "fn uak_template_packer_selects_an_exact_fit_cpfp_package_parent_first()"
    )
    exact_test_end = packing_test_source.find("\n#[test]", exact_test_start + 1)
    exact_test = (
        packing_test_source[exact_test_start:exact_test_end]
        if exact_test_start >= 0 and exact_test_end > exact_test_start
        else ""
    )
    ordered_production_observations = (
        "let exact_hashes = packed_hashes(&exact);",
        "vec![parent.clone(), child.clone()]",
        "let one_byte_hashes = packed_hashes(&one_byte_short);",
        "vec![rival.clone(), parent.clone()]",
        "assert_eq!(one_byte_short.cycles(), 40);",
        "let one_cycle_hashes = packed_hashes(&one_cycle_short);",
        "vec![parent.clone()]",
        "assert_eq!(one_cycle_short.cycles(), 10);",
    )
    for token in ordered_production_observations:
        if exact_test.count(token) != 1:
            errors.append(f"package packing production observation lacks unique {token}")
    for hashes in ("exact_hashes", "one_byte_hashes", "one_cycle_hashes"):
        observation = exact_test.find(f"let {hashes} =")
        model = exact_test.find(
            "model_packing_matches_production(", observation + 1
        )
        if observation < 0 or model < 0 or observation >= model:
            errors.append(
                f"package packing model precedes the production observation {hashes}"
            )
    for token in (
        "fn uak_template_complete_dependency_scan_preserves_causal_and_later_independent_work",
        "fn uak_template_complete_dependency_scan_does_not_censor_independent_dependency_work",
        "complete_dependency_scan_refinement",
        "captured_edge_bound",
        "assert_eq!(selected",
    ):
        if token not in template_test_source:
            errors.append(f"complete dependency-scan refinement evidence lacks {token}")
    return errors


def validate_template_packing_refinement_sources() -> list[str]:
    try:
        return template_packing_refinement_errors(
            (REPO_ROOT / "tx-pool/src/tests/model/two_phase.rs").read_text(),
            (REPO_ROOT / "tx-pool/src/authority/packing.rs").read_text(),
            (REPO_ROOT / "tx-pool/src/authority/template.rs").read_text(),
            (REPO_ROOT / "tx-pool/src/authority/tests/packing.rs").read_text(),
            (REPO_ROOT / "tx-pool/src/authority/tests/template.rs").read_text(),
        )
    except OSError as error:
        return [f"cannot inspect template packing refinement: {error}"]


def validate_template_packing_refinement_canary() -> list[str]:
    """Reject removal of the short limits or the nontriviality falsifier."""

    try:
        model_source = (REPO_ROOT / "tx-pool/src/tests/model/two_phase.rs").read_text()
        packing_source = (REPO_ROOT / "tx-pool/src/authority/packing.rs").read_text()
        template_source = (REPO_ROOT / "tx-pool/src/authority/template.rs").read_text()
        packing_test_source = (
            REPO_ROOT / "tx-pool/src/authority/tests/packing.rs"
        ).read_text()
        template_test_source = (
            REPO_ROOT / "tx-pool/src/authority/tests/template.rs"
        ).read_text()
    except OSError as error:
        return [f"cannot inspect template packing canary: {error}"]
    start = packing_test_source.find("    let one_byte_limit =")
    end = packing_test_source.find("\n}\n\n#[test]", start)
    if start < 0 or end < 0:
        return ["template packing short-limit canary source is absent"]
    mutation = packing_test_source[:start] + packing_test_source[end:]
    if not template_packing_refinement_errors(
        model_source,
        packing_source,
        template_source,
        mutation,
        template_test_source,
    ):
        return ["template packing checker admitted removal of both short-limit observations"]
    falsifier_start = packing_test_source.find(
        "fn assert_model_rejects_every_distinct_reordering("
    )
    falsifier_end = packing_test_source.find(
        "\nfn output_transaction", falsifier_start
    )
    if falsifier_start < 0 or falsifier_end < 0:
        return ["template packing permutation falsifier source is absent"]
    without_falsifier = (
        packing_test_source[:falsifier_start]
        + packing_test_source[falsifier_end:]
    )
    if not template_packing_refinement_errors(
        model_source,
        packing_source,
        template_source,
        without_falsifier,
        template_test_source,
    ):
        return ["template packing checker admitted removal of its permutation falsifier"]
    return []


def validate_dynamic_compositions(census: dict, axis_ids: set[str]) -> list[str]:
    """Require cross-axis claims to form one mechanically bound hyperedge."""

    errors: list[str] = []
    rows = census.get("dynamic_compositions")
    if not isinstance(rows, list):
        return ["dynamic compositions must be a list"]
    seen: set[str] = set()
    row_axes: dict[str, set[str]] = {}
    required_fields = {
        "id",
        "axis_ids",
        "ordered_stages",
        "commutation_law",
        "premises",
        "counterexamples",
        "anchors",
        "status",
    }
    for row in rows:
        if not isinstance(row, dict) or set(row) != required_fields:
            errors.append(f"invalid dynamic composition {row!r}")
            continue
        row_id = row["id"]
        if not isinstance(row_id, str) or row_id in seen:
            errors.append(f"invalid or duplicate dynamic composition ID {row_id!r}")
            continue
        seen.add(row_id)
        axes = row["axis_ids"]
        if (
            not isinstance(axes, list)
            or len(axes) < 2
            or len(axes) != len(set(axes))
            or not set(axes).issubset(axis_ids)
        ):
            errors.append(f"dynamic composition {row_id} axes differ")
            axes = []
        row_axes[row_id] = set(axes)
        stages = row["ordered_stages"]
        if (
            not isinstance(stages, list)
            or not stages
            or len(stages) != len(set(stages))
            or not all(isinstance(stage, str) and stage for stage in stages)
        ):
            errors.append(f"dynamic composition {row_id} stages differ")
        if not isinstance(row["commutation_law"], str) or not row["commutation_law"]:
            errors.append(f"dynamic composition {row_id} has no commutation law")

        premises = row["premises"]
        premise_ids: set[str] = set()
        if not isinstance(premises, list) or not premises:
            errors.append(f"dynamic composition {row_id} has no premises")
            premises = []
        for premise in premises:
            if not isinstance(premise, dict) or set(premise) != {
                "id",
                "kind",
                "owner",
                "contract_ref",
                "statement",
                "falsifier",
            }:
                errors.append(f"dynamic composition {row_id} has invalid premise")
                continue
            premise_id = premise["id"]
            if not isinstance(premise_id, str) or premise_id in premise_ids:
                errors.append(f"dynamic composition {row_id} premise IDs differ")
                continue
            premise_ids.add(premise_id)
            if premise["kind"] not in ALLOWED_DYNAMIC_PREMISE_KINDS:
                errors.append(f"dynamic composition {row_id} premise kind differs")
            for field in ("owner", "statement", "falsifier"):
                if not isinstance(premise[field], str) or not premise[field]:
                    errors.append(
                        f"dynamic composition {row_id} premise {premise_id} lacks {field}"
                    )
            if resolve_contract_ref(premise["contract_ref"]) != premise["statement"]:
                errors.append(
                    f"dynamic composition {row_id} premise {premise_id} contract binding differs"
                )

        anchors = row["anchors"]
        anchor_ids: set[str] = set()
        if not isinstance(anchors, dict) or set(anchors) != {
            "production",
            "model",
            "integration",
            "residual_model",
        }:
            errors.append(f"dynamic composition {row_id} anchor groups differ")
            anchors = {}
        for group in ("production", "model", "integration", "residual_model"):
            values = anchors.get(group)
            if not isinstance(values, list) or not values:
                errors.append(f"dynamic composition {row_id} lacks {group} anchors")
                continue
            for anchor in values:
                if not isinstance(anchor, dict) or set(anchor) != {"path", "symbol"}:
                    errors.append(f"dynamic composition {row_id} has invalid {group} anchor")
                    continue
                path = REPO_ROOT / anchor["path"]
                anchor_id = f"{anchor['path']}::{anchor['symbol']}"
                if anchor_id in anchor_ids:
                    errors.append(f"dynamic composition {row_id} duplicates anchor {anchor_id}")
                anchor_ids.add(anchor_id)
                if not path.is_file() or anchor["symbol"] not in path.read_text():
                    errors.append(
                        f"dynamic composition {row_id} anchor {anchor_id} is absent"
                    )

        counterexamples = row["counterexamples"]
        counterexample_ids: set[str] = set()
        if not isinstance(counterexamples, list) or not counterexamples:
            errors.append(f"dynamic composition {row_id} has no counterexamples")
            counterexamples = []
        for counterexample in counterexamples:
            if not isinstance(counterexample, dict) or set(counterexample) != {
                "id",
                "rejected_claim",
                "observation",
                "anchor",
            }:
                errors.append(f"dynamic composition {row_id} has invalid counterexample")
                continue
            counterexample_id = counterexample["id"]
            if (
                not isinstance(counterexample_id, str)
                or counterexample_id in counterexample_ids
            ):
                errors.append(f"dynamic composition {row_id} counterexample IDs differ")
                continue
            counterexample_ids.add(counterexample_id)
            if counterexample["anchor"] not in anchor_ids:
                errors.append(
                    f"dynamic composition {row_id} counterexample {counterexample_id} anchor differs"
                )
            for field in ("rejected_claim", "observation"):
                if not isinstance(counterexample[field], str) or not counterexample[field]:
                    errors.append(
                        f"dynamic composition {row_id} counterexample {counterexample_id} lacks {field}"
                    )

        expected = REQUIRED_DYNAMIC_COMPOSITIONS.get(row_id)
        if expected is None:
            errors.append(f"unexpected dynamic composition {row_id}")
        else:
            if set(axes) != expected["axis_ids"]:
                errors.append(f"dynamic composition {row_id} required axes differ")
            if stages != expected["ordered_stages"]:
                errors.append(f"dynamic composition {row_id} required stages differ")
            if premise_ids != expected["premise_ids"]:
                errors.append(f"dynamic composition {row_id} premise universe differs")
            if counterexample_ids != expected["counterexample_ids"]:
                errors.append(
                    f"dynamic composition {row_id} counterexample universe differs"
                )
            if row["status"] != expected["status"]:
                errors.append(f"dynamic composition {row_id} status differs")

    if seen != set(REQUIRED_DYNAMIC_COMPOSITIONS):
        errors.append("dynamic composition universe differs")

    axis_projection: dict[str, set[str]] = {row_id: set() for row_id in seen}
    for axis in census.get("semantic_axes", []):
        if not isinstance(axis, dict):
            continue
        relation = axis.get("model_relation", {})
        refs = relation.get("dynamic_composition_refs", [])
        if (
            not isinstance(refs, list)
            or len(refs) != len(set(refs))
            or not set(refs).issubset(seen)
        ):
            errors.append(f"axis {axis.get('id')} dynamic composition references differ")
            continue
        if "dynamic_composition" in str(relation.get("status")) and not refs:
            errors.append(f"axis {axis.get('id')} claims an unbound dynamic composition")
        for row_id in refs:
            axis_projection[row_id].add(axis.get("id"))
    for row_id, axes in row_axes.items():
        if axis_projection.get(row_id, set()) != axes:
            errors.append(f"dynamic composition {row_id} axis projection differs")
    covered_axes = set().union(*axis_projection.values()) if axis_projection else set()
    if covered_axes != axis_ids:
        errors.append("dynamic composition axis coverage differs")

    two_phase_source = (REPO_ROOT / "tx-pool/src/tests/model/two_phase.rs").read_text()
    tla_source = (REPO_ROOT / "tx-pool/formal/ProposalLiveness.tla").read_text()
    errors.extend(template_service_premise_errors(two_phase_source))
    for forbidden in ("FairBlockLimits", "run_fair"):
        if forbidden in two_phase_source:
            errors.append(f"qualitative liveness mechanism {forbidden} was retained")
    for required in (
        "CurrentProposalOfferWithoutOptionalUncles",
        "CurrentProposalOfferWithCompatibleUncles",
        "CurrentOfferWithoutOptionalUncles",
        "CurrentOfferWithCompatibleUncles",
        ".realizes_proposal_offer()",
        ".realizes_commit_offer()",
    ):
        if required not in two_phase_source:
            errors.append(f"mandatory-offer service quotient lacks {required}")
    if "FairSpec" in tla_source:
        errors.append("qualitative FairSpec was retained as liveness evidence")
    try:
        formal_registry = json.loads(
            (REPO_ROOT / "tx-pool/formal/models.json").read_text()
        )
        proposal_modules = {
            run.get("module")
            for run in formal_registry.get("runs", [])
            if isinstance(run, dict) and "Proposal" in str(run.get("module"))
        }
        if proposal_modules != {"ProposalLiveness.tla"}:
            errors.append("proposal liveness has zero or duplicate formal specifications")
    except (OSError, json.JSONDecodeError):
        errors.append("proposal liveness formal registry is unreadable")
    return errors


def validate_surfaces(census: dict, axis_ids: set[str]) -> tuple[list[str], set[str]]:
    errors: list[str] = []
    surfaces = census.get("bottom_up_surfaces")
    if not isinstance(surfaces, list):
        return ["bottom-up surfaces must be a list"], set()
    seen: set[str] = set()
    covered: set[str] = set()
    for surface in surfaces:
        if not isinstance(surface, dict) or set(surface) != {
            "id",
            "kind",
            "path",
            "symbol",
            "axis_id",
        }:
            errors.append(f"invalid bottom-up surface {surface!r}")
            continue
        surface_id = surface["id"]
        if not isinstance(surface_id, str) or surface_id in seen:
            errors.append(f"invalid or duplicate surface ID {surface_id!r}")
            continue
        seen.add(surface_id)
        axis_id = surface["axis_id"]
        if axis_id not in axis_ids:
            errors.append(f"surface {surface_id} has unknown axis")
        else:
            covered.add(axis_id)
        path = REPO_ROOT / surface["path"]
        symbol = surface["symbol"]
        if not path.is_file():
            errors.append(f"surface {surface_id} path is absent")
        elif not isinstance(symbol, str) or symbol not in path.read_text():
            errors.append(f"surface {surface_id} symbol is absent")
        if surface["kind"] not in {
            "production",
            "public_api",
            "configuration",
            "persistence",
            "integration",
            "model",
        }:
            errors.append(f"surface {surface_id} kind differs")
    return errors, covered


def validate_production_refinement_edges(
    census: dict, axis_ids: set[str]
) -> tuple[list[str], list[str]]:
    """Keep model theorems separate from production realization evidence."""

    errors: list[str] = []
    rows = census.get("production_refinement_edges")
    if not isinstance(rows, list):
        return ["production refinement edges must be a list"], []
    seen: set[str] = set()
    open_edges: list[str] = []
    compositions = {
        row.get("id")
        for row in census.get("dynamic_compositions", [])
        if isinstance(row, dict)
    }
    for row in rows:
        if not isinstance(row, dict) or set(row) != PRODUCTION_REFINEMENT_FIELDS:
            errors.append(f"invalid production refinement edge {row!r}")
            continue
        edge_id = row["id"]
        if not isinstance(edge_id, str) or edge_id in seen:
            errors.append(f"invalid or duplicate production refinement edge {edge_id!r}")
            continue
        seen.add(edge_id)
        expected = REQUIRED_PRODUCTION_REFINEMENT_EDGES.get(edge_id)
        if expected is None:
            errors.append(f"unexpected production refinement edge {edge_id}")
            continue
        axes = row["axis_ids"]
        if (
            not isinstance(axes, list)
            or len(axes) != len(set(axes))
            or not set(axes).issubset(axis_ids)
            or set(axes) != expected["axis_ids"]
        ):
            errors.append(f"production refinement edge {edge_id} axes differ")
        composition = row["dynamic_composition_ref"]
        if (
            composition not in compositions
            or composition != expected["dynamic_composition_ref"]
        ):
            errors.append(
                f"production refinement edge {edge_id} dynamic composition differs"
            )

        anchor_groups = (
            ("model_anchors", expected["model_anchors"]),
            ("production_producers", expected["producer_anchors"]),
            ("production_consumers", expected["consumer_anchors"]),
        )
        for field, expected_anchors in anchor_groups:
            anchors = row[field]
            if not isinstance(anchors, list) or not anchors:
                errors.append(f"production refinement edge {edge_id} lacks {field}")
                continue
            observed_anchors: set[str] = set()
            for anchor in anchors:
                if not isinstance(anchor, dict) or set(anchor) != {"path", "symbol"}:
                    errors.append(
                        f"production refinement edge {edge_id} has invalid {field} anchor"
                    )
                    continue
                path = REPO_ROOT / anchor["path"]
                symbol = anchor["symbol"]
                anchor_id = f"{anchor['path']}::{symbol}"
                if anchor_id in observed_anchors:
                    errors.append(
                        f"production refinement edge {edge_id} duplicates anchor {anchor_id}"
                    )
                observed_anchors.add(anchor_id)
                if not path.is_file() or symbol not in path.read_text():
                    errors.append(
                        f"production refinement edge {edge_id} anchor {anchor_id} is absent"
                    )
            if observed_anchors != expected_anchors:
                errors.append(
                    f"production refinement edge {edge_id} {field} anchor universe differs"
                )

        for field in (
            "required_relation",
            "falsifier",
            "negative_canary",
        ):
            if row[field] != expected[field]:
                errors.append(f"production refinement edge {edge_id} {field} differs")
        status = row["status"]
        if status not in {
            OPEN_PRODUCTION_REFINEMENT_STATUS,
            PROVED_PRODUCTION_REFINEMENT_STATUS,
        }:
            errors.append(f"production refinement edge {edge_id} status differs")
        elif status == OPEN_PRODUCTION_REFINEMENT_STATUS:
            open_edges.append(edge_id)
            errors.extend(validate_current_candidate_refinement_sources(edge_id))
        elif edge_id == "PR-SCRIPT-PROOF-QUOTIENT":
            errors.extend(validate_script_proof_refinement_sources())
        else:
            errors.extend(validate_current_candidate_refinement_sources(edge_id))

    if seen != set(REQUIRED_PRODUCTION_REFINEMENT_EDGES):
        errors.append("production refinement edge universe differs")
    return errors, sorted(open_edges)


def eviction_status_provenance_errors(
    refinement_source: str, production_checker_source: str
) -> list[str]:
    """Bind eviction order to the sealed default-production proposal receipt."""

    errors: list[str] = []
    compact = "".join(refinement_source.split())
    exact_bridge = (
        "fnproduction_eviction_status_receipt(receipt:&ProposalContextReceipt)"
        "->ProposalStatusReceipt{"
        "eviction_status_witness(refinement_status(receipt.status()))}"
    )
    if exact_bridge not in compact:
        errors.append("eviction production status bridge is not the exact sealed-receipt map")
    if compact.count("production_eviction_status_receipt(") != 2 or (
        "status:production_eviction_status_receipt(&entry.proposal)" not in compact
    ):
        errors.append("eviction refinement does not consume the Accepted entry receipt exactly once")
    if "status:eviction_status_witness(refinement_status(entry.status()))" in compact:
        errors.append("eviction refinement regained a free cached-status witness")
    if production_checker_source.count("def validate_proposal_history_provenance()") != 1 or (
        production_checker_source.count("*validate_proposal_history_provenance(),") != 1
    ):
        errors.append("eviction receipt bridge lacks the executed production-history residual")
    return errors


def validate_eviction_status_provenance_sources() -> list[str]:
    try:
        refinement = (
            REPO_ROOT / "tx-pool/src/authority/tests/refinement.rs"
        ).read_text()
        production_checker = (
            REPO_ROOT / "tx-pool/scripts/check_production_contracts.py"
        ).read_text()
    except OSError as error:
        return [f"cannot inspect eviction status provenance: {error}"]
    return eviction_status_provenance_errors(refinement, production_checker)


def cost_observation_provenance_errors(
    model_state_source: str,
    model_kernel_source: str,
    scheduler_source: str,
    model_refinement_source: str,
    refinement_source: str,
    production_state_source: str,
    resolver_source: str,
    model_settlement_source: str,
) -> list[str]:
    """Bind every cost coordinate to one owner and one exact observation class."""

    errors: list[str] = []
    state = "".join(model_state_source.split())
    kernel = "".join(model_kernel_source.split())
    scheduler = "".join(scheduler_source.split())
    model_refinement = "".join(model_refinement_source.split())
    refinement = "".join(refinement_source.split())
    production_state = "".join(production_state_source.split())
    resolver = "".join(resolver_source.split())
    model_settlement = "".join(model_settlement_source.split())

    cost_body = type_body(model_state_source, "ModelTransactionCost")
    transaction_body = type_body(model_state_source, "Transaction")
    if cost_body is None or top_level_members(cost_body, "struct") != [
        "payload_bytes",
        "fee",
        "cycles",
    ]:
        errors.append("model cost quotient coordinate universe differs")
    elif re.search(r"pub\((?:super|crate)\)\s+(?:payload_bytes|fee|cycles)\s*:", cost_body):
        errors.append("model cost quotient coordinates are not sealed")
    if transaction_body is None or top_level_members(transaction_body, "struct")[-1:] != [
        "cost"
    ]:
        errors.append("transaction does not own exactly one sealed cost quotient")
    for required in (
        "constBLOCK_VECTOR_OFFSET_BYTES:u32=4;",
        "self.payload_bytes+Self::BLOCK_VECTOR_OFFSET_BYTES",
        "pub(super)cost:ModelTransactionCost,",
        "bytes:self.cost.payload_bytes()",
        "serialized_bytes:owner.transaction.cost.serialized_bytes()",
    ):
        if required not in state:
            errors.append(f"model cost quotient lacks {required}")
    if ".cost.payload_bytes()" in kernel or ".cost.payload_bytes()" in scheduler:
        errors.append("an economic observation aliases retained payload bytes")
    for required in (
        "candidate.cost.serialized_bytes()",
        "transaction.cost.serialized_bytes()",
    ):
        if required not in kernel:
            errors.append(f"model economic kernel lacks {required}")
    if "bytes:owner.transaction.cost.serialized_bytes()" not in scheduler:
        errors.append("scheduler fee-rate quotient does not use block-serialized bytes")
    if (
        "pub(crate)fnready_order_observation(items:&[ModelTransactionCost])"
        not in model_refinement
        or ".with_cost(item)" not in model_refinement
    ):
        errors.append("Ready refinement does not consume the shared cost quotient")

    exact_cost_bridge = (
        "fnproduction_cost_receipt(transaction:&TransactionView,metrics:&CandidateMetrics,)"
        "->Option<ModelTransactionCost>"
    )
    for required in (
        exact_cost_bridge,
        "transaction.data().total_size()",
        "ModelTransactionCost::new(payload_bytes,metrics.fee.as_u64(),metrics.cost.cycles)",
        "cost.serialized_bytes()).ok()?==metrics.cost.serialized_bytes",
        "metrics.cost.serialized_bytes==transaction.data().serialized_size_in_block()",
    ):
        if required not in refinement:
            errors.append(f"production cost receipt lacks {required}")
    if refinement.count("production_cost_receipt(") != 3:
        errors.append("Ready and eviction do not share one production cost receipt")

    for required in (
        "Self::RemoteDeclaredCycles(limit)iflimit.declared()>large_cycle_threshold",
        "Self::RemoteDeclaredCycles(_)|Self::Trusted=>VerifyCycleClass::Small",
    ):
        if required not in production_state:
            errors.append(f"production verify-class quotient lacks {required}")
    for required in (
        "Self::RemoteDeclaredCycles(cycles)ifcycles>large_cycle_threshold",
        "Self::RemoteDeclaredCycles(_)|Self::Trusted=>VerifyCycleClass::Small",
    ):
        if required not in model_settlement:
            errors.append(f"model verify-class quotient lacks {required}")
    if ".payload_policy().verify_cycle_class(large_cycle_threshold)" not in resolver:
        errors.append("resolver bypasses the payload-policy verify-class quotient")
    return errors


def validate_cost_observation_provenance_sources() -> list[str]:
    paths = {
        "model_state_source": "tx-pool/src/tests/model/state.rs",
        "model_kernel_source": "tx-pool/src/tests/model/kernel.rs",
        "scheduler_source": "tx-pool/src/tests/model/scheduler_quotient.rs",
        "model_refinement_source": "tx-pool/src/tests/model/refinement.rs",
        "refinement_source": "tx-pool/src/authority/tests/refinement.rs",
        "production_state_source": "tx-pool/src/authority/state.rs",
        "resolver_source": "tx-pool/src/authority/resolver.rs",
        "model_settlement_source": "tx-pool/src/tests/model/settlement_transition.rs",
    }
    try:
        sources = {
            parameter: (REPO_ROOT / relative).read_text()
            for parameter, relative in paths.items()
        }
    except OSError as error:
        return [f"cannot inspect cost observation provenance: {error}"]
    return cost_observation_provenance_errors(**sources)


def validate_model_inputs(census: dict, axis_ids: set[str]) -> tuple[list[str], list[str]]:
    errors: list[str] = []
    groups = census.get("model_input_groups")
    if not isinstance(groups, list):
        return ["model input groups must be a list"], []
    seen_groups: set[str] = set()
    seen_edges: set[str] = set()
    open_edges: list[str] = []
    edge_statuses: dict[str, str] = {}
    for group in groups:
        if not isinstance(group, dict) or set(group) != {
            "id",
            "path",
            "symbol",
            "kind",
            "members",
            "default",
            "overrides",
        }:
            errors.append(f"invalid model input group {group!r}")
            continue
        group_id = group["id"]
        if not isinstance(group_id, str) or group_id in seen_groups:
            errors.append(f"invalid or duplicate model input group {group_id!r}")
            continue
        seen_groups.add(group_id)
        path = REPO_ROOT / group["path"]
        if not path.is_file():
            errors.append(f"model input group {group_id} path is absent")
            continue
        body = type_body(path.read_text(), group["symbol"])
        if body is None:
            errors.append(f"model input group {group_id} symbol is absent")
            continue
        kind = group["kind"]
        if kind not in {"struct", "enum"}:
            errors.append(f"model input group {group_id} kind differs")
            continue
        discovered = top_level_members(body, kind)
        members = group["members"]
        if not isinstance(members, list) or not all(
            isinstance(member, str) for member in members
        ):
            errors.append(f"model input group {group_id} members are invalid")
            continue
        if discovered != members:
            errors.append(f"model input group {group_id} member universe or order differs")
        default = group["default"]
        overrides = group["overrides"]
        classification_fields = {
                "axis_id",
                "producer_kind",
                "producer_ref",
                "relation_status",
                "falsifier",
        }
        if not isinstance(default, dict) or set(default) != classification_fields:
            errors.append(f"model input group {group_id} default fields differ")
            continue
        if (
            not isinstance(overrides, dict)
            or not set(overrides).issubset(set(members))
            or not all(
                isinstance(value, dict) and set(value) == classification_fields
                for value in overrides.values()
            )
        ):
            errors.append(f"model input group {group_id} overrides differ")
            continue
        for member_name in members:
            member = overrides.get(member_name, default)
            edge_id = f"{group_id}.{member_name}"
            if edge_id in seen_edges:
                errors.append(f"duplicate model input edge {edge_id}")
            seen_edges.add(edge_id)
            if member["axis_id"] not in axis_ids:
                errors.append(f"model input edge {edge_id} has unknown axis")
            if member["producer_kind"] not in ALLOWED_PRODUCER_KINDS:
                errors.append(f"model input edge {edge_id} producer kind differs")
            status = member["relation_status"]
            if status not in ALLOWED_RELATION_STATUS:
                errors.append(f"model input edge {edge_id} relation status differs")
            elif status in OPEN_RELATION_STATUS:
                open_edges.append(edge_id)
            edge_statuses[edge_id] = status
            if not isinstance(member["producer_ref"], str) or not member["producer_ref"]:
                errors.append(f"model input edge {edge_id} producer is absent")
            if not isinstance(member["falsifier"], str) or not member["falsifier"]:
                errors.append(f"model input edge {edge_id} falsifier is absent")

    query = census.get("function_input_edges")
    if not isinstance(query, list):
        errors.append("function input edges must be a list")
        query = []
    for edge in query:
        if not isinstance(edge, dict) or set(edge) != {
            "id",
            "path",
            "symbol",
            "parameter",
            "axis_id",
            "producer_kind",
            "producer_ref",
            "relation_status",
            "falsifier",
        }:
            errors.append(f"invalid function input edge {edge!r}")
            continue
        edge_id = edge["id"]
        if edge_id in seen_edges:
            errors.append(f"duplicate model input edge {edge_id}")
        seen_edges.add(edge_id)
        path = REPO_ROOT / edge["path"]
        phrase = f"{edge['parameter']}:"
        if not path.is_file() or edge["symbol"] not in path.read_text() or phrase not in path.read_text():
            errors.append(f"function input edge {edge_id} source binding differs")
        if edge["axis_id"] not in axis_ids:
            errors.append(f"function input edge {edge_id} has unknown axis")
        if edge["producer_kind"] not in ALLOWED_PRODUCER_KINDS:
            errors.append(f"function input edge {edge_id} producer kind differs")
        status = edge["relation_status"]
        if status not in ALLOWED_RELATION_STATUS:
            errors.append(f"function input edge {edge_id} relation status differs")
        elif status in OPEN_RELATION_STATUS:
            open_edges.append(edge_id)

        edge_statuses[edge_id] = status
    for forbidden in FORBIDDEN_FREE_DERIVED:
        if forbidden in edge_statuses:
            errors.append(f"forbidden derived semantic input {forbidden} was reintroduced")
    for required, expected_status in REQUIRED_PROPOSAL_INPUT_STATUS.items():
        if edge_statuses.get(required) != expected_status:
            errors.append(
                f"proposal input {required} is not bound as {expected_status}"
            )
    for required, expected_status in REQUIRED_PRODUCTION_PROVENANCE_STATUS.items():
        if edge_statuses.get(required) != expected_status:
            errors.append(
                f"production provenance {required} is not bound as {expected_status}"
            )
    for required, expected_status in REQUIRED_TWO_PHASE_INPUT_STATUS.items():
        if edge_statuses.get(required) != expected_status:
            errors.append(
                f"two-phase input {required} is not bound as {expected_status}"
            )
    for required in REQUIRED_COMPLETION_RELATIONS:
        if edge_statuses.get(required) != "proved":
            errors.append(f"completion relation {required} is not mechanically closed")
    for required, expected_status in REQUIRED_DEPENDENCY_INPUT_STATUS.items():
        if edge_statuses.get(required) != expected_status:
            errors.append(
                f"dependency input {required} is not bound as {expected_status}"
            )
    for required in REQUIRED_COST_RELATIONS:
        if edge_statuses.get(required) != "proved":
            errors.append(f"cost relation {required} is not mechanically closed")
    discovered_carriers = set(discovered_input_carriers())
    classified_carriers = {
        f"{group.get('path')}::{group.get('symbol')}"
        for group in groups
        if isinstance(group, dict)
        and f"{group.get('path')}::{group.get('symbol')}" in discovered_carriers
    }
    if classified_carriers != discovered_carriers:
        errors.append("model input carrier producer classification is incomplete")
    return errors, sorted(open_edges)


def validate_proposal_derivation_shape() -> list[str]:
    errors: list[str] = []
    model_root = REPO_ROOT / "tx-pool/src/tests/model"
    state_source = (model_root / "state.rs").read_text()
    kernel_source = (model_root / "kernel.rs").read_text()
    boundary_source = (model_root / "boundaries.rs").read_text()
    proposal_source = (model_root / "proposal.rs").read_text()
    eviction_source = (model_root / "eviction_quotient.rs").read_text()

    evidence_body = type_body(state_source, "ResolvedEvidence")
    if evidence_body is None or "proposal_status" in top_level_members(
        evidence_body, "struct"
    ):
        errors.append("resolved evidence can still carry a proposal status")

    transition_body = type_body(kernel_source, "ChainTransition")
    transition_members = (
        top_level_members(transition_body, "struct")
        if transition_body is not None
        else []
    )
    if "proposals" not in transition_members or {"proposed", "gap"}.intersection(
        transition_members
    ):
        errors.append("chain transition proposal history shape differs")

    query_match = re.search(
        r"fn\s+query_subject\s*\((.*?)\)\s*->", boundary_source, re.DOTALL
    )
    if query_match is None or "proposal_window_status" in query_match.group(1):
        errors.append("query subject can still accept a free proposal status")
    if "omega.proposal_status(&owner.transaction)" not in boundary_source:
        errors.append("query subject no longer derives status from authority history")

    sealed_receipt = re.search(
        r"pub\(crate\)\s+struct\s+ProposalStatusReceipt\s*\(\s*AcceptedStatus\s*\)\s*;",
        proposal_source,
    )
    if sealed_receipt is None:
        errors.append("proposal status receipt is not a sealed tuple")
    for path in sorted(model_root.glob("*.rs")):
        if path.name != "proposal.rs" and re.search(
            r"ProposalStatusReceipt\s*\(", path.read_text()
        ):
            errors.append(
                f"proposal status receipt is directly constructed outside its owner: {path.name}"
            )

    eviction_body = type_body(eviction_source, "EvictionRefinementInput")
    if eviction_body is None or not re.search(
        r"status\s*:\s*ProposalStatusReceipt", eviction_body
    ):
        errors.append("eviction quotient does not require a sealed proposal receipt")
    return errors


def validate_carrier_inventory(census: dict) -> list[str]:
    errors: list[str] = []
    carriers = census.get("discovered_input_carriers")
    if not isinstance(carriers, list) or len(carriers) != len(set(carriers)):
        return ["discovered model input carrier inventory is invalid"]
    if carriers != discovered_input_carriers():
        errors.append("discovered model input carrier inventory differs")
    return errors


def differential_symptom_partition() -> dict[str, str]:
    value = json.loads(
        (
            REPO_ROOT
            / "tx-pool/optimization-evidence/integration-differential-census.json"
        ).read_text()
    )
    partitions = value["derived_partitions"]
    labels = (
        "both_cross_cells",
        "baseline_harness_candidate_product_only",
        "candidate_harness_baseline_product_only",
        "extension_baseline_product_failures",
    )
    result: dict[str, str] = {}
    for label in labels:
        for symptom in partitions[label]:
            if symptom in result:
                raise ValueError(f"duplicate differential symptom {symptom}")
            result[symptom] = label
    return result


def validate_difference_adjudication(
    census: dict, axis_ids: set[str]
) -> tuple[list[str], dict[str, int], list[str]]:
    errors: list[str] = []
    clusters = census.get("difference_clusters")
    rows = census.get("difference_dispositions")
    if not isinstance(clusters, list) or not isinstance(rows, list):
        return ["difference clusters and dispositions must be lists"], {}, []
    source_ids = {
        identity.get("id")
        for identity in census.get("source_identities", [])
        if isinstance(identity, dict)
    }
    cluster_ids: set[str] = set()
    cluster_symptoms: dict[str, set[str]] = {}
    for cluster in clusters:
        if not isinstance(cluster, dict) or set(cluster) != {
            "id",
            "axis_ids",
            "violated_law",
            "symptom_ids",
            "root_action",
            "deletion_counterexample",
        }:
            errors.append(f"invalid difference cluster {cluster!r}")
            continue
        cluster_id = cluster["id"]
        if not isinstance(cluster_id, str) or cluster_id in cluster_ids:
            errors.append(f"invalid or duplicate difference cluster {cluster_id!r}")
            continue
        cluster_ids.add(cluster_id)
        axes = cluster["axis_ids"]
        symptoms = cluster["symptom_ids"]
        if (
            not isinstance(axes, list)
            or not axes
            or len(axes) != len(set(axes))
            or not set(axes).issubset(axis_ids)
        ):
            errors.append(f"difference cluster {cluster_id} axes differ")
        if (
            not isinstance(symptoms, list)
            or not symptoms
            or len(symptoms) != len(set(symptoms))
        ):
            errors.append(f"difference cluster {cluster_id} symptoms differ")
            symptoms = []
        cluster_symptoms[cluster_id] = set(symptoms)
        for field in ("violated_law", "root_action", "deletion_counterexample"):
            if not isinstance(cluster[field], str) or not cluster[field]:
                errors.append(f"difference cluster {cluster_id} lacks {field}")

    expected_partition = differential_symptom_partition()
    seen: set[str] = set()
    row_clusters: dict[str, set[str]] = {cluster_id: set() for cluster_id in cluster_ids}
    counts = {disposition: 0 for disposition in sorted(ALLOWED_DIFFERENCE_DISPOSITIONS)}
    required_fields = {
        "id",
        "cluster_id",
        "source_partition",
        "trace",
        "observation_axis",
        "baseline_observation",
        "candidate_observation",
        "adjudication_refs",
        "shortest_continuation",
        "disposition",
        "semantic_owner",
        "required_root_action",
        "evidence_status",
    }
    for row in rows:
        if not isinstance(row, dict) or set(row) != required_fields:
            errors.append(f"invalid difference disposition {row!r}")
            continue
        symptom = row["id"]
        if not isinstance(symptom, str) or symptom in seen:
            errors.append(f"invalid or duplicate difference symptom {symptom!r}")
            continue
        seen.add(symptom)
        if row["source_partition"] != expected_partition.get(symptom):
            errors.append(f"difference symptom {symptom} source partition differs")
        cluster_id = row["cluster_id"]
        if cluster_id not in cluster_ids:
            errors.append(f"difference symptom {symptom} has unknown cluster")
        else:
            row_clusters[cluster_id].add(symptom)
        if row["observation_axis"] not in axis_ids:
            errors.append(f"difference symptom {symptom} has unknown observation axis")
        refs = row["adjudication_refs"]
        if (
            not isinstance(refs, list)
            or not refs
            or len(refs) != len(set(refs))
            or not set(refs).issubset(axis_ids | source_ids)
        ):
            errors.append(f"difference symptom {symptom} adjudication references differ")
        disposition = row["disposition"]
        if disposition not in ALLOWED_DIFFERENCE_DISPOSITIONS:
            errors.append(f"difference symptom {symptom} disposition differs")
        else:
            counts[disposition] += 1
        if (
            disposition == "intentional_delta_with_owner"
            and not str(row["semantic_owner"]).startswith("authorization:")
        ):
            errors.append(f"intentional difference {symptom} has no authorization")
        for field in (
            "trace",
            "baseline_observation",
            "candidate_observation",
            "shortest_continuation",
            "semantic_owner",
            "required_root_action",
        ):
            if not isinstance(row[field], str) or not row[field]:
                errors.append(f"difference symptom {symptom} lacks {field}")
        if row["evidence_status"] != "verified":
            errors.append(f"difference symptom {symptom} is not verified")
    if seen != set(expected_partition):
        errors.append("difference disposition universe differs")
    for cluster_id, symptoms in cluster_symptoms.items():
        if symptoms != row_clusters.get(cluster_id, set()):
            errors.append(f"difference cluster {cluster_id} row projection differs")
    clustered = set().union(*cluster_symptoms.values()) if cluster_symptoms else set()
    if clustered != set(expected_partition):
        errors.append("difference cluster universe differs")
    unadjudicated = sorted(set(expected_partition) - seen)
    return errors, counts, unadjudicated


def validate_completeness(
    census: dict,
    axis_ids: set[str],
    semantic_grain_ids: list[str],
    implementation_axis_ids: set[str],
    open_model_edges: list[str],
    open_production_edges: list[str],
    difference_counts: dict[str, int],
    unadjudicated_differences: list[str],
) -> list[str]:
    errors: list[str] = []
    expected = {
        "normative_axis_ids": sorted(axis_ids),
        "implementation_axis_ids": sorted(implementation_axis_ids),
        "relation_axis_ids": sorted(axis_ids),
        "semantic_grain_ids": semantic_grain_ids,
        "semantic_grain_projection_differences": [],
        "axis_projection_differences": [],
        "unclassified_model_inputs": [],
        "open_model_relations": open_model_edges,
        "open_production_refinement_edges": open_production_edges,
        "difference_disposition_counts": difference_counts,
        "unadjudicated_differences": unadjudicated_differences,
        "integration_behavior_partition_state": "complete",
        "census_state": (
            "exact_grain_axis_input_composition_and_current_candidate_projection_complete"
            if not open_production_edges
            else "exact_grain_axis_input_and_composition_projection_complete_with_open_current_candidate_production_refinement"
        ),
    }
    if census.get("completeness") != expected:
        errors.append("semantic census completeness projection differs")
    return errors


def validate(census: dict, verify_sources: bool = True) -> list[str]:
    errors: list[str] = []
    expected_top = {
        "schema_version",
        "authority",
        "goal_ref",
        "source_identities",
        "source_sets",
        "semantic_axes",
        "semantic_grains",
        "dynamic_compositions",
        "bottom_up_surfaces",
        "production_refinement_edges",
        "model_input_groups",
        "function_input_edges",
        "discovered_input_carriers",
        "difference_clusters",
        "difference_dispositions",
        "completeness",
        "evidence_sha256",
    }
    if not isinstance(census, dict) or set(census) != expected_top:
        return ["semantic refinement census fields differ"]
    if census["schema_version"] != 7:
        errors.append("semantic refinement census schema version differs")
    if census["authority"] != (
        "construction_completeness_census_separates_normative_protocol_candidate_production_and_compatibility_evidence_while_rules_remain_source_owned_and_gaps_are_not_release_evidence"
    ):
        errors.append("semantic refinement census authority boundary differs")
    if census["goal_ref"] != "tx-pool/architecture-contract.json#/optimization_goal":
        errors.append("semantic refinement census goal reference differs")
    payload = {key: value for key, value in census.items() if key != "evidence_sha256"}
    if census["evidence_sha256"] != canonical_sha256(payload):
        errors.append("semantic refinement census canonical hash differs")
    if verify_sources:
        errors.extend(validate_source_identities(census))
        errors.extend(validate_source_sets(census))
        errors.extend(validate_eviction_status_provenance_sources())
        errors.extend(validate_cost_observation_provenance_sources())
        errors.extend(validate_conditional_cycle_refinement_sources())
        errors.extend(validate_template_packing_refinement_sources())
    axis_errors, axis_ids = validate_axes(census)
    errors.extend(axis_errors)
    grain_errors, semantic_grain_ids = validate_semantic_grains(census, axis_ids)
    errors.extend(grain_errors)
    errors.extend(validate_dynamic_compositions(census, axis_ids))
    surface_errors, implementation_axis_ids = validate_surfaces(census, axis_ids)
    errors.extend(surface_errors)
    for axis in census.get("semantic_axes", []):
        if not isinstance(axis, dict):
            continue
        owned = set(axis.get("bottom_up_surface_ids", []))
        actual = {
            surface.get("id")
            for surface in census.get("bottom_up_surfaces", [])
            if isinstance(surface, dict) and surface.get("axis_id") == axis.get("id")
        }
        if owned != actual:
            errors.append(f"axis {axis.get('id')} surface projection differs")
    production_errors, open_production_edges = validate_production_refinement_edges(
        census, axis_ids
    )
    errors.extend(production_errors)
    input_errors, open_model_edges = validate_model_inputs(census, axis_ids)
    errors.extend(input_errors)
    errors.extend(validate_proposal_derivation_shape())
    errors.extend(validate_carrier_inventory(census))
    difference_errors, difference_counts, unadjudicated = (
        validate_difference_adjudication(census, axis_ids)
    )
    errors.extend(difference_errors)
    errors.extend(
        validate_completeness(
            census,
            axis_ids,
            semantic_grain_ids,
            implementation_axis_ids,
            open_model_edges,
            open_production_edges,
            difference_counts,
            unadjudicated,
        )
    )
    return errors


def update_derived_projections(census: dict) -> list[str]:
    """Refresh only mechanically derived identities and exact-grain projections."""

    try:
        contract = json.loads(CONTRACT_PATH.read_text())
        registry_raw = BEHAVIOR_REGISTRY_PATH.read_bytes()
        registry = json.loads(registry_raw)
    except (OSError, json.JSONDecodeError) as error:
        return [f"cannot update semantic census projections: {error}"]
    semantic_grains, errors = derive_semantic_grains(contract, registry)
    if errors:
        return errors
    census["schema_version"] = 7
    census["semantic_grains"] = semantic_grains
    source_sets = census.get("source_sets")
    if not isinstance(source_sets, dict) or set(source_sets) != set(
        REQUIRED_SOURCE_SET_PATTERNS
    ):
        return ["cannot update semantic census without the exact source-set universe"]
    for set_id, patterns in REQUIRED_SOURCE_SET_PATTERNS.items():
        paths = source_set_paths(set_id)
        relative = [path.relative_to(REPO_ROOT).as_posix() for path in paths]
        source_sets[set_id] = {
            "patterns": patterns,
            "path_count": len(paths),
            "path_set_sha256": line_set_sha256(relative),
            "content_set_sha256": content_set_sha256(paths),
        }
    model_input_groups = census.get("model_input_groups")
    if not isinstance(model_input_groups, list):
        return ["cannot update semantic census without model input groups"]
    for group in model_input_groups:
        if not isinstance(group, dict):
            return ["cannot update an invalid model input group"]
        try:
            path = REPO_ROOT / group["path"]
            body = type_body(path.read_text(), group["symbol"])
            kind = group["kind"]
        except (KeyError, OSError, TypeError) as error:
            return [f"cannot update model input group: {error}"]
        if body is None or kind not in {"struct", "enum"}:
            return [f"cannot derive model input group {group.get('id')}"]
        group["members"] = top_level_members(body, kind)
        overrides = group.get("overrides")
        if not isinstance(overrides, dict) or not set(overrides).issubset(
            set(group["members"])
        ):
            return [f"model input group {group.get('id')} has stale overrides"]
    for source in census.get("source_identities", []):
        if not isinstance(source, dict):
            return ["cannot update an invalid source identity"]
        try:
            source_path = REPO_ROOT / source["path"]
            source["sha256"] = (
                optimization_contract_semantic_sha256(source_path)
                if source.get("id") == "NS-OPTIMIZATION-CONTRACT"
                else raw_sha256(source_path)
            )
        except (KeyError, OSError, ValueError, json.JSONDecodeError) as error:
            return [f"cannot update source identity {source.get('id')}: {error}"]
    completeness = census.get("completeness")
    if not isinstance(completeness, dict):
        return ["cannot update semantic census without completeness projection"]
    completeness["semantic_grain_ids"] = [row["id"] for row in semantic_grains]
    completeness["semantic_grain_projection_differences"] = []
    open_production = completeness.get("open_production_refinement_edges")
    completeness["census_state"] = (
        "exact_grain_axis_input_composition_and_current_candidate_projection_complete"
        if open_production == []
        else "exact_grain_axis_input_and_composition_projection_complete_with_open_current_candidate_production_refinement"
    )
    census["evidence_sha256"] = canonical_sha256(
        {key: value for key, value in census.items() if key != "evidence_sha256"}
    )
    return []


def validate_canaries(census: dict) -> list[str]:
    errors: list[str] = []
    errors.extend(validate_template_packing_refinement_canary())
    missing_axis = copy.deepcopy(census)
    missing_axis["semantic_axes"].pop()
    if not any("axis universe differs" in error for error in validate(missing_axis, False)):
        errors.append("semantic census missing-axis canary was admitted")

    missing_grain = copy.deepcopy(census)
    missing_grain["semantic_grains"].pop()
    if not any(
        "semantic grain relation projection differs" in error
        for error in validate(missing_grain, False)
    ):
        errors.append("semantic census missing-grain canary was admitted")

    rebound_grain = copy.deepcopy(census)
    rebound_grain["semantic_grains"][0]["axis_id"] = (
        "SA-NONCONTEXTUAL-VALIDITY"
    )
    if not any(
        "semantic grain relation projection differs" in error
        for error in validate(rebound_grain, False)
    ):
        errors.append("semantic census rebound-grain canary was admitted")

    phantom_role = copy.deepcopy(census)
    phantom_role["semantic_grains"][0]["production_role_refs"].append(
        "phantom_production_role"
    )
    if not any(
        "semantic grain relation projection differs" in error
        for error in validate(phantom_role, False)
    ):
        errors.append("semantic census phantom-role canary was admitted")

    omitted_role = copy.deepcopy(census)
    omitted_role["semantic_grains"][0]["production_role_refs"].pop()
    if not any(
        "semantic grain relation projection differs" in error
        for error in validate(omitted_role, False)
    ):
        errors.append("semantic census omitted-role canary was admitted")

    wrong_behavior_role = copy.deepcopy(census)
    first_grain = wrong_behavior_role["semantic_grains"][0]
    other_role = next(
        role
        for grain in wrong_behavior_role["semantic_grains"][1:]
        for role in grain["production_role_refs"]
        if grain["behavior_id"] != first_grain["behavior_id"]
        and role not in first_grain["production_role_refs"]
    )
    first_grain["production_role_refs"][0] = other_role
    first_grain["production_role_refs"].sort()
    if not any(
        "semantic grain relation projection differs" in error
        for error in validate(wrong_behavior_role, False)
    ):
        errors.append("semantic census wrong-behavior-role canary was admitted")

    inconsistent_behavior_copy = copy.deepcopy(census)
    by_behavior: dict[str, list[dict]] = {}
    for grain in inconsistent_behavior_copy["semantic_grains"]:
        by_behavior.setdefault(grain["behavior_id"], []).append(grain)
    repeated = next(rows for rows in by_behavior.values() if len(rows) > 1)
    repeated[1]["behavior_role_relation_sha256"] = "0" * 64
    if not any(
        "semantic grain relation projection differs" in error
        for error in validate(inconsistent_behavior_copy, False)
    ):
        errors.append(
            "semantic census inconsistent behavior-role-copy canary was admitted"
        )

    missing_composition = copy.deepcopy(census)
    missing_composition["dynamic_compositions"].pop()
    if not any(
        "dynamic composition universe differs" in error
        for error in validate(missing_composition, False)
    ):
        errors.append("semantic census missing-composition canary was admitted")

    missing_production_edge = copy.deepcopy(census)
    missing_production_edge["production_refinement_edges"].pop()
    if not any(
        "production refinement edge universe differs" in error
        for error in validate(missing_production_edge, False)
    ):
        errors.append("semantic census missing-production-edge canary was admitted")

    false_current_refinement = copy.deepcopy(census)
    current_edge = next(
        row
        for row in false_current_refinement["production_refinement_edges"]
        if row["id"] == "PR-REMOTE-CYCLE-LIMIT"
    )
    current_edge["status"] = PROVED_PRODUCTION_REFINEMENT_STATUS
    original_current_validator = globals()[
        "validate_current_candidate_refinement_sources"
    ]
    globals()["validate_current_candidate_refinement_sources"] = lambda edge_id: [
        f"{edge_id} current production observation differs"
    ]
    try:
        if not any(
            "current production observation differs" in error
            for error in validate(false_current_refinement, False)
        ):
            errors.append("semantic census false current-production proof canary was admitted")
    finally:
        globals()[
            "validate_current_candidate_refinement_sources"
        ] = original_current_validator

    model_property_as_producer = copy.deepcopy(census)
    model_property_as_producer["production_refinement_edges"][0][
        "production_producers"
    ] = copy.deepcopy(
        model_property_as_producer["production_refinement_edges"][0]["model_anchors"]
    )
    if not any(
        "production_producers anchor universe differs" in error
        for error in validate(model_property_as_producer, False)
    ):
        errors.append("semantic census model-as-production canary was admitted")

    misbound_production_anchor = copy.deepcopy(census)
    misbound_production_anchor["production_refinement_edges"][0][
        "production_producers"
    ][0]["path"] = "tx-pool/src/tests/model/boundaries.rs"
    if not any(
        "production_producers anchor universe differs" in error
        for error in validate(misbound_production_anchor, False)
    ):
        errors.append("semantic census production-anchor binding canary was admitted")

    untyped_service = copy.deepcopy(census)
    untyped_service["dynamic_compositions"][0]["premises"][1]["kind"] = (
        "qualitative_fairness"
    )
    if not any(
        "premise kind differs" in error
        for error in validate(untyped_service, False)
    ):
        errors.append("semantic census untyped-service canary was admitted")

    false_capacity_proof = copy.deepcopy(census)
    for group in false_capacity_proof["model_input_groups"]:
        if group["id"] == "MI-CURRENT-TEMPLATE-COMPOSITION":
            group["default"]["producer_kind"] = "authority_state"
            group["default"]["relation_status"] = "proved"
    if not any(
        "two-phase input" in error
        for error in validate(false_capacity_proof, False)
    ):
        errors.append("semantic census false-template-environment-proof canary was admitted")

    try:
        two_phase_source = (
            REPO_ROOT / "tx-pool/src/tests/model/two_phase.rs"
        ).read_text()
    except OSError:
        errors.append("cannot construct template-service-premise source canary")
    else:
        raw_capacity = two_phase_source.replace(
            "premise: TemplateServicePremise,", "premise: usize,", 1
        )
        if not template_service_premise_errors(raw_capacity):
            errors.append("raw-template-capacity proxy canary was admitted")
        spliced_dependency_bound = two_phase_source.replace(
            "source.captured_dependency_edge_bound,", "usize::MAX,", 1
        )
        if not template_service_premise_errors(spliced_dependency_bound):
            errors.append("template-service dependency-bound splice canary was admitted")
        unsealed_cohort = two_phase_source.replace(
            "retained_source_indices: initial.retained_source_indices,",
            "retained_source_indices: initial.packed_source_indices,",
            1,
        )
        if not template_service_premise_errors(unsealed_cohort):
            errors.append("two-phase unsealed-compiler-output canary was admitted")
        unbounded_proposals = two_phase_source.replace(
            "source.composition.max_block_proposals,", "usize::MAX,"
        )
        if not template_service_premise_errors(unbounded_proposals):
            errors.append("two-phase consensus-proposal-limit omission canary was admitted")
        unsealed_service_prefix = two_phase_source.replace(
            "let proposals = compilation\n            .proposal_source_indices",
            "let proposals = compilation\n            .retained_source_indices",
            1,
        )
        if not template_service_premise_errors(unsealed_service_prefix):
            errors.append("two-phase retained/proposal-prefix alias canary was admitted")
        statusless_current_pack = two_phase_source.replace(
            "AcceptedStatus::Pending => ProposalWindowPosition::Outside,",
            "AcceptedStatus::Pending => ProposalWindowPosition::Proposed,",
            1,
        )
        if not template_service_premise_errors(statusless_current_pack):
            errors.append("two-phase Pending/current-pack alias canary was admitted")
        unsealed_uncles = two_phase_source.replace(
            "source.composition.candidate_uncles.iter().cloned(),",
            "std::iter::empty(),",
            1,
        )
        if not template_service_premise_errors(unsealed_uncles):
            errors.append("two-phase candidate-uncle source splice canary was admitted")
        unbounded_residence = two_phase_source.replace(
            "if proposal_span > residence_span {",
            "if false {",
            1,
        )
        if not template_service_premise_errors(unbounded_residence):
            errors.append("two-phase proposal residence-span omission canary was admitted")
        conflated_service = two_phase_source.replace(
            ".realizes_commit_offer()",
            ".realizes_proposal_offer()",
            1,
        )
        if not template_service_premise_errors(conflated_service):
            errors.append("two-phase proposal/commit service alias canary was admitted")

    open_phase_exit = copy.deepcopy(census)
    open_phase_exit["semantic_axes"][0]["model_relation"]["status"] = (
        "open_completion_identity"
    )
    if not any(
        "phase-exit status differs" in error
        for error in validate(open_phase_exit, False)
    ):
        errors.append("semantic census open phase-exit relation canary was admitted")

    uncovered_axis = copy.deepcopy(census)
    uncovered_axis["semantic_axes"][0]["model_relation"][
        "dynamic_composition_refs"
    ] = []
    if not any(
        "axis coverage differs" in error
        for error in validate(uncovered_axis, False)
    ):
        errors.append("semantic census uncovered-axis canary was admitted")

    free_derived = copy.deepcopy(census)
    free_derived["function_input_edges"].append(
        {
            "id": "MI-QUERY-SUBJECT.proposal_window_status",
            "path": "tx-pool/src/tests/model/eviction_quotient.rs",
            "symbol": "fn eviction_status_witness",
            "parameter": "status",
            "axis_id": "SA-PROPOSAL-WINDOW-TWO-STEP",
            "producer_kind": "pure_derivation",
            "producer_ref": "forbidden_free_status",
            "relation_status": "invalid_free_derived",
            "falsifier": "caller_fabricates_proposal_status",
        }
    )
    if not any(
        "forbidden derived semantic input" in error
        for error in validate(free_derived, False)
    ):
        errors.append("semantic census free-derived canary was admitted")

    false_provenance = copy.deepcopy(census)
    for edge in false_provenance["function_input_edges"]:
        if edge["id"] == "MI-EVICTION-PRODUCTION-STATUS-RECEIPT.receipt":
            edge["relation_status"] = "typed_assumption"
    if not any(
        "production provenance" in error
        for error in validate(false_provenance, False)
    ):
        errors.append("semantic census production-provenance canary was admitted")

    try:
        refinement_source = (
            REPO_ROOT / "tx-pool/src/authority/tests/refinement.rs"
        ).read_text()
        production_checker_source = (
            REPO_ROOT / "tx-pool/scripts/check_production_contracts.py"
        ).read_text()
    except OSError:
        errors.append("cannot construct eviction receipt provenance source canary")
    else:
        free_receipt = refinement_source.replace(
            "production_eviction_status_receipt(&entry.proposal)",
            "eviction_status_witness(refinement_status(entry.status()))",
            1,
        )
        if not any(
            "free cached-status witness" in error
            or "does not consume the Accepted entry receipt" in error
            for error in eviction_status_provenance_errors(
                free_receipt, production_checker_source
            )
        ):
            errors.append("semantic census free eviction-status bridge canary was admitted")

    try:
        cost_sources = {
            "model_state_source": (
                REPO_ROOT / "tx-pool/src/tests/model/state.rs"
            ).read_text(),
            "model_kernel_source": (
                REPO_ROOT / "tx-pool/src/tests/model/kernel.rs"
            ).read_text(),
            "scheduler_source": (
                REPO_ROOT / "tx-pool/src/tests/model/scheduler_quotient.rs"
            ).read_text(),
            "model_refinement_source": (
                REPO_ROOT / "tx-pool/src/tests/model/refinement.rs"
            ).read_text(),
            "refinement_source": refinement_source,
            "production_state_source": (
                REPO_ROOT / "tx-pool/src/authority/state.rs"
            ).read_text(),
            "resolver_source": (
                REPO_ROOT / "tx-pool/src/authority/resolver.rs"
            ).read_text(),
            "model_settlement_source": (
                REPO_ROOT / "tx-pool/src/tests/model/settlement_transition.rs"
            ).read_text(),
        }
    except (OSError, UnboundLocalError):
        errors.append("cannot construct cost-observation source canaries")
    else:
        payload_alias = copy.deepcopy(cost_sources)
        payload_alias["model_kernel_source"] = payload_alias[
            "model_kernel_source"
        ].replace(
            "candidate.cost.serialized_bytes()",
            "candidate.cost.payload_bytes()",
            1,
        )
        if not any(
            "aliases retained payload bytes" in error
            for error in cost_observation_provenance_errors(**payload_alias)
        ):
            errors.append("semantic census payload/serialized alias canary was admitted")

        false_block_size = copy.deepcopy(cost_sources)
        false_block_size["refinement_source"] = false_block_size[
            "refinement_source"
        ].replace(
            "transaction.data().serialized_size_in_block()",
            "transaction.data().total_size()",
            1,
        )
        if not any(
            "production cost receipt lacks" in error
            for error in cost_observation_provenance_errors(**false_block_size)
        ):
            errors.append("semantic census false block-size receipt canary was admitted")

        free_verify_class = copy.deepcopy(cost_sources)
        free_verify_class["resolver_source"] = free_verify_class[
            "resolver_source"
        ].replace(
            ".verify_cycle_class(large_cycle_threshold)",
            ".verify_cycle_class(u64::MAX)",
            1,
        )
        if not any(
            "resolver bypasses" in error
            for error in cost_observation_provenance_errors(**free_verify_class)
        ):
            errors.append("semantic census free verify-class canary was admitted")

    unproved_cost = copy.deepcopy(census)
    for group in unproved_cost["model_input_groups"]:
        if group["id"] == "MI-TRANSACTION-COST":
            group["overrides"]["fee"]["relation_status"] = "typed_assumption"
    if not any(
        "cost relation MI-TRANSACTION-COST.fee" in error
        for error in validate(unproved_cost, False)
    ):
        errors.append("semantic census unproved-cost canary was admitted")

    false_script_proof = copy.deepcopy(census)
    script_edge = next(
        row
        for row in false_script_proof["production_refinement_edges"]
        if row["id"] == "PR-SCRIPT-PROOF-QUOTIENT"
    )
    script_edge["status"] = PROVED_PRODUCTION_REFINEMENT_STATUS
    original_validator = globals()["validate_script_proof_refinement_sources"]
    globals()["validate_script_proof_refinement_sources"] = lambda: [
        "script proof cache is not a sealed cycles-only quotient"
    ]
    try:
        if not any(
            "script proof cache is not a sealed cycles-only quotient" in error
            for error in validate(false_script_proof, False)
        ):
            errors.append("semantic census false script-proof source canary was admitted")
    finally:
        globals()["validate_script_proof_refinement_sources"] = original_validator

    unproved_completion = copy.deepcopy(census)
    for group in unproved_completion["model_input_groups"]:
        if group["id"] == "MI-WORK-RESULT":
            group["overrides"]["Verified"]["relation_status"] = "typed_assumption"
    if not any(
        "completion relation" in error
        for error in validate(unproved_completion, False)
    ):
        errors.append("semantic census unproved-completion canary was admitted")

    free_group_tag = copy.deepcopy(census)
    for group in free_group_tag["model_input_groups"]:
        if group["id"] == "MI-TRANSACTION":
            group["overrides"]["dep_groups"]["relation_status"] = "typed_assumption"
    if not any(
        "dependency input MI-TRANSACTION.dep_groups" in error
        for error in validate(free_group_tag, False)
    ):
        errors.append("semantic census free-dep-group canary was admitted")

    missing_member = copy.deepcopy(census)
    missing_member["model_input_groups"][0]["members"].pop()
    if not any("member universe" in error for error in validate(missing_member, False)):
        errors.append("semantic census missing-model-input canary was admitted")

    duplicate_surface = copy.deepcopy(census)
    duplicate_surface["bottom_up_surfaces"].append(
        copy.deepcopy(duplicate_surface["bottom_up_surfaces"][0])
    )
    if not any("duplicate surface" in error for error in validate(duplicate_surface, False)):
        errors.append("semantic census duplicate-surface canary was admitted")

    missing_difference = copy.deepcopy(census)
    missing_difference["difference_dispositions"].pop()
    if not any(
        "difference disposition universe differs" in error
        for error in validate(missing_difference, False)
    ):
        errors.append("semantic census missing-difference canary was admitted")

    unauthorized = copy.deepcopy(census)
    unauthorized["difference_dispositions"][0]["disposition"] = (
        "intentional_delta_with_owner"
    )
    if not any("has no authorization" in error for error in validate(unauthorized, False)):
        errors.append("semantic census unauthorized-delta canary was admitted")

    contract_path = REPO_ROOT / "tx-pool/architecture-contract.json"
    try:
        contract = json.loads(contract_path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        errors.append(f"semantic projection canary cannot load contract: {error}")
    else:
        def semantic_projection_hash(value: dict) -> str:
            return canonical_sha256(
                {
                    field: value[field]
                    for field in OPTIMIZATION_CONTRACT_SEMANTIC_FIELDS
                }
            )

        baseline_hash = semantic_projection_hash(contract)
        descendant = copy.deepcopy(contract)
        first_evidence = next(
            iter(descendant["optimality_protocol"]["construction_evidence"].values())
        )
        first_evidence["sha256"] = "0" * 64
        if semantic_projection_hash(descendant) != baseline_hash:
            errors.append(
                "semantic census projection forms a cycle through certificate evidence"
            )
        changed_semantics = copy.deepcopy(contract)
        changed_semantics["optimization_goal"]["scope"] += "_canary"
        if semantic_projection_hash(changed_semantics) == baseline_hash:
            errors.append("semantic census projection omitted a normative goal change")
    return errors


def main() -> int:
    try:
        census = json.loads(CENSUS_PATH.read_text())
    except (OSError, json.JSONDecodeError) as error:
        print(f"error: cannot load semantic refinement census: {error}", file=sys.stderr)
        return 1
    if sys.argv[1:] == ["--update"]:
        update_errors = update_derived_projections(census)
        if update_errors:
            for error in update_errors:
                print(f"error: {error}", file=sys.stderr)
            return 1
        CENSUS_PATH.write_text(json.dumps(census, indent=2) + "\n")
    elif sys.argv[1:]:
        print("usage: check_semantic_refinement_census.py [--update]", file=sys.stderr)
        return 2
    errors = [*validate(census), *validate_canaries(census)]
    if errors:
        for error in errors:
            print(f"error: {error}", file=sys.stderr)
        return 1
    print(
        "validated semantic refinement census: "
        f"{len(census['semantic_axes'])} axes, "
        f"{len(census['semantic_grains'])} exact grains, "
        f"{len(census['dynamic_compositions'])} dynamic compositions, "
        f"{len(census['bottom_up_surfaces'])} bottom-up surfaces, "
        f"{len(census['difference_dispositions'])} total difference dispositions, "
        f"{len(census['completeness']['open_model_relations'])} owned model gaps, "
        f"{len(census['completeness']['open_production_refinement_edges'])} open production refinement edges"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
