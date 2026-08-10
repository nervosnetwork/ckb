#!/usr/bin/env python3
"""Validate the current tx-pool security evidence against nextest discovery."""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile

from check_review_guide import (
    invariant_unit_evidence,
    load_registry,
    repo_path,
    target_invariant_ids,
    validate_interruption_contract,
    validate_registry,
)


REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_MANIFEST = REPO_ROOT / "tx-pool" / "security-regression-manifest.json"
REQUIRED_ROOT_FAMILIES = {f"F{number}" for number in range(1, 9)}
REQUIRED_PROOF_POLICY = {
    "primary_evidence": "executable_mathematical_model_and_mechanical_check_before_prose",
    "system_transition": "total_Step_over_AuthoritySlot_P_D_L_with_KernelStep_over_Omega_A_K",
    "batch_equivalence": "ObsKernel_CommitBatch_Omega_equals_no_interleave_canonical_KernelStep_fold",
    "conservation": "exact_owner_charge_effect_capability_and_compute_permit_conservation",
    "progress": "per_obligation_rank_or_monotonic_level_under_named_fairness_premises",
    "model_review": "phase_boundary_model_delta_and_refinement_audit",
    "prose_role": "trusted_boundaries_assumptions_and_rationale_only",
}
REQUIRED_RELEASE_COMPATIBILITY_POLICY = {
    "node_downgrade": "unsupported",
    "legacy_tx_pool_configuration": "accepted_on_forward_upgrade",
    "missing_new_configuration_fields": "validated_compatibility_defaults",
    "legacy_verify_budget": "translated_without_shrinking_the_old_aggregate",
    "legacy_persistence_v1": "accepted_and_revalidated",
    "current_persistence_write": "v2_only",
    "reverse_persistence_migration": "unsupported",
}
REQUIRED_CONVERGENCE_STATES = ["construction", "acceptance", "accepted"]
REQUIRED_CONVERGENCE_LAW_SOURCES = {
    "root_families",
    "target_invariants",
    "optimization_goal",
    "release_surface",
    "landing_protocol",
    "construction_root_families",
    "residual_risks",
}
REQUIRED_ACCEPTANCE_UNIVERSE = {
    "production_sources",
    "resolved_features",
    "test_inventory",
    "configuration_and_migration_surfaces",
    "tool_semantics",
    "optimization_certificate",
    "landing_target_and_rehearsal",
    "declared_workload_environment_matrix",
}
REQUIRED_CONSTRUCTION_INVALIDATORS = {
    "production_source_change",
    "model_or_release_law_test_change",
    "resolved_feature_change",
    "configuration_or_migration_change",
    "landing_reconciliation_change",
    "semantic_contract_change",
    "new_semantic_family",
}
REQUIRED_EVIDENCE_INVALIDATORS = {
    "correctness_oracle_change_without_product_or_release_law_change",
    "tool_semantics_change",
    "performance_environment_change",
    "fixed_binary_or_workload_change",
    "raw_evidence_artifact_change",
}
REQUIRED_MUTATION_TERMINALS = {
    "caught",
    "compile_unviable",
    "proved_equivalent_or_unconstructible",
    "release_blocker",
}
REQUIRED_CONVERGENCE_PHASES = {
    "basis_and_roadmap_normalization",
    "release_boundary_adjudication",
    "architecture_optimality_synthesis",
    "registered_semantic_root_closure",
    "constructive_simplification",
    "landing_rehearsal",
    "evidence_universe_freeze",
    "complete_mutation",
    "deterministic_smoke",
    "complete_correctness",
    "empirical_performance_acceptance",
    "portability_and_final_review",
}
REQUIRED_OPTIMALITY_FEASIBILITY_SOURCES = {
    "optimization_goal",
    "proof_policy",
    "root_families",
    "target_invariants",
    "release_surface",
    "landing_protocol",
    "construction_root_families",
    "residual_risks",
}
REQUIRED_OPTIMALITY_NORMAL_FORM_AXES = {
    "authority_and_atomic_commit",
    "lifecycle_capability_and_evidence",
    "commutativity_coupling_and_batching",
    "dependency_scheduler_and_progress",
    "resources_and_adversarial_bounds",
    "effects_queries_and_derived_projections",
    "tasks_locks_queues_channels_and_failure_domains",
    "compatibility_migration_and_landing",
}
REQUIRED_OPTIMALITY_CERTIFICATE_REQUIREMENTS = {
    "release_basis_hash",
    "normal_form_coverage_proof",
    "generated_candidate_partition_hash",
    "feasibility_proof_per_partition",
    "conditional_static_lower_bound_per_dimension",
    "witness_static_cost_equals_lower_bounds",
    "declared_workload_environment_matrix_hash",
    "noise_gated_empirical_frontier_evidence",
    "conditional_complexity_lower_bound_per_dimension",
    "witness_complexity_cost_equals_lower_bounds",
    "production_refinement_and_cost_binding",
    "independent_negative_certificate_canaries",
}
REQUIRED_OPTIMALITY_RELEASE_GATE = {
    "required_claim": "globally_optimal_within_declared_model_and_empirical_matrix",
    "uncertified_disposition": "release_blocker",
    "degradation_path": "forbidden_without_explicit_user_goal_change",
    "coverage_owner_phase": "architecture_optimality_synthesis",
    "certificate_owner_phase": "constructive_simplification",
    "review_owner_phase": "portability_and_final_review",
}
REQUIRED_STATIC_LOWER_BOUND_RULE = {
    "normalized_cost": "J_static_B(n)=absolute_static_cost(n)-L_B",
    "codomain": "Nat^7_in_optimization_goal_static_objective_order",
    "common_floor": "L_B_is_release_law_implied_and_shared_by_every_X0_normal_form",
    "conditional_rule": "minimize_coordinate_i_with_prior_coordinates_fixed_to_zero",
    "lower_bound": [0, 0, 0, 0, 0, 0, 0],
    "absolute_cost_binding": "production_refinement_and_cost_binding",
}
REQUIRED_EMPIRICAL_SINGLETON_RULE = {
    "premise": "X1_cardinality_equals_one",
    "theorem": "for_every_empirical_objective_argmin_over_X1_equals_X1",
    "construction_measurement_universe": (
        "empty_by_measurement_record_admission"
    ),
    "acceptance_obligation": "fixed_release_binary_confirmation_remains_required",
}
ARCHITECTURE_SYNTHESIS_REQUIREMENTS = {
    "release_basis_hash",
    "normal_form_coverage_proof",
    "generated_candidate_partition_hash",
    "feasibility_proof_per_partition",
    "conditional_static_lower_bound_per_dimension",
    "witness_static_cost_equals_lower_bounds",
    "declared_workload_environment_matrix_hash",
    "noise_gated_empirical_frontier_evidence",
}
SIMPLIFICATION_REQUIREMENTS = (
    REQUIRED_OPTIMALITY_CERTIFICATE_REQUIREMENTS
    - ARCHITECTURE_SYNTHESIS_REQUIREMENTS
)
REQUIRED_RUST_API_DISPOSITIONS = {
    "non_authoritative_facade",
    "intentional_major",
    "revert",
}
REQUIRED_RUST_API_LANDING_EVIDENCE = {
    "candidate_major_version_relation",
    "release_migration_notes",
    "generated_workspace_reverse_dependency_build_and_tests",
    "rehearsal_against_current_official_develop",
}
REQUIRED_LANDING_GENERATORS = {
    "workspace_reverse_dependencies": (
        "cargo_metadata_reverse_dependency_closure",
        "changed_workspace_packages_against_target",
    ),
    "managed_integration_impact": (
        "registered_integration_impact_closure",
        "changed_paths_against_target",
    ),
    "release_surface_consumers": (
        "release_surface_anchor_closure",
        "release_surface.anchors",
    ),
    "merge_conflict_surface": (
        "git_merge_tree_conflict_closure",
        "candidate_and_target_trees",
    ),
}
REQUIRED_LANDING_FEASIBILITY = {
    "reconciles_the_same_frozen_X3_witness_and_final_tree",
    "all_release_surface_version_migration_and_recovery_requirements_hold",
    "zero_unresolved_semantic_conflicts",
    "generated_downstream_universe_builds_and_tests",
    "local_rehearsal_is_recoverable_and_grants_no_push_merge_or_release_authority",
}
REQUIRED_LANDING_COST_OBJECTIVE = [
    "residual_semantic_conflict_and_migration_risk",
    "maximum_unvalidated_semantic_delta",
    "migration_and_recovery_cut_count",
    "conflict_resolution_operation_count",
    "downstream_build_and_test_repetition_count",
    "review_dependency_edge_count",
    "history_rewrite_operation_count",
    "canonical_candidate_order_index",
]
REQUIRED_TIMED_SCENARIOS = {
    "independent_cheap": {"always_success"},
    "independent_crypto": {"secp256k1"},
    "dependency_frontiers": {
        "dependent_forest_10",
        "dependent_forest_10_reverse",
        "fanout",
        "fanout_reverse",
        "always_success_fanin_8",
    },
}
REQUIRED_ADVERSARIAL_SHAPES = {
    "rbf_conflict": {
        "accepted_victim_closure",
        "rejected_candidate",
        "winner_failure_and_history_recovery",
    },
    "full_pool_eviction": {
        "near_configured_limits",
        "hostile_causal_closure",
        "one_over_each_declared_bound",
    },
    "reorg": {"blank_fork", "recovered_tree", "large_bounded_fork"},
    "template": {
        "independent_suffix",
        "deep_dependency_tree",
        "conditional_cycle",
        "uncle_pressure",
    },
    "peer_pressure": {"many_owners", "large_cycle_backlog", "ban_and_refetch"},
    "shutdown": {"compute_in_flight", "effect_io_in_flight", "reorg_in_flight"},
}
REQUIRED_MATRIX_ENVIRONMENT = {
    "scope": "one_content_addressed_controlled_host_class_per_record",
    "required_fingerprint_fields": [
        "source_revision_and_tracked_diff",
        "binary_harness_lockfile_and_workspace_hashes",
        "toolchain_target_features_and_build_flags",
        "logical_cpu_platform_kernel_and_filesystem",
        "power_thermal_and_competing_load_state",
        "scenario_command_and_target_window",
    ],
    "binary_rule": "build_each_side_once_then_reuse_the_exact_hash",
}
REQUIRED_MATRIX_NOISE_POLICY = {
    "schedule": "adjacent_balanced_ab_ba",
    "minimum_pairs": 6,
    "diagnostic_max_paired_relative_mad_basis_points": 200,
    "selection_and_acceptance_max_paired_relative_mad_basis_points": 150,
    "outlier_deletion": "forbidden",
    "repeat_until_favorable": "forbidden",
    "mismatched_or_noisy_record": "invalid_not_pass",
}
REQUIRED_MATRIX_MEASUREMENT_RECORD_PROTOCOL = {
    "schema_version": 1,
    "input_identity": {
        "deterministic_runner": "scenario_parameters_and_runner_source_sha256",
        "generated_or_randomized": "fixture_or_generator_sha256_and_explicit_seed",
    },
    "construction": {
        "admission": "only_when_X1_has_multiple_static_minimizers",
        "purpose": "select_X2_only_among_X1",
        "required_bindings": [
            "record_role",
            "release_basis_sha256",
            "candidate_partition_sha256",
            "X1_candidate_binary_sha256",
            "matrix_sha256",
            "runner_source_sha256",
            "environment_fingerprint_values",
            "scenario_parameters",
            "fixture_or_generator_sha256",
            "seed_if_randomized",
            "raw_samples_sha256",
        ],
        "inadmissible_as": ["release_acceptance_evidence"],
    },
    "acceptance": {
        "admission": "only_after_complete_correctness_on_frozen_universe",
        "purpose": "confirm_frozen_X2_release_binary",
        "required_bindings": [
            "record_role",
            "acceptance_universe_sha256",
            "frozen_X2_witness",
            "release_binary_sha256",
            "matrix_sha256",
            "runner_source_sha256",
            "environment_fingerprint_values",
            "scenario_parameters",
            "fixture_or_generator_sha256",
            "seed_if_randomized",
            "raw_samples_sha256",
        ],
        "inadmissible_as": ["X2_selection_evidence", "topology_repair_input"],
    },
}
MAX_MATRIX_TRANSACTIONS = 32_768
REQUIRED_OPTIMALITY_CERTIFICATE_FIELDS = {
    "release_basis_sha256",
    "candidate_partition_sha256",
    "workload_environment_matrix_sha256",
    "normal_form_coverage_evidence",
    "feasibility_evidence",
    "conditional_static_lower_bounds",
    "witness",
    "witness_static_cost",
    "empirical_frontier_evidence",
    "conditional_complexity_lower_bounds",
    "witness_complexity_cost",
    "production_refinement_evidence",
    "negative_canary_evidence",
}
MODEL_BOUNDARY_FAMILIES = {
    "acceptance_publication",
    "administration",
    "authority_read",
    "callback_publication",
    "chain_control",
    "derived_accelerator",
    "integration_test_rpc",
    "internal_fixture",
    "local_submit",
    "lifecycle_protocol",
    "persistence_protocol",
    "proposal_ingress",
    "proposal_remote_origin",
    "recent_reject_store",
    "rejection_publication",
    "relay_and_ban_settlement",
    "relay_generation_reset",
    "relay_parent_request",
    "relay_release",
    "relay_settlement",
    "remote_ingress",
    "remote_payload",
    "startup_protocol",
    "template_protocol",
    "test_accept",
    "trusted_payload",
    "verification_control",
}
REQUIRED_MODEL_CROSS_CUTTING_PROTOCOLS = {
    "ordinary_controller_request_conservation",
    "ordered_chain_request_conservation",
}
MODEL_TEST_PREFIX = "mathematical_model::"
MUTATION_SELECTOR_KINDS = {
    "all_methods",
    "method",
    "function",
    "struct_fields",
    "remaining_path",
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument(
        "--update-inventory",
        action="store_true",
        help="rewrite the manifest-declared nextest inventory before validating it",
    )
    parser.add_argument(
        "--release",
        action="store_true",
        help="also fail while the manifest contains an explicit release blocker",
    )
    parser.add_argument(
        "--integration-spec-list",
        type=Path,
        help="ckb-test --list-specs output used to verify managed integration names",
    )
    parser.add_argument(
        "--integration-only",
        action="store_true",
        help="validate only the managed integration inventory against --integration-spec-list",
    )
    return parser.parse_args()


def load_manifest(path: Path) -> dict:
    try:
        return json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise SystemExit(f"cannot load security manifest {path}: {error}") from error


def load_repo_json(value: object, field: str) -> tuple[dict | None, list[str]]:
    if not isinstance(value, str):
        return None, [f"manifest {field} must be a repository-relative path"]
    try:
        path = repo_path(value)
        return json.loads(path.read_text()), []
    except (OSError, ValueError, json.JSONDecodeError) as error:
        return None, [f"cannot load {field} {value}: {error}"]


def _string_set(value: object) -> set[str]:
    if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
        return set()
    return set(value)


def normalized_prose(value: str) -> str:
    """Compare release prose independently of Markdown line wrapping."""

    return " ".join(value.split())


def rust_enum_variants(path_value: str, enum_name: str) -> tuple[set[str], list[str]]:
    """Discover the top-level variants of one ordinary Rust enum declaration."""

    try:
        source = repo_path(path_value).read_text()
    except (OSError, ValueError) as error:
        return set(), [f"cannot read Rust enum owner {path_value}: {error}"]
    declaration = re.search(rf"\benum\s+{re.escape(enum_name)}\b[^{{]*{{", source)
    if declaration is None:
        return set(), [f"Rust enum {enum_name} is absent from {path_value}"]
    cursor = declaration.end()
    depth = 1
    start = cursor
    while cursor < len(source) and depth:
        if source[cursor] == "{":
            depth += 1
        elif source[cursor] == "}":
            depth -= 1
        cursor += 1
    if depth:
        return set(), [f"Rust enum {enum_name} has no closing brace in {path_value}"]
    body = source[start : cursor - 1]
    variants: set[str] = set()
    depth = 0
    segment_start = 0
    for index, character in enumerate(body):
        if character in "({[<":
            depth += 1
        elif character in ")}]>":
            depth = max(depth - 1, 0)
        elif character == "," and depth == 0:
            segment = body[segment_start:index]
            segment_start = index + 1
            names = re.findall(r"(?m)^\s*(?:#\[[^\n]*\]\s*)*([A-Z][A-Za-z0-9_]*)\b", segment)
            if names:
                variants.add(names[0])
    tail = body[segment_start:]
    names = re.findall(r"(?m)^\s*(?:#\[[^\n]*\]\s*)*([A-Z][A-Za-z0-9_]*)\b", tail)
    if names:
        variants.add(names[0])
    if not variants:
        return set(), [f"Rust enum {enum_name} has no discovered variants in {path_value}"]
    return variants, []


def require_source_symbols(
    path_value: str, symbols: list[str], description: str
) -> list[str]:
    try:
        source = repo_path(path_value).read_text()
    except (OSError, ValueError) as error:
        return [f"cannot read {description} owner {path_value}: {error}"]
    return [
        f"{description} symbol {symbol!r} is absent from {path_value}"
        for symbol in symbols
        if symbol not in source
    ]


def _nonempty_unique_strings(value: object) -> bool:
    return (
        isinstance(value, list)
        and bool(value)
        and all(isinstance(item, str) and bool(item.strip()) for item in value)
        and len(value) == len(set(value))
    )


def _validate_planned_owner(
    value: object, description: str, require_symbols: bool
) -> list[str]:
    if not isinstance(value, dict):
        return [f"{description} must be an object"]
    path_value = value.get("path")
    symbols = value.get("symbols")
    if not isinstance(path_value, str) or not _nonempty_unique_strings(symbols):
        return [f"{description} must define one path and unique symbols"]
    try:
        path = repo_path(path_value)
    except ValueError as error:
        return [str(error)]
    if require_symbols:
        return require_source_symbols(path_value, symbols, description)
    if not path.parent.is_dir():
        return [f"{description} parent directory is absent for {path_value}"]
    return []


def resolve_mutation_owner(
    owner_ref: object, contract: dict, registry: dict
) -> tuple[dict | None, list[str]]:
    """Resolve one semantic owner reference without copying its path."""

    if not isinstance(owner_ref, dict):
        return None, ["mutation obligation owner_ref must be an object"]
    kind = owner_ref.get("kind")
    symbol = owner_ref.get("symbol")
    if not isinstance(symbol, str) or not symbol.strip():
        return None, ["mutation obligation owner_ref must name one symbol"]

    if kind == "topology_target":
        component_id = owner_ref.get("component_id")
        components = contract.get("selected_topology", {}).get("components", [])
        matches = [
            component
            for component in components
            if isinstance(component, dict) and component.get("id") == component_id
        ]
        if len(matches) != 1:
            return None, [
                f"mutation topology owner references unknown component {component_id!r}"
            ]
        owner = matches[0].get("target_owner")
        if not isinstance(owner, dict) or symbol not in _string_set(owner.get("symbols")):
            return None, [
                f"mutation topology owner symbol {symbol!r} is absent from "
                f"component {component_id!r}"
            ]
        return {"path": owner.get("path"), "symbol": symbol}, []

    if kind == "behavior_owner":
        behavior_id = owner_ref.get("behavior_id")
        owners = [
            owner
            for behavior in registry.get("behaviors", [])
            if isinstance(behavior, dict) and behavior.get("id") == behavior_id
            for owner in behavior.get("implementation_owners", [])
            if isinstance(owner, dict) and symbol in _string_set(owner.get("symbols"))
        ]
        if len(owners) != 1:
            return None, [
                f"mutation behavior owner {behavior_id!r} symbol {symbol!r} "
                f"resolved {len(owners)} times"
            ]
        return {"path": owners[0].get("path"), "symbol": symbol}, []

    return None, [f"mutation obligation has unknown owner_ref kind {kind!r}"]


def validate_mutation_acceptance(
    value: object, contract: dict, registry: dict
) -> list[str]:
    """Validate semantic mutation obligations; candidate rows stay generated."""

    if not isinstance(value, dict) or value.get("schema_version") != 1:
        return ["security manifest mutation_acceptance schema_version must be 1"]
    errors: list[str] = []
    if value.get("test_target") != "lib":
        errors.append("mutation acceptance must use the complete library test target")
    obligations = value.get("obligations")
    if not isinstance(obligations, list) or not obligations:
        return errors + ["mutation acceptance obligations must be a non-empty list"]

    bindings = contract.get("refinement_inventory", {}).get("semantic_bindings", {})
    components = {
        component.get("id"): component
        for component in contract.get("selected_topology", {}).get("components", [])
        if isinstance(component, dict) and isinstance(component.get("id"), str)
    }
    seen_ids: set[str] = set()
    forbidden_derived_fields = {
        "candidate_count",
        "candidate_digest",
        "cargo_mutants_version",
        "command",
        "invariants",
        "path",
        "test_count",
        "tests",
    }
    for obligation in obligations:
        if not isinstance(obligation, dict):
            errors.append(f"invalid mutation obligation: {obligation!r}")
            continue
        obligation_id = obligation.get("id")
        if (
            not isinstance(obligation_id, str)
            or not obligation_id.startswith("V1-MUT-")
            or obligation_id in seen_ids
        ):
            errors.append(f"invalid or duplicate mutation obligation ID: {obligation_id!r}")
        else:
            seen_ids.add(obligation_id)
        copied_fields = forbidden_derived_fields.intersection(obligation)
        if copied_fields:
            errors.append(
                f"mutation obligation {obligation_id!r} copies generated fields: "
                f"{sorted(copied_fields)}"
            )

        binding_id = obligation.get("semantic_binding")
        binding = bindings.get(binding_id) if isinstance(bindings, dict) else None
        if not isinstance(binding, dict):
            errors.append(
                f"mutation obligation {obligation_id!r} references unknown semantic "
                f"binding {binding_id!r}"
            )
            binding = {}

        component_id = obligation.get("component_id")
        component = components.get(component_id) if component_id is not None else None
        if component_id is not None and component is None:
            errors.append(
                f"mutation obligation {obligation_id!r} references unknown component "
                f"{component_id!r}"
            )
        if component is not None and not _string_set(component.get("behavior_ids")).intersection(
            _string_set(binding.get("behavior_ids"))
        ):
            errors.append(
                f"mutation obligation {obligation_id!r} component and semantic binding "
                "share no behavior law"
            )

        owner, owner_errors = resolve_mutation_owner(
            obligation.get("owner_ref"), contract, registry
        )
        errors.extend(owner_errors)
        owner_ref = obligation.get("owner_ref")
        if isinstance(owner_ref, dict) and owner_ref.get("kind") == "behavior_owner":
            behavior_id = owner_ref.get("behavior_id")
            if behavior_id not in _string_set(binding.get("behavior_ids")):
                errors.append(
                    f"mutation obligation {obligation_id!r} owner behavior "
                    f"{behavior_id!r} is absent from semantic binding {binding_id!r}"
                )
        if (
            isinstance(owner_ref, dict)
            and owner_ref.get("kind") == "topology_target"
            and owner_ref.get("component_id") != component_id
        ):
            errors.append(
                f"mutation obligation {obligation_id!r} topology owner and component differ"
            )
        if owner is not None:
            path_value = owner.get("path")
            symbol = owner.get("symbol")
            if not isinstance(path_value, str) or not isinstance(symbol, str):
                errors.append(
                    f"mutation obligation {obligation_id!r} resolved an invalid owner"
                )
            else:
                errors.extend(
                    require_source_symbols(
                        path_value, [symbol], f"mutation obligation {obligation_id}"
                    )
                )

        selector = obligation.get("selector")
        if not isinstance(selector, dict) or selector.get("kind") not in MUTATION_SELECTOR_KINDS:
            errors.append(
                f"mutation obligation {obligation_id!r} has an invalid selector"
            )
            continue
        selector_kind = selector.get("kind")
        selector_name = selector.get("name")
        owner_symbol = owner.get("symbol") if owner is not None else None
        if selector_kind == "method":
            if not isinstance(selector_name, str) or not re.fullmatch(
                r"[a-z][A-Za-z0-9_]*", selector_name
            ):
                errors.append(
                    f"mutation obligation {obligation_id!r} method selector needs one name"
                )
            if isinstance(owner_symbol, str) and owner_symbol.startswith("fn "):
                errors.append(
                    f"mutation obligation {obligation_id!r} cannot select a method "
                    "from a free-function owner"
                )
        elif selector_name is not None:
            errors.append(
                f"mutation obligation {obligation_id!r} non-method selector copies a name"
            )
        if selector_kind == "function" and (
            not isinstance(owner_symbol, str) or not owner_symbol.startswith("fn ")
        ):
            errors.append(
                f"mutation obligation {obligation_id!r} function selector needs a "
                "free-function owner"
            )
        if selector_kind in {"all_methods", "struct_fields"} and isinstance(
            owner_symbol, str
        ) and owner_symbol.startswith(("fn ", "enum ")):
            errors.append(
                f"mutation obligation {obligation_id!r} {selector_kind} needs a type owner"
            )
    return errors


def validate_mutation_acceptance_canaries(
    value: dict, contract: dict, registry: dict
) -> list[str]:
    """Prove that unknown, duplicate and copied mutation facts are rejected."""

    errors: list[str] = []
    unknown_binding = copy.deepcopy(value)
    unknown_binding["obligations"][0]["semantic_binding"] = "missing_binding"
    observed = validate_mutation_acceptance(unknown_binding, contract, registry)
    if not any("unknown semantic binding" in error for error in observed):
        errors.append("mutation acceptance unknown-binding canary did not fail")

    duplicate = copy.deepcopy(value)
    duplicate["obligations"].append(copy.deepcopy(duplicate["obligations"][0]))
    observed = validate_mutation_acceptance(duplicate, contract, registry)
    if not any("duplicate mutation obligation ID" in error for error in observed):
        errors.append("mutation acceptance duplicate-ID canary did not fail")

    copied_row = copy.deepcopy(value)
    copied_row["obligations"][0]["candidate_count"] = 1
    observed = validate_mutation_acceptance(copied_row, contract, registry)
    if not any("copies generated fields" in error for error in observed):
        errors.append("mutation acceptance copied-row canary did not fail")
    return errors


def validate_rust_api_compatibility(contract: dict) -> list[str]:
    """Validate the total public Rust API release decision."""

    errors: list[str] = []
    policy = contract.get("release_surface", {}).get("rust_api_compatibility")
    expected_fields = {
        "schema_version",
        "decision_function",
        "facade_constraint",
        "version_transition",
        "delta_generator_role",
        "landing_evidence_requirements",
        "landing_obligation",
    }
    if not isinstance(policy, dict) or policy.get("schema_version") != 1:
        return ["Rust API compatibility schema_version must be 1"]
    if set(policy) != expected_fields:
        errors.append("Rust API compatibility fields differ")

    decision = policy.get("decision_function")
    if not isinstance(decision, dict) or set(decision) != {
        "domain",
        "codomain",
        "operator",
        "value",
    }:
        errors.append("Rust API compatibility decision function fields differ")
    else:
        codomain = decision.get("codomain")
        if (
            not isinstance(codomain, list)
            or len(codomain) != len(set(codomain))
            or set(codomain) != REQUIRED_RUST_API_DISPOSITIONS
        ):
            errors.append("Rust API compatibility decision codomain differs")
        if (
            decision.get("domain") != "all_public_rust_api_delta"
            or decision.get("operator") != "constant"
            or decision.get("value") != "intentional_major"
        ):
            errors.append(
                "Rust API compatibility must map every public delta to intentional_major"
            )

    if policy.get("facade_constraint") != (
        "must_not_restore_removed_mutable_transaction_or_policy_authority"
    ):
        errors.append("Rust API compatibility facade constraint differs")
    if policy.get("version_transition") != {
        "baseline": "latest_published_ckb_tx_pool_version_at_landing",
        "candidate": "reconciled_release_version",
        "operator": "candidate_major_strictly_greater_than_baseline_major",
    }:
        errors.append("Rust API compatibility major-version transition differs")
    if policy.get("delta_generator_role") != "optional_diagnostic_only":
        errors.append("Rust API delta generator must remain an optional diagnostic")
    evidence = policy.get("landing_evidence_requirements")
    if (
        not isinstance(evidence, list)
        or len(evidence) != len(set(evidence))
        or set(evidence) != REQUIRED_RUST_API_LANDING_EVIDENCE
    ):
        errors.append("Rust API compatibility landing evidence requirements differ")
    if policy.get("landing_obligation") != "landing_topology":
        errors.append("Rust API compatibility must transfer evidence to landing_topology")

    try:
        cargo_toml = repo_path("tx-pool/Cargo.toml").read_text()
    except (OSError, ValueError) as error:
        errors.append(f"cannot inspect ckb-tx-pool publication boundary: {error}")
        cargo_toml = ""
    package_match = re.search(
        r"(?ms)^\[package\]\s*(.*?)(?=^\[|\Z)", cargo_toml
    )
    package = package_match.group(1) if package_match is not None else ""
    if re.search(r'(?m)^name\s*=\s*"ckb-tx-pool"\s*$', package) is None:
        errors.append("Rust API compatibility is not bound to ckb-tx-pool")
    elif re.search(r"(?m)^publish\s*=\s*(?:false|\[\s*\])\s*$", package):
        errors.append("Rust API compatibility requires a publishable ckb-tx-pool crate")
    return errors


def validate_landing_protocol(contract: dict) -> list[str]:
    """Validate the finite, goal-bound landing optimization problem."""

    errors: list[str] = []
    protocol = contract.get("landing_protocol")
    expected_fields = {
        "schema_version",
        "goal_ref",
        "owner_phase",
        "target",
        "candidate_ref",
        "downstream_universe",
        "feasibility_constraints",
        "cost_objective",
        "selection",
        "current_selection",
    }
    if not isinstance(protocol, dict) or protocol.get("schema_version") != 1:
        return ["architecture contract landing_protocol schema_version must be 1"]
    if set(protocol) != expected_fields:
        errors.append("landing protocol fields differ")
    if protocol.get("goal_ref") != "optimization_goal":
        errors.append("landing protocol must reference the canonical optimization goal")
    if protocol.get("owner_phase") != "landing_rehearsal":
        errors.append("landing protocol owner phase differs")
    if protocol.get("target") != "current_official_develop_at_rehearsal":
        errors.append("landing protocol target differs")
    if protocol.get("candidate_ref") != "convergence_protocol.landing_candidates":
        errors.append("landing protocol candidate reference differs")

    universe = protocol.get("downstream_universe")
    if not isinstance(universe, dict) or set(universe) != {"operator", "generators"}:
        errors.append("landing downstream universe fields differ")
        generators = []
    else:
        if universe.get("operator") != "set_union":
            errors.append("landing downstream universe must use set union")
        generators = universe.get("generators")
        if not isinstance(generators, list):
            errors.append("landing downstream universe generators must be a list")
            generators = []
    discovered_generators: dict[str, tuple[object, object]] = {}
    for generator in generators:
        if not isinstance(generator, dict) or set(generator) != {
            "id",
            "operator",
            "input",
        }:
            errors.append(f"invalid landing downstream generator {generator!r}")
            continue
        generator_id = generator.get("id")
        if not isinstance(generator_id, str) or generator_id in discovered_generators:
            errors.append(f"invalid or duplicate landing generator ID {generator_id!r}")
            continue
        discovered_generators[generator_id] = (
            generator.get("operator"),
            generator.get("input"),
        )
    if discovered_generators != REQUIRED_LANDING_GENERATORS:
        errors.append("landing downstream generator set differs")

    feasibility = protocol.get("feasibility_constraints")
    if (
        not isinstance(feasibility, list)
        or len(feasibility) != len(set(feasibility))
        or set(feasibility) != REQUIRED_LANDING_FEASIBILITY
    ):
        errors.append("landing feasibility constraints differ")
    if protocol.get("cost_objective") != REQUIRED_LANDING_COST_OBJECTIVE:
        errors.append("landing lexicographic cost objective differs")
    if protocol.get("selection") != [
        {
            "set": "L0",
            "operator": "filter",
            "domain_ref": "convergence_protocol.landing_candidates",
            "constraint_ref": "feasibility_constraints",
        },
        {
            "set": "L1",
            "operator": "lexicographic_argmin",
            "domain_ref": "L0",
            "objective_ref": "cost_objective",
        },
    ]:
        errors.append("landing L0/L1 selection algebra differs")

    selected = protocol.get("current_selection")
    if selected is not None:
        selected_fields = {
            "candidate",
            "target_revision",
            "final_tree_sha256",
            "downstream_universe_sha256",
            "candidate_matrix_sha256",
            "cost_vector",
            "rehearsal_evidence_sha256",
        }
        if not isinstance(selected, dict) or set(selected) != selected_fields:
            errors.append("landing current selection certificate fields differ")
        else:
            candidates = _string_set(
                contract.get("convergence_protocol", {}).get("landing_candidates")
            )
            if selected.get("candidate") not in candidates:
                errors.append("landing current selection uses an unknown candidate")
            if re.fullmatch(r"[0-9a-f]{40}", str(selected.get("target_revision"))) is None:
                errors.append("landing current selection target revision is invalid")
            for field in (
                "final_tree_sha256",
                "downstream_universe_sha256",
                "candidate_matrix_sha256",
                "rehearsal_evidence_sha256",
            ):
                if re.fullmatch(r"[0-9a-f]{64}", str(selected.get(field))) is None:
                    errors.append(f"landing current selection {field} is invalid")
            cost = selected.get("cost_vector")
            if (
                not isinstance(cost, list)
                or len(cost) != len(REQUIRED_LANDING_COST_OBJECTIVE)
                or not all(
                    isinstance(value, int) and not isinstance(value, bool) and value >= 0
                    for value in cost
                )
            ):
                errors.append("landing current selection cost vector is invalid")
    return errors


def validate_workload_environment_matrix(contract: dict) -> list[str]:
    """Validate the finite X2 and fixed-binary confirmation matrix."""

    errors: list[str] = []
    matrix = contract.get("declared_workload_environment_matrix")
    if not isinstance(matrix, dict) or matrix.get("schema_version") != 2:
        return ["declared workload/environment matrix schema_version must be 2"]
    if set(matrix) != {
        "schema_version",
        "objective_ref",
        "phase_roles",
        "measurement_record_protocol",
        "timed_families",
        "adversarial_families",
        "required_observations",
        "environment",
        "noise_policy",
    }:
        errors.append("declared workload/environment matrix fields differ")
    if matrix.get("objective_ref") != "optimization_goal.empirical_objective":
        errors.append("workload matrix empirical objective reference differs")
    if matrix.get("phase_roles") != {
        "construction": "select_X2_only_among_statically_minimal_X1_candidates",
        "acceptance": "confirm_the_frozen_X2_witness_without_topology_repair",
    }:
        errors.append("workload matrix phase roles differ")
    if (
        matrix.get("measurement_record_protocol")
        != REQUIRED_MATRIX_MEASUREMENT_RECORD_PROTOCOL
    ):
        errors.append("workload matrix measurement record protocol differs")

    timed = matrix.get("timed_families")
    timed_by_id: dict[str, dict] = {}
    if not isinstance(timed, list):
        errors.append("timed workload families must be a list")
        timed = []
    for family in timed:
        if not isinstance(family, dict) or set(family) != {
            "id",
            "runner",
            "scenarios",
            "pool_populations",
            "peers",
            "workers",
            "target_transactions",
        }:
            errors.append(f"invalid timed workload family {family!r}")
            continue
        family_id = family.get("id")
        if not isinstance(family_id, str) or family_id in timed_by_id:
            errors.append(f"invalid or duplicate timed workload family {family_id!r}")
            continue
        timed_by_id[family_id] = family
    if set(timed_by_id) != set(REQUIRED_TIMED_SCENARIOS):
        errors.append("timed workload family universe differs")
    for family_id, expected_scenarios in REQUIRED_TIMED_SCENARIOS.items():
        family = timed_by_id.get(family_id, {})
        scenarios = family.get("scenarios")
        if (
            family.get("runner") != "fixed_binary_one_shot"
            or not _nonempty_unique_strings(scenarios)
            or set(scenarios) != expected_scenarios
        ):
            errors.append(f"timed workload family {family_id} scenarios differ")
        for field, expected in (("peers", {1, 4}), ("workers", {1, 8})):
            values = family.get(field)
            if (
                not isinstance(values, list)
                or not all(
                    isinstance(value, int) and not isinstance(value, bool)
                    for value in values
                )
                or len(values) != len(set(values))
                or set(values) != expected
            ):
                errors.append(
                    f"timed workload family {family_id} scaling dimension {field} differs"
                )
        targets = family.get("target_transactions")
        if (
            not isinstance(targets, list)
            or not targets
            or not all(
                isinstance(value, int)
                and not isinstance(value, bool)
                and 0 < value <= MAX_MATRIX_TRANSACTIONS
                for value in targets
            )
            or len(targets) != len(set(targets))
        ):
            errors.append(f"timed workload family {family_id} target sizes are invalid")
            targets = []
        populations = family.get("pool_populations")
        population_by_state: dict[str, int] = {}
        if not isinstance(populations, list):
            populations = []
        for population in populations:
            if not isinstance(population, dict) or set(population) != {
                "state",
                "warm_transactions",
            }:
                errors.append(f"timed workload family {family_id} population is invalid")
                continue
            state = population.get("state")
            warm = population.get("warm_transactions")
            if (
                state not in {"cold", "warm"}
                or state in population_by_state
                or not isinstance(warm, int)
                or isinstance(warm, bool)
                or warm < 0
                or (state == "cold" and warm != 0)
                or (state == "warm" and warm == 0)
            ):
                errors.append(f"timed workload family {family_id} population differs")
                continue
            population_by_state[state] = warm
        expected_states = {"cold"} if family_id == "dependency_frontiers" else {
            "cold",
            "warm",
        }
        if set(population_by_state) != expected_states:
            errors.append(f"timed workload family {family_id} pool states differ")
        if targets and any(
            target + warm > MAX_MATRIX_TRANSACTIONS
            for target in targets
            for warm in population_by_state.values()
        ):
            errors.append(f"timed workload family {family_id} exceeds the runner bound")

    adversarial = matrix.get("adversarial_families")
    adversarial_by_id: dict[str, set[str]] = {}
    if not isinstance(adversarial, list):
        errors.append("adversarial workload families must be a list")
        adversarial = []
    for family in adversarial:
        if not isinstance(family, dict) or set(family) != {"id", "shapes"}:
            errors.append(f"invalid adversarial workload family {family!r}")
            continue
        family_id = family.get("id")
        shapes = family.get("shapes")
        if (
            not isinstance(family_id, str)
            or family_id in adversarial_by_id
            or not _nonempty_unique_strings(shapes)
        ):
            errors.append(f"invalid or duplicate adversarial workload family {family_id!r}")
            continue
        adversarial_by_id[family_id] = set(shapes)
    if adversarial_by_id != REQUIRED_ADVERSARIAL_SHAPES:
        errors.append("adversarial workload family universe differs")

    if matrix.get("required_observations") != contract.get(
        "optimization_goal", {}
    ).get("empirical_objective"):
        errors.append("workload matrix required observations differ from the goal")
    if matrix.get("environment") != REQUIRED_MATRIX_ENVIRONMENT:
        errors.append("workload matrix environment fingerprint contract differs")
    if matrix.get("noise_policy") != REQUIRED_MATRIX_NOISE_POLICY:
        errors.append("workload matrix noise policy differs")
    return errors


def validate_release_protocol_canaries(contract: dict) -> list[str]:
    """Prove release choices cannot regress to tool, facade or prose authority."""

    errors: list[str] = []
    facade = copy.deepcopy(contract)
    facade["release_surface"]["rust_api_compatibility"]["decision_function"][
        "value"
    ] = "non_authoritative_facade"
    observed = validate_rust_api_compatibility(facade)
    if not any("map every public delta" in error for error in observed):
        errors.append("Rust API release canary admitted a compatibility facade")

    tool_gate = copy.deepcopy(contract)
    tool_gate["release_surface"]["rust_api_compatibility"][
        "delta_generator_role"
    ] = "required_gate"
    observed = validate_rust_api_compatibility(tool_gate)
    if not any("optional diagnostic" in error for error in observed):
        errors.append("Rust API release canary made a delta tool authoritative")

    incomplete_landing = copy.deepcopy(contract)
    incomplete_landing["landing_protocol"]["downstream_universe"]["generators"].pop()
    observed = validate_landing_protocol(incomplete_landing)
    if not any("generator set differs" in error for error in observed):
        errors.append("landing canary admitted an incomplete downstream universe")

    reordered_cost = copy.deepcopy(contract)
    objective = reordered_cost["landing_protocol"]["cost_objective"]
    objective[0], objective[1] = objective[1], objective[0]
    observed = validate_landing_protocol(reordered_cost)
    if not any("cost objective differs" in error for error in observed):
        errors.append("landing canary admitted a reordered cost objective")

    weakened_selection = copy.deepcopy(contract)
    weakened_selection["landing_protocol"]["selection"][1]["operator"] = "filter"
    observed = validate_landing_protocol(weakened_selection)
    if not any("selection algebra differs" in error for error in observed):
        errors.append("landing canary admitted a non-optimal selection operator")

    incomplete_matrix = copy.deepcopy(contract)
    incomplete_matrix["declared_workload_environment_matrix"][
        "adversarial_families"
    ].pop()
    observed = validate_workload_environment_matrix(incomplete_matrix)
    if not any("adversarial workload family universe differs" in error for error in observed):
        errors.append("workload-matrix canary admitted an omitted adversarial family")

    weakened_noise = copy.deepcopy(contract)
    weakened_noise["declared_workload_environment_matrix"]["noise_policy"][
        "selection_and_acceptance_max_paired_relative_mad_basis_points"
    ] = 999
    observed = validate_workload_environment_matrix(weakened_noise)
    if not any("noise policy differs" in error for error in observed):
        errors.append("workload-matrix canary admitted a weakened noise gate")

    reused_construction_record = copy.deepcopy(contract)
    record_protocol = reused_construction_record[
        "declared_workload_environment_matrix"
    ]["measurement_record_protocol"]
    record_protocol["acceptance"] = copy.deepcopy(record_protocol["construction"])
    observed = validate_workload_environment_matrix(reused_construction_record)
    if not any("measurement record protocol differs" in error for error in observed):
        errors.append(
            "workload-matrix canary admitted construction evidence as acceptance"
        )
    return errors


def validate_selected_topology(contract: dict, registry: dict) -> list[str]:
    errors: list[str] = []
    topology = contract.get("selected_topology")
    slices_contract = contract.get("implementation_slices")
    release_surface = contract.get("release_surface")
    if not isinstance(topology, dict) or topology.get("schema_version") != 1:
        return ["architecture contract selected_topology schema_version must be 1"]
    if not isinstance(slices_contract, dict) or slices_contract.get("schema_version") != 1:
        return ["architecture contract implementation_slices schema_version must be 1"]
    if not isinstance(release_surface, dict) or release_surface.get("schema_version") != 3:
        return ["architecture contract release_surface schema_version must be 3"]
    if release_surface.get("compatibility_policy") != REQUIRED_RELEASE_COMPATIBILITY_POLICY:
        errors.append(
            "release surface must define the exact forward-only node, configuration "
            "and persistence compatibility policy"
        )
    if topology.get("status") not in {"normative_blueprint", "implemented"}:
        errors.append("selected topology has an invalid status")
    if topology.get("authority") != contract.get("authority", {}).get("transaction_owner"):
        errors.append("selected topology must reuse the sole transaction authority")

    inventory = contract.get("refinement_inventory", {})
    model_roots = inventory.get("model_roots", {})
    model_roles = set(model_roots.values()) if isinstance(model_roots, dict) else set()
    invariants = target_invariant_ids(contract)
    behavior_ids = {
        entry.get("id")
        for entry in registry.get("behaviors", [])
        if isinstance(entry, dict) and isinstance(entry.get("id"), str)
    }
    unit_evidence = {
        entry.get("test"): entry
        for entry in registry.get("unit_evidence", [])
        if isinstance(entry, dict) and isinstance(entry.get("test"), str)
    }

    allowed_statuses = _string_set(slices_contract.get("allowed_statuses"))
    if allowed_statuses != {"blueprint", "implemented"}:
        errors.append("implementation slices must allow exactly blueprint and implemented")
    slices = slices_contract.get("slices")
    if not isinstance(slices, list) or not slices:
        errors.append("implementation slices must be a non-empty list")
        slices = []
    component_statuses: dict[str, str] = {}
    seen_slice_ids: set[str] = set()
    sequences: list[int] = []
    for entry in slices:
        if not isinstance(entry, dict):
            errors.append(f"invalid implementation slice: {entry!r}")
            continue
        slice_id = entry.get("id")
        sequence = entry.get("sequence")
        status = entry.get("status")
        component_ids = entry.get("component_ids")
        paths = entry.get("production_paths")
        if not isinstance(slice_id, str) or not slice_id.strip() or slice_id in seen_slice_ids:
            errors.append(f"implementation slice has invalid or duplicate ID: {slice_id!r}")
        else:
            seen_slice_ids.add(slice_id)
        if not isinstance(sequence, int) or isinstance(sequence, bool) or sequence <= 0:
            errors.append(f"implementation slice {slice_id!r} has invalid sequence")
        else:
            sequences.append(sequence)
        if status not in allowed_statuses:
            errors.append(f"implementation slice {slice_id!r} has invalid status {status!r}")
        if not _nonempty_unique_strings(component_ids):
            errors.append(f"implementation slice {slice_id!r} has invalid component IDs")
            component_ids = []
        for component_id in component_ids:
            if component_id in component_statuses:
                errors.append(f"topology component {component_id!r} belongs to multiple slices")
            elif isinstance(status, str):
                component_statuses[component_id] = status
        if not _nonempty_unique_strings(paths):
            errors.append(f"implementation slice {slice_id!r} has invalid production paths")
        else:
            for path_value in paths:
                try:
                    path = repo_path(path_value)
                except ValueError as error:
                    errors.append(str(error))
                    continue
                if not path.exists() and not path.parent.is_dir():
                    errors.append(
                        f"implementation slice {slice_id!r} path has no existing parent: {path_value}"
                    )
        if not isinstance(entry.get("exit_gate"), str) or not entry["exit_gate"].strip():
            errors.append(f"implementation slice {slice_id!r} has no exit gate")
    if sorted(sequences) != list(range(1, len(slices) + 1)):
        errors.append("implementation slice sequences must be contiguous from one")

    components = topology.get("components")
    if not isinstance(components, list) or not components:
        errors.append("selected topology components must be a non-empty list")
        components = []
    seen_component_ids: set[str] = set()
    expected_cost_fields = {
        "authority_owners_added",
        "authority_locks_added",
        "tasks_added",
        "channel_instances_bound",
        "protocol_state_bound",
        "transient_bound",
        "apply_bound",
    }
    for component in components:
        if not isinstance(component, dict):
            errors.append(f"invalid topology component: {component!r}")
            continue
        component_id = component.get("id")
        if (
            not isinstance(component_id, str)
            or not component_id.strip()
            or component_id in seen_component_ids
        ):
            errors.append(f"topology component has invalid or duplicate ID: {component_id!r}")
            continue
        seen_component_ids.add(component_id)
        status = component_statuses.get(component_id)
        if status is None:
            errors.append(f"topology component {component_id!r} has no implementation slice")
            status = "blueprint"
        disposition = component.get("disposition")
        if disposition not in {"implement", "retain"}:
            errors.append(f"topology component {component_id!r} has invalid disposition")
        configured_roles = _string_set(component.get("model_roles"))
        if not _nonempty_unique_strings(component.get("model_roles")):
            errors.append(f"topology component {component_id!r} has invalid model roles")
        unknown_roles = configured_roles.difference(model_roles)
        if unknown_roles:
            errors.append(
                f"topology component {component_id!r} uses unknown model roles: {sorted(unknown_roles)}"
            )
        configured_invariants = _string_set(component.get("invariants"))
        if not _nonempty_unique_strings(component.get("invariants")):
            errors.append(f"topology component {component_id!r} has invalid invariants")
        unknown_invariants = configured_invariants.difference(invariants)
        if unknown_invariants:
            errors.append(
                f"topology component {component_id!r} uses unknown invariants: {sorted(unknown_invariants)}"
            )
        configured_behaviors = _string_set(component.get("behavior_ids"))
        if not _nonempty_unique_strings(component.get("behavior_ids")):
            errors.append(f"topology component {component_id!r} has invalid behaviors")
        unknown_behaviors = configured_behaviors.difference(behavior_ids)
        if unknown_behaviors:
            errors.append(
                f"topology component {component_id!r} uses unknown behaviors: {sorted(unknown_behaviors)}"
            )
        falsifiers = component.get("falsifier_tests")
        if not _nonempty_unique_strings(falsifiers):
            errors.append(f"topology component {component_id!r} has invalid falsifiers")
            falsifiers = []
        covered: set[str] = set()
        for test in falsifiers:
            evidence = unit_evidence.get(test)
            if evidence is None:
                errors.append(f"topology component {component_id!r} has unknown falsifier {test!r}")
                continue
            if evidence.get("behavior_id") not in configured_behaviors:
                errors.append(
                    f"topology component {component_id!r} falsifier {test!r} belongs to an unrelated behavior"
                )
            covered.update(_string_set(evidence.get("invariants")))
        missing_coverage = configured_invariants.difference(covered)
        if missing_coverage:
            errors.append(
                f"topology component {component_id!r} lacks exact falsifiers for {sorted(missing_coverage)}"
            )
        errors.extend(
            _validate_planned_owner(
                component.get("target_owner"),
                f"topology component {component_id!r} target owner",
                disposition == "retain" or status == "implemented",
            )
        )
        current_owners = component.get("current_owners")
        if not isinstance(current_owners, list) or not current_owners:
            errors.append(f"topology component {component_id!r} has no current owner anchors")
        elif disposition == "retain" or status == "blueprint":
            for index, owner in enumerate(current_owners):
                errors.extend(
                    _validate_planned_owner(
                        owner,
                        f"topology component {component_id!r} current owner {index}",
                        True,
                    )
                )
        cost = component.get("cost")
        if not isinstance(cost, dict) or set(cost) != expected_cost_fields:
            errors.append(f"topology component {component_id!r} has an incomplete cost ledger")
        else:
            for field in {
                "authority_owners_added",
                "authority_locks_added",
                "tasks_added",
            }:
                value = cost[field]
                if not isinstance(value, int) or isinstance(value, bool) or value < 0:
                    errors.append(
                        f"topology component {component_id!r} cost {field!r} must be a non-negative integer"
                    )
            for field in (
                "channel_instances_bound",
                "protocol_state_bound",
                "transient_bound",
                "apply_bound",
            ):
                if not isinstance(cost[field], str) or not cost[field].strip():
                    errors.append(
                        f"topology component {component_id!r} cost {field!r} must be explicit"
                    )
    unknown_slice_components = set(component_statuses).difference(seen_component_ids)
    if unknown_slice_components:
        errors.append(
            f"implementation slices use unknown topology components: {sorted(unknown_slice_components)}"
        )

    alternatives = topology.get("rejected_alternatives")
    if not isinstance(alternatives, list) or not alternatives:
        errors.append("selected topology must record rejected alternatives")
        alternatives = []
    seen_alternatives: set[str] = set()
    for alternative in alternatives:
        if not isinstance(alternative, dict):
            errors.append(f"invalid rejected topology alternative: {alternative!r}")
            continue
        alternative_id = alternative.get("id")
        if (
            not isinstance(alternative_id, str)
            or not alternative_id.strip()
            or alternative_id in seen_alternatives
        ):
            errors.append(f"rejected topology has invalid or duplicate ID: {alternative_id!r}")
        else:
            seen_alternatives.add(alternative_id)
        if not isinstance(alternative.get("reason"), str) or not alternative["reason"].strip():
            errors.append(f"rejected topology {alternative_id!r} has no reason")
        falsifiers = alternative.get("falsifier_tests")
        if not _nonempty_unique_strings(falsifiers):
            errors.append(f"rejected topology {alternative_id!r} has invalid falsifiers")
        else:
            for test in falsifiers:
                if test not in unit_evidence:
                    errors.append(
                        f"rejected topology {alternative_id!r} has unknown falsifier {test!r}"
                    )

    anchors = release_surface.get("anchors")
    if not isinstance(anchors, list) or not anchors:
        errors.append("release surface must contain anchors")
        anchors = []
    seen_anchor_ids: set[str] = set()
    for anchor in anchors:
        if not isinstance(anchor, dict):
            errors.append(f"invalid release-surface anchor: {anchor!r}")
            continue
        anchor_id = anchor.get("id")
        if (
            not isinstance(anchor_id, str)
            or not anchor_id.strip()
            or anchor_id in seen_anchor_ids
        ):
            errors.append(f"release surface has invalid or duplicate ID: {anchor_id!r}")
        else:
            seen_anchor_ids.add(anchor_id)
        files = anchor.get("files")
        if not isinstance(files, list) or not files:
            errors.append(f"release surface {anchor_id!r} has no files")
            continue
        for file_entry in files:
            if not isinstance(file_entry, dict):
                errors.append(f"release surface {anchor_id!r} has invalid file entry")
                continue
            path_value = file_entry.get("path")
            phrases = file_entry.get("required_phrases")
            if not isinstance(path_value, str) or not _nonempty_unique_strings(phrases):
                errors.append(f"release surface {anchor_id!r} has invalid path or phrases")
                continue
            try:
                contents = repo_path(path_value).read_text()
            except (OSError, ValueError) as error:
                errors.append(f"release surface {anchor_id!r} cannot read {path_value}: {error}")
                continue
            normalized_contents = normalized_prose(contents)
            for phrase in phrases:
                if normalized_prose(phrase) not in normalized_contents:
                    errors.append(
                        f"release surface {anchor_id!r} phrase {phrase!r} is absent from {path_value}"
                    )
    return errors


def validate_selected_topology_canaries(contract: dict, registry: dict) -> list[str]:
    errors: list[str] = []

    unknown_invariant = copy.deepcopy(contract)
    unknown_invariant["selected_topology"]["components"][0]["invariants"].append(
        "T999999"
    )
    observed = validate_selected_topology(unknown_invariant, registry)
    if not any("unknown invariants" in error and "T999999" in error for error in observed):
        errors.append("selected-topology validator missed its unknown-invariant canary")

    missing_target = copy.deepcopy(contract)
    retained = next(
        (
            component
            for component in missing_target["selected_topology"]["components"]
            if component.get("disposition") == "retain"
        ),
        None,
    )
    if retained is None:
        errors.append("selected-topology contract has no retained target for canary")
        return errors
    retained["target_owner"]["symbols"].append("__contract_canary_missing_symbol__")
    observed = validate_selected_topology(missing_target, registry)
    if not any("__contract_canary_missing_symbol__" in error for error in observed):
        errors.append("selected-topology validator missed its absent-target canary")

    duplicate_component = copy.deepcopy(contract)
    duplicate_id = duplicate_component["implementation_slices"]["slices"][0][
        "component_ids"
    ][0]
    duplicate_component["implementation_slices"]["slices"][1][
        "component_ids"
    ].append(duplicate_id)
    observed = validate_selected_topology(duplicate_component, registry)
    if not any("belongs to multiple slices" in error for error in observed):
        errors.append("selected-topology validator missed its duplicate-slice canary")

    missing_release_phrase = copy.deepcopy(contract)
    missing_release_phrase["release_surface"]["anchors"][0]["files"][0][
        "required_phrases"
    ].append("__contract_canary_missing_release_phrase__")
    observed = validate_selected_topology(missing_release_phrase, registry)
    if not any(
        "__contract_canary_missing_release_phrase__" in error for error in observed
    ):
        errors.append("selected-topology validator missed its release-surface canary")

    downgrade_policy = copy.deepcopy(contract)
    downgrade_policy["release_surface"]["compatibility_policy"]["node_downgrade"] = (
        "supported"
    )
    observed = validate_selected_topology(downgrade_policy, registry)
    if not any("forward-only" in error for error in observed):
        errors.append("selected-topology validator missed its downgrade-policy canary")
    return errors


def _validate_named_dag(
    rows: object, label: str, *, require_bindings: bool = False
) -> tuple[dict[str, set[str]], list[str]]:
    errors: list[str] = []
    dependencies: dict[str, set[str]] = {}
    binding_owners: dict[str, str] = {}
    if not isinstance(rows, list):
        return {}, [f"{label} must be a list"]

    for row in rows:
        if not isinstance(row, dict) or not isinstance(row.get("id"), str):
            errors.append(f"each {label} node must have one string id")
            continue
        node = row["id"]
        if node in dependencies:
            errors.append(f"duplicate {label} node {node}")
            continue
        required = row.get("requires")
        required_set = _string_set(required)
        if not isinstance(required, list) or len(required_set) != len(required):
            errors.append(f"{label} node {node} requires must be a unique string list")
        dependencies[node] = required_set

        bindings = row.get("binds")
        if require_bindings:
            binding_set = _string_set(bindings)
            if (
                not isinstance(bindings, list)
                or not binding_set
                or len(binding_set) != len(bindings)
            ):
                errors.append(f"{label} node {node} must own unique bindings")
            for binding in binding_set:
                prior = binding_owners.setdefault(binding, node)
                if prior != node:
                    errors.append(
                        f"{label} binding {binding} has owners {prior} and {node}"
                    )

    unknown = set().union(*dependencies.values()).difference(dependencies)
    if unknown:
        errors.append(f"{label} has unknown dependencies {sorted(unknown)}")

    closed: set[str] = set()
    remaining = set(dependencies)
    while remaining:
        ready = {
            node for node in remaining if dependencies[node] <= closed
        }
        if not ready:
            errors.append(f"{label} contains a cycle")
            break
        closed.update(ready)
        remaining.difference_update(ready)
    return dependencies, errors


def _descendants(dependencies: dict[str, set[str]], start: str) -> set[str]:
    result: set[str] = set()
    frontier = [start]
    while frontier:
        current = frontier.pop()
        for node, required in dependencies.items():
            if current in required and node not in result:
                result.add(node)
                frontier.append(node)
    return result


def _natural_vector(value: object) -> bool:
    return (
        isinstance(value, list)
        and bool(value)
        and all(
            isinstance(item, int) and not isinstance(item, bool) and item >= 0
            for item in value
        )
    )


def canonical_json_sha256(value: object) -> str:
    payload = json.dumps(
        value, sort_keys=True, separators=(",", ":"), ensure_ascii=True
    ).encode()
    return hashlib.sha256(payload).hexdigest()


def expected_release_basis(manifest: dict, contract: dict) -> tuple[dict, list[str]]:
    """Derive the content-addressed release-law and feasibility basis."""

    errors: list[str] = []
    convergence = contract.get("convergence_protocol", {})
    optimality = contract.get("optimality_protocol", {})
    release_laws = _string_set(convergence.get("release_law_sources"))
    feasibility = _string_set(optimality.get("feasibility_sources"))
    source_hashes: dict[str, str] = {}
    for source in sorted(release_laws.union(feasibility)):
        if source == "construction_root_families":
            value = manifest.get(source)
        else:
            value = contract.get(source)
        if value is None:
            errors.append(f"release basis source {source!r} is absent")
            continue
        source_hashes[source] = canonical_json_sha256(value)
    payload = {
        "schema_version": 1,
        "release_law_sources": sorted(release_laws),
        "feasibility_sources": sorted(feasibility),
        "source_sha256": source_hashes,
    }
    return {**payload, "basis_sha256": canonical_json_sha256(payload)}, errors


def validate_release_basis_value(
    value: object, manifest: dict, contract: dict
) -> list[str]:
    expected, errors = expected_release_basis(manifest, contract)
    if value != expected:
        errors.append("release-basis evidence differs from its generated projection")
    return errors


def validate_release_basis_evidence(manifest: dict, contract: dict) -> list[str]:
    evidence = (
        contract.get("optimality_protocol", {})
        .get("construction_evidence", {})
        .get("release_basis_hash")
    )
    if evidence is None:
        return []
    if not isinstance(evidence, dict) or not isinstance(evidence.get("path"), str):
        return ["release-basis construction evidence has no artifact path"]
    value, errors = load_repo_json(evidence["path"], "release_basis_evidence")
    if value is not None:
        errors.extend(validate_release_basis_value(value, manifest, contract))
    return errors


def expected_workload_matrix_evidence(contract: dict) -> tuple[dict, list[str]]:
    """Bind the declared matrix to its current fixed-binary runner sources."""

    errors: list[str] = []
    runner_sources = [
        "tx-pool/benches/profile_one_shot.rs",
        "tx-pool/scripts/benchmark.py",
        "tx-pool/scripts/cross_version_benchmark.py",
        "tx-pool/src/benchmark.rs",
    ]
    source_hashes: dict[str, str] = {}
    for path_value in runner_sources:
        try:
            source_hashes[path_value] = hashlib.sha256(
                repo_path(path_value).read_bytes()
            ).hexdigest()
        except (OSError, ValueError) as error:
            errors.append(f"cannot hash workload-matrix runner {path_value}: {error}")
    payload = {
        "schema_version": 1,
        "matrix_sha256": canonical_json_sha256(
            contract.get("declared_workload_environment_matrix")
        ),
        "runner_source_sha256": source_hashes,
    }
    return {**payload, "evidence_sha256": canonical_json_sha256(payload)}, errors


def validate_workload_matrix_evidence_value(
    value: object, contract: dict
) -> list[str]:
    expected, errors = expected_workload_matrix_evidence(contract)
    if value != expected:
        errors.append("workload-matrix evidence differs from its generated projection")
    return errors


def validate_workload_matrix_evidence(contract: dict) -> list[str]:
    evidence = (
        contract.get("optimality_protocol", {})
        .get("construction_evidence", {})
        .get("declared_workload_environment_matrix_hash")
    )
    if evidence is None:
        return []
    if not isinstance(evidence, dict) or not isinstance(evidence.get("path"), str):
        return ["workload-matrix construction evidence has no artifact path"]
    value, errors = load_repo_json(evidence["path"], "workload_matrix_evidence")
    if value is not None:
        errors.extend(validate_workload_matrix_evidence_value(value, contract))
    return errors


def expected_normal_form_basis_atoms(
    manifest: dict, contract: dict
) -> tuple[set[str], list[str]]:
    """Derive the semantic atom universe that the normal-form axes must cover."""

    errors: list[str] = []
    atoms: set[str] = set()

    def add_keyed(prefix: str, value: object) -> None:
        if not isinstance(value, dict) or not value:
            errors.append(f"normal-form basis source {prefix} is not a nonempty object")
            return
        atoms.update(f"{prefix}.{key}" for key in value if isinstance(key, str) and key)
        if len(atoms.intersection({f"{prefix}.{key}" for key in value})) != len(value):
            errors.append(f"normal-form basis source {prefix} has an invalid key")

    def add_named(prefix: str, value: object) -> None:
        names = _string_set(value)
        if not isinstance(value, list) or not names or len(names) != len(value):
            errors.append(f"normal-form basis source {prefix} is not a unique string list")
            return
        atoms.update(f"{prefix}.{name}" for name in names)

    goal = contract.get("optimization_goal")
    if not isinstance(goal, dict):
        errors.append("normal-form basis cannot read optimization_goal")
    else:
        atoms.update(
            {
                "optimization_goal.scope",
                "optimization_goal.concurrency_law",
                "optimization_goal.coupling_law",
                "optimization_goal.claim_boundary",
            }
        )
        for field in (
            "hard_constraints",
            "static_objective",
            "empirical_objective",
            "complexity_objective",
        ):
            add_named(f"optimization_goal.{field}", goal.get(field))

    for source in ("proof_policy", "root_families", "target_invariants"):
        add_keyed(source, contract.get(source))

    release = contract.get("release_surface")
    if not isinstance(release, dict):
        errors.append("normal-form basis cannot read release_surface")
    else:
        add_keyed(
            "release_surface.compatibility_policy",
            release.get("compatibility_policy"),
        )
        rust_api = release.get("rust_api_compatibility")
        if not isinstance(rust_api, dict):
            errors.append("normal-form basis cannot read Rust API compatibility")
        else:
            atoms.update(
                {
                    "release_surface.rust_api_compatibility.decision_function",
                    "release_surface.rust_api_compatibility.facade_constraint",
                    "release_surface.rust_api_compatibility.version_transition",
                }
            )
            add_named(
                "release_surface.rust_api_compatibility.landing_evidence_requirements",
                rust_api.get("landing_evidence_requirements"),
            )
        anchors = release.get("anchors")
        if not isinstance(anchors, list) or not anchors:
            errors.append("normal-form basis cannot read release-surface anchors")
        else:
            anchor_ids = [
                row.get("id") for row in anchors if isinstance(row, dict)
            ]
            add_named("release_surface.anchors", anchor_ids)

    landing = contract.get("landing_protocol")
    if not isinstance(landing, dict):
        errors.append("normal-form basis cannot read landing_protocol")
    else:
        downstream = landing.get("downstream_universe")
        generators = downstream.get("generators") if isinstance(downstream, dict) else None
        generator_ids = (
            [row.get("id") for row in generators if isinstance(row, dict)]
            if isinstance(generators, list)
            else None
        )
        add_named("landing_protocol.downstream_universe.generators", generator_ids)
        add_named(
            "landing_protocol.feasibility_constraints",
            landing.get("feasibility_constraints"),
        )
        add_named("landing_protocol.cost_objective", landing.get("cost_objective"))

    construction_roots = manifest.get("construction_root_families")
    if not isinstance(construction_roots, list) or not construction_roots:
        errors.append("normal-form basis cannot read construction root families")
    else:
        root_ids = [
            row.get("id") for row in construction_roots if isinstance(row, dict)
        ]
        add_named("construction_root_families", root_ids)
        members: list[object] = []
        for row in construction_roots:
            if not isinstance(row, dict) or not isinstance(row.get("members"), list):
                errors.append("normal-form basis construction root has invalid members")
                continue
            members.extend(row["members"])
        add_named("construction_root_members", members)

    add_keyed("residual_risks", contract.get("residual_risks"))
    return atoms, errors


def global_optimality_model_summary() -> tuple[dict | None, list[str]]:
    """Compile and execute the Rust normal-form model instead of mirroring it."""

    errors: list[str] = []
    model_path = repo_path("tx-pool/src/tests/model/topology.rs").resolve()
    wrapper = f'''#[path = r#"{model_path.as_posix()}"#]
mod topology;

fn main() {{
    let summary = topology::global_optimality_summary();
    println!(
        "{{{{\\\"axis_cardinalities\\\":{{:?}},\\\"total_normal_forms\\\":{{}},\\\"feasible_normal_forms\\\":{{}},\\\"rejected_normal_forms\\\":{{}},\\\"rejected_by_law\\\":{{:?}},\\\"minimum_static_extra_cost\\\":{{:?}},\\\"static_minimizers\\\":{{}},\\\"selected_static_minimizers\\\":{{}},\\\"minimum_facade_static_extra_cost\\\":{{:?}},\\\"minimum_partitioned_resource_static_extra_cost\\\":{{:?}}}}}}",
        summary.axis_cardinalities,
        summary.total_normal_forms,
        summary.feasible_normal_forms,
        summary.rejected_normal_forms,
        summary.rejected_by_law,
        summary.minimum_static_extra_cost,
        summary.static_minimizers,
        summary.selected_static_minimizers,
        summary.minimum_facade_static_extra_cost,
        summary.minimum_partitioned_resource_static_extra_cost,
    );
}}
'''
    try:
        with tempfile.TemporaryDirectory(prefix="ckb-txpool-optimality-") as temporary:
            temporary_path = Path(temporary)
            wrapper_path = temporary_path / "global_optimality.rs"
            binary_path = temporary_path / "global_optimality"
            wrapper_path.write_text(wrapper)
            compile_result = subprocess.run(
                [
                    "rustc",
                    "--edition=2024",
                    "-O",
                    str(wrapper_path),
                    "-o",
                    str(binary_path),
                ],
                cwd=REPO_ROOT,
                capture_output=True,
                text=True,
                check=False,
            )
            if compile_result.returncode != 0:
                return None, [
                    "cannot compile global optimality model: "
                    f"{compile_result.stderr.strip()}"
                ]
            run_result = subprocess.run(
                [str(binary_path)],
                cwd=REPO_ROOT,
                capture_output=True,
                text=True,
                check=False,
            )
            if run_result.returncode != 0:
                return None, [
                    "cannot execute global optimality model: "
                    f"{run_result.stderr.strip()}"
                ]
            return json.loads(run_result.stdout), errors
    except (OSError, ValueError, json.JSONDecodeError) as error:
        return None, [f"cannot evaluate global optimality model: {error}"]


def expected_normal_form_partition_evidence(
    coverage: object, manifest: dict, contract: dict
) -> tuple[dict | None, list[str]]:
    """Generate coverage, candidate partition and X0 feasibility evidence."""

    errors: list[str] = []
    axes_value = contract.get("optimality_protocol", {}).get("normal_form_axes")
    axes = _string_set(axes_value)
    if not isinstance(coverage, dict) or set(coverage) != axes:
        errors.append("normal-form basis-axis coverage differs from the declared axes")
        coverage = {}
    covered_atoms: list[str] = []
    for axis in axes_value if isinstance(axes_value, list) else []:
        assigned = coverage.get(axis)
        if not isinstance(assigned, list) or not all(
            isinstance(atom, str) and atom for atom in assigned
        ):
            errors.append(f"normal-form axis {axis} has invalid basis coverage")
            continue
        if len(assigned) != len(set(assigned)):
            errors.append(f"normal-form axis {axis} repeats a basis atom")
        covered_atoms.extend(assigned)
    if len(covered_atoms) != len(set(covered_atoms)):
        errors.append("one normal-form basis atom is owned by multiple primary axes")

    expected_atoms, atom_errors = expected_normal_form_basis_atoms(manifest, contract)
    errors.extend(atom_errors)
    covered_set = set(covered_atoms)
    missing = sorted(expected_atoms - covered_set)
    extra = sorted(covered_set - expected_atoms)
    if missing or extra:
        errors.append(
            "normal-form basis coverage differs: "
            f"missing={missing}, extra={extra}"
        )

    basis, basis_errors = expected_release_basis(manifest, contract)
    errors.extend(basis_errors)
    summary, model_errors = global_optimality_model_summary()
    errors.extend(model_errors)
    if summary is None:
        return None, errors
    law_counts = summary.get("rejected_by_law")
    if not isinstance(law_counts, list) or len(law_counts) != 3:
        return None, errors + ["global optimality model returned invalid law counts"]
    axis_cardinalities = summary.get("axis_cardinalities")
    if (
        not isinstance(axes_value, list)
        or not isinstance(axis_cardinalities, list)
        or len(axis_cardinalities) != len(axes_value)
    ):
        return None, errors + [
            "global optimality model returned invalid axis cardinalities"
        ]

    goal = contract.get("optimization_goal", {})
    static_dimensions = (
        goal.get("static_objective") if isinstance(goal, dict) else None
    )
    static_rule = contract.get("optimality_protocol", {}).get(
        "static_lower_bound_rule"
    )
    lower_bound = (
        static_rule.get("lower_bound") if isinstance(static_rule, dict) else None
    )
    minimum_cost = summary.get("minimum_static_extra_cost")
    if (
        not _nonempty_unique_strings(static_dimensions)
        or not isinstance(lower_bound, list)
        or len(lower_bound) != len(static_dimensions)
        or minimum_cost != lower_bound
    ):
        errors.append("global optimality model does not attain the static lower bound")
        static_dimensions = []
        lower_bound = []
    static_cost_by_dimension = dict(
        zip(static_dimensions, lower_bound, strict=True)
    )
    x1_cardinality = summary.get("static_minimizers")
    selected_cardinality = summary.get("selected_static_minimizers")
    if x1_cardinality != 1 or selected_cardinality != 1:
        errors.append(
            "global optimality model does not establish the singleton X1 premise"
        )

    matrix = contract.get("declared_workload_environment_matrix")
    record_protocol = (
        matrix.get("measurement_record_protocol")
        if isinstance(matrix, dict)
        else None
    )

    model_sources = [
        "tx-pool/src/tests/model/topology.rs",
        "tx-pool/src/tests/model/topology_properties.rs",
    ]
    model_hashes: dict[str, str] = {}
    for path_value in model_sources:
        try:
            model_hashes[path_value] = hashlib.sha256(
                repo_path(path_value).read_bytes()
            ).hexdigest()
        except (OSError, ValueError) as error:
            errors.append(f"cannot hash normal-form model source {path_value}: {error}")
    payload = {
        "schema_version": 1,
        "release_basis_sha256": basis.get("basis_sha256"),
        "optimization_goal_sha256": canonical_json_sha256(
            contract.get("optimization_goal")
        ),
        "static_lower_bound_rule_sha256": canonical_json_sha256(static_rule),
        "basis_atom_count": len(expected_atoms),
        "basis_axis_coverage": coverage,
        "axis_cardinalities": dict(zip(axes_value, axis_cardinalities, strict=True)),
        "candidate_partition": {
            "normal_forms": summary.get("total_normal_forms"),
            "X0_feasible": summary.get("feasible_normal_forms"),
            "rejected": summary.get("rejected_normal_forms"),
            "rejected_by_feasibility_law": dict(
                zip(
                    ["hard_constraints", "concurrency_law", "coupling_law"],
                    law_counts,
                    strict=True,
                )
            ),
        },
        "X1_static_frontier": {
            "minimum_extra_cost": minimum_cost,
            "conditional_lower_bound_by_dimension": static_cost_by_dimension,
            "selected_witness_cost_by_dimension": static_cost_by_dimension,
            "minimizers": summary.get("static_minimizers"),
            "selected_witness_minimizers": summary.get(
                "selected_static_minimizers"
            ),
            "minimum_non_authoritative_facade_extra_cost": summary.get(
                "minimum_facade_static_extra_cost"
            ),
            "minimum_partitioned_resource_extra_cost": summary.get(
                "minimum_partitioned_resource_static_extra_cost"
            ),
            "witness": contract.get("optimality_protocol", {}).get(
                "current_witness"
            ),
        },
        "X2_empirical_frontier": {
            "selection_rule": REQUIRED_EMPIRICAL_SINGLETON_RULE["theorem"],
            "X1_cardinality": x1_cardinality,
            "construction_measurement_universe": [],
            "construction_measurement_records": [],
            "declared_matrix_sha256": canonical_json_sha256(matrix),
            "measurement_record_protocol_sha256": canonical_json_sha256(
                record_protocol
            ),
            "X2_cardinality": selected_cardinality,
            "witness": contract.get("optimality_protocol", {}).get(
                "current_witness"
            ),
            "acceptance_obligation": REQUIRED_EMPIRICAL_SINGLETON_RULE[
                "acceptance_obligation"
            ],
        },
        "model_source_sha256": model_hashes,
        "proof_tests": [
            "mathematical_model::topology_properties::model_global_normal_form_partition_is_finite_unique_and_total",
            "mathematical_model::topology_properties::model_global_static_objective_has_one_zero_extra_cost_witness",
            "mathematical_model::topology_properties::model_complete_topology_selection_rejects_partial_fixes_without_stitching_exceptions",
        ],
    }
    return {**payload, "evidence_sha256": canonical_json_sha256(payload)}, errors


def validate_normal_form_partition_evidence_value(
    value: object, manifest: dict, contract: dict
) -> list[str]:
    """Reject stale or incomplete generated normal-form evidence."""

    if not isinstance(value, dict):
        return ["normal-form partition evidence must be an object"]
    expected, errors = expected_normal_form_partition_evidence(
        value.get("basis_axis_coverage"), manifest, contract
    )
    if expected is None:
        return errors
    if value != expected:
        errors.append(
            "normal-form partition evidence differs from its generated projection"
        )
    return errors


def validate_normal_form_partition_evidence(
    manifest: dict, contract: dict
) -> list[str]:
    evidence = contract.get("optimality_protocol", {}).get(
        "construction_evidence", {}
    )
    requirements = (
        "normal_form_coverage_proof",
        "generated_candidate_partition_hash",
        "feasibility_proof_per_partition",
    )
    artifacts = [evidence.get(requirement) for requirement in requirements]
    if all(artifact is None for artifact in artifacts):
        return []
    frontier_requirements = (
        "conditional_static_lower_bound_per_dimension",
        "witness_static_cost_equals_lower_bounds",
        "noise_gated_empirical_frontier_evidence",
    )
    frontier_artifacts = [
        evidence.get(requirement) for requirement in frontier_requirements
    ]
    if any(artifact is not None for artifact in frontier_artifacts):
        artifacts.extend(frontier_artifacts)
    if any(artifact is None for artifact in artifacts) or len(
        {canonical_json_sha256(artifact) for artifact in artifacts}
    ) != 1:
        return [
            "normal-form coverage, partition, feasibility and X1/X2 frontiers must share one artifact"
        ]
    artifact = artifacts[0]
    if not isinstance(artifact, dict) or not isinstance(artifact.get("path"), str):
        return ["normal-form partition construction evidence has no artifact path"]
    value, errors = load_repo_json(artifact["path"], "normal_form_partition_evidence")
    if value is not None:
        errors.extend(
            validate_normal_form_partition_evidence_value(value, manifest, contract)
        )
    return errors


def validate_optimization_goal(contract: dict) -> list[str]:
    """Validate the objective algebra without copying its semantic contents."""

    errors: list[str] = []
    goal = contract.get("optimization_goal")
    required_fields = {
        "schema_version",
        "scope",
        "hard_constraints",
        "static_objective",
        "empirical_objective",
        "complexity_objective",
        "feasibility_laws",
        "selection",
        "concurrency_law",
        "coupling_law",
        "claim_boundary",
    }
    if not isinstance(goal, dict):
        return ["architecture contract optimization_goal must be an object"]
    if set(goal) != required_fields:
        errors.append("optimization goal fields differ")
    if goal.get("schema_version") != 2:
        errors.append("optimization goal schema_version must be 2")
    if re.fullmatch(r"[a-z][a-z0-9_]+", str(goal.get("scope"))) is None:
        errors.append("optimization goal scope must be one stable identifier")

    dimension_fields = (
        "hard_constraints",
        "static_objective",
        "empirical_objective",
        "complexity_objective",
    )
    dimensions: dict[str, set[str]] = {}
    for field in dimension_fields:
        value = goal.get(field)
        if not _nonempty_unique_strings(value):
            errors.append(f"optimization goal {field} must be nonempty and unique")
            dimensions[field] = set()
        else:
            dimensions[field] = set(value)
    objective_fields = dimension_fields[1:]
    for index, left in enumerate(objective_fields):
        for right in objective_fields[index + 1 :]:
            overlap = dimensions[left].intersection(dimensions[right])
            if overlap:
                errors.append(
                    f"optimization goal dimensions overlap across {left}/{right}: "
                    f"{sorted(overlap)}"
                )

    if goal.get("feasibility_laws") != [
        "hard_constraints",
        "concurrency_law",
        "coupling_law",
    ]:
        errors.append("optimization goal feasibility law conjunction differs")

    expected_selection = [
        {
            "set": "X0",
            "operator": "filter",
            "domain_ref": "admissible_normal_forms",
            "constraint_ref": "feasibility_laws",
        },
        {
            "set": "X1",
            "operator": "lexicographic_argmin",
            "domain_ref": "X0",
            "objective_ref": "static_objective",
            "proof_ref": "conditional_static_lower_bounds",
        },
        {
            "set": "X2",
            "operator": "noise_gated_argmin",
            "domain_ref": "X1",
            "objective_ref": "empirical_objective",
            "matrix_ref": "declared_workload_environment_matrix",
        },
        {
            "set": "X3",
            "operator": "lexicographic_argmin",
            "domain_ref": "X2",
            "objective_ref": "complexity_objective",
            "proof_ref": "conditional_complexity_lower_bounds",
        },
    ]
    if goal.get("selection") != expected_selection:
        errors.append("optimization goal X0-X3 selection algebra differs")
    for field in ("concurrency_law", "coupling_law", "claim_boundary"):
        if not isinstance(goal.get(field), str) or not goal[field]:
            errors.append(f"optimization goal {field} must be nonempty")
    return errors


def validate_optimality_protocol(contract: dict) -> list[str]:
    """Reject an unscoped or uncertified final optimization claim."""

    errors: list[str] = []
    protocol = contract.get("optimality_protocol")
    if not isinstance(protocol, dict) or protocol.get("schema_version") != 5:
        return ["architecture contract optimality_protocol schema_version must be 5"]
    if set(protocol) != {
        "schema_version",
        "claim_scope",
        "admissible_domain",
        "release_gate",
        "feasibility_sources",
        "normal_form_axes",
        "objective_ref",
        "static_lower_bound_rule",
        "empirical_singleton_rule",
        "certificate_requirements",
        "construction_evidence",
        "current_witness",
        "certificate",
        "empirical_selection_phase",
        "release_binary_confirmation_phase",
        "empirical_boundary",
        "current_claim",
    }:
        errors.append("optimality protocol fields differ")
    if protocol.get("claim_scope") != (
        "semantic_topology_static_costs_empirical_frontier_and_complexity_"
        "under_explicit_release_laws"
    ):
        errors.append("optimality claim scope differs")
    if protocol.get("admissible_domain") != (
        "architectures_modulo_external_observational_equivalence"
    ):
        errors.append("optimality admissible domain differs")
    if protocol.get("release_gate") != REQUIRED_OPTIMALITY_RELEASE_GATE:
        errors.append("optimality release gate and no-degradation decision differs")
    if _string_set(protocol.get("feasibility_sources")) != (
        REQUIRED_OPTIMALITY_FEASIBILITY_SOURCES
    ):
        errors.append("optimality feasibility sources differ")
    if _string_set(protocol.get("normal_form_axes")) != (
        REQUIRED_OPTIMALITY_NORMAL_FORM_AXES
    ):
        errors.append("optimality normal-form axes differ")

    if protocol.get("objective_ref") != "optimization_goal":
        errors.append("optimality objective must reference optimization_goal")
    if protocol.get("static_lower_bound_rule") != REQUIRED_STATIC_LOWER_BOUND_RULE:
        errors.append("optimality conditional static lower-bound rule differs")
    if protocol.get("empirical_singleton_rule") != REQUIRED_EMPIRICAL_SINGLETON_RULE:
        errors.append("optimality empirical singleton theorem differs")
    if protocol.get("empirical_selection_phase") != (
        "architecture_optimality_synthesis"
    ):
        errors.append("empirical architecture selection must occur upstream")
    if protocol.get("release_binary_confirmation_phase") != (
        "empirical_performance_acceptance"
    ):
        errors.append("release binary confirmation phase differs")

    if _string_set(protocol.get("certificate_requirements")) != (
        REQUIRED_OPTIMALITY_CERTIFICATE_REQUIREMENTS
    ):
        errors.append("optimality certificate requirements differ")
    construction_evidence = protocol.get("construction_evidence")
    if (
        not isinstance(construction_evidence, dict)
        or set(construction_evidence) != REQUIRED_OPTIMALITY_CERTIFICATE_REQUIREMENTS
    ):
        errors.append("optimality construction evidence universe differs")
        construction_evidence = {}
    pending_requirements: set[str] = set()
    for requirement in REQUIRED_OPTIMALITY_CERTIFICATE_REQUIREMENTS:
        artifact = construction_evidence.get(requirement)
        if artifact is None:
            pending_requirements.add(requirement)
            continue
        if not isinstance(artifact, dict) or set(artifact) != {"path", "sha256"}:
            errors.append(
                f"optimality construction evidence {requirement} fields differ"
            )
            continue
        path_value = artifact.get("path")
        expected_hash = artifact.get("sha256")
        if (
            not isinstance(path_value, str)
            or re.fullmatch(r"[0-9a-f]{64}", str(expected_hash)) is None
        ):
            errors.append(
                f"optimality construction evidence {requirement} identity is invalid"
            )
            continue
        try:
            path = repo_path(path_value)
            actual_hash = hashlib.sha256(path.read_bytes()).hexdigest()
        except (OSError, ValueError) as error:
            errors.append(
                f"cannot read optimality construction evidence {requirement}: {error}"
            )
            continue
        if actual_hash != expected_hash:
            errors.append(
                f"optimality construction evidence {requirement} hash differs"
            )
    witness = protocol.get("current_witness")
    if witness != contract.get("selected_topology", {}).get("id"):
        errors.append("optimality witness must be the selected topology")
    if protocol.get("empirical_boundary") != (
        "construction_matrix_measurement_selects_among_static_minima;_release_"
        "binary_acceptance_confirms_without_repairing_topology;_neither_proves_"
        "universal_wall_clock_optimality"
    ):
        errors.append("optimality empirical boundary differs")

    claim = protocol.get("current_claim")
    certificate = protocol.get("certificate")
    if claim == "unproved":
        if certificate is not None:
            errors.append("unproved optimality claim cannot retain a certificate")
        if not pending_requirements:
            errors.append("complete optimality construction evidence requires certification")
        return errors
    if claim != "globally_optimal_within_declared_model_and_empirical_matrix":
        return errors + [f"unknown optimality claim {claim!r}"]
    if not isinstance(certificate, dict):
        return errors + ["global optimality claim requires one certificate"]
    if pending_requirements:
        errors.append(
            "global optimality claim retains incomplete construction evidence "
            f"{sorted(pending_requirements)}"
        )
    if set(certificate) != REQUIRED_OPTIMALITY_CERTIFICATE_FIELDS:
        errors.append("global optimality certificate fields differ")
    for field in (
        "release_basis_sha256",
        "candidate_partition_sha256",
        "workload_environment_matrix_sha256",
    ):
        if re.fullmatch(r"[0-9a-f]{64}", str(certificate.get(field))) is None:
            errors.append(f"global optimality certificate has invalid {field}")
    for field in (
        "normal_form_coverage_evidence",
        "feasibility_evidence",
        "empirical_frontier_evidence",
        "production_refinement_evidence",
        "negative_canary_evidence",
    ):
        if not _nonempty_unique_strings(certificate.get(field)):
            errors.append(f"global optimality certificate has invalid {field}")
    goal = contract.get("optimization_goal", {})
    for label, dimension_field in (
        ("static", "static_objective"),
        ("complexity", "complexity_objective"),
    ):
        lower_field = f"conditional_{label}_lower_bounds"
        witness_field = f"witness_{label}_cost"
        lower_bounds = certificate.get(lower_field)
        witness_cost = certificate.get(witness_field)
        expected_dimensions = _string_set(
            goal.get(dimension_field) if isinstance(goal, dict) else None
        )
        for field, vector in (
            (lower_field, lower_bounds),
            (witness_field, witness_cost),
        ):
            if not isinstance(vector, dict) or set(vector) != expected_dimensions:
                errors.append(f"global optimality certificate has invalid {field}")
            elif not all(_natural_vector(value) for value in vector.values()):
                errors.append(f"global optimality certificate has non-natural {field}")
        if isinstance(lower_bounds, dict) and isinstance(witness_cost, dict):
            if lower_bounds != witness_cost:
                errors.append(
                    f"global optimality witness does not attain every {label} lower bound"
                )
    if certificate.get("witness") != witness:
        errors.append("global optimality certificate names a different witness")
    return errors


def validate_optimality_canaries(contract: dict) -> list[str]:
    """Prove that wording alone cannot upgrade the current witness to optimal."""

    errors: list[str] = []
    false_claim = copy.deepcopy(contract)
    false_claim["optimality_protocol"]["current_claim"] = (
        "globally_optimal_within_declared_model_and_empirical_matrix"
    )
    observed = validate_optimality_protocol(false_claim)
    if not any("requires one certificate" in error for error in observed):
        errors.append("optimality canary admitted an uncertified global claim")

    duplicate_dimension = copy.deepcopy(contract)
    dimensions = duplicate_dimension["optimization_goal"]["static_objective"]
    dimensions.append(dimensions[0])
    observed = validate_optimization_goal(duplicate_dimension)
    if not any("static_objective must be nonempty and unique" in error for error in observed):
        errors.append("optimality canary admitted a duplicate objective dimension")

    changed_selection = copy.deepcopy(contract)
    changed_selection["optimization_goal"]["selection"][2]["operator"] = (
        "accept_first"
    )
    observed = validate_optimization_goal(changed_selection)
    if not any("X0-X3 selection algebra differs" in error for error in observed):
        errors.append("optimality canary admitted a weaker selection relation")

    uncoupled_feasibility = copy.deepcopy(contract)
    uncoupled_feasibility["optimization_goal"]["feasibility_laws"].remove(
        "coupling_law"
    )
    observed = validate_optimization_goal(uncoupled_feasibility)
    if not any("feasibility law conjunction differs" in error for error in observed):
        errors.append("optimality canary admitted a goal without the coupling law")

    late_empirical_selection = copy.deepcopy(contract)
    late_empirical_selection["optimality_protocol"]["empirical_selection_phase"] = (
        "empirical_performance_acceptance"
    )
    observed = validate_optimality_protocol(late_empirical_selection)
    if not any("must occur upstream" in error for error in observed):
        errors.append("optimality canary admitted downstream architecture selection")

    degraded_claim = copy.deepcopy(contract)
    degraded_claim["optimality_protocol"]["release_gate"]["degradation_path"] = (
        "best_known"
    )
    observed = validate_optimality_protocol(degraded_claim)
    if not any("no-degradation decision differs" in error for error in observed):
        errors.append("optimality canary admitted a silent best-known downgrade")

    fabricated_singleton_measurement = copy.deepcopy(contract)
    fabricated_singleton_measurement["optimality_protocol"][
        "empirical_singleton_rule"
    ]["construction_measurement_universe"] = "nonempty"
    observed = validate_optimality_protocol(fabricated_singleton_measurement)
    if not any("empirical singleton theorem differs" in error for error in observed):
        errors.append("optimality canary admitted measurement into a singleton X1")

    unhashed_progress = copy.deepcopy(contract)
    unhashed_progress["optimality_protocol"]["construction_evidence"][
        "release_basis_hash"
    ] = {"path": "tx-pool/architecture-contract.json"}
    observed = validate_optimality_protocol(unhashed_progress)
    if not any("construction evidence release_basis_hash fields differ" in error for error in observed):
        errors.append("optimality canary admitted progress without content identity")
    return errors


def validate_convergence_protocol(contract: dict) -> list[str]:
    """Validate the finite convergence relation and its two dependency DAGs."""

    errors: list[str] = []
    protocol = contract.get("convergence_protocol")
    if not isinstance(protocol, dict) or protocol.get("schema_version") != 1:
        return ["architecture contract convergence_protocol schema_version must be 1"]

    if protocol.get("states") != REQUIRED_CONVERGENCE_STATES:
        errors.append(
            "convergence states must be ordered construction, acceptance, accepted"
        )
    for field, expected in (
        ("release_law_sources", REQUIRED_CONVERGENCE_LAW_SOURCES),
        ("terminal_mutation_dispositions", REQUIRED_MUTATION_TERMINALS),
    ):
        actual = _string_set(protocol.get(field))
        if actual != expected:
            errors.append(
                f"convergence {field} differs: expected={sorted(expected)}, "
                f"actual={sorted(actual)}"
            )
    for field in ("construction_rank", "acceptance_universe", "acceptance_rank"):
        value = protocol.get(field)
        if (
            not isinstance(value, list)
            or not value
            or not all(isinstance(item, str) and item for item in value)
            or len(value) != len(set(value))
        ):
            errors.append(f"convergence {field} must be a nonempty unique string list")
    if _string_set(protocol.get("acceptance_universe")) != REQUIRED_ACCEPTANCE_UNIVERSE:
        errors.append("convergence acceptance universe differs")
    if protocol.get("termination_boundary") != (
        "fixed_complete_basis_and_eventually_stable_construction_inputs"
    ):
        errors.append("convergence termination boundary differs")

    invalidators = protocol.get("invalidators")
    if not isinstance(invalidators, dict):
        errors.append("convergence invalidators must be an object")
    else:
        construction = _string_set(invalidators.get("construction"))
        evidence_only = _string_set(invalidators.get("evidence_only"))
        if construction != REQUIRED_CONSTRUCTION_INVALIDATORS:
            errors.append("convergence construction invalidators differ")
        if evidence_only != REQUIRED_EVIDENCE_INVALIDATORS:
            errors.append("convergence evidence-only invalidators differ")
        if construction.intersection(evidence_only):
            errors.append("convergence invalidator classes overlap")

    evidence_dag = protocol.get("evidence_dag")
    if not isinstance(evidence_dag, dict):
        errors.append("convergence evidence_dag must be an object")
        evidence_dependencies = {}
    else:
        if evidence_dag.get("invalidation") != (
            "changed_node_and_transitive_successors_only"
        ):
            errors.append("evidence DAG must use transitive-successor invalidation")
        evidence_dependencies, dag_errors = _validate_named_dag(
            evidence_dag.get("nodes"),
            "convergence evidence DAG",
            require_bindings=True,
        )
        errors.extend(dag_errors)

    expected_evidence_nodes = {
        "reconciled_product",
        "correctness_oracle",
        "mutation_result",
        "deterministic_smoke_result",
        "complete_correctness_result",
        "empirical_performance_result",
        "final_release",
    }
    if set(evidence_dependencies) != expected_evidence_nodes:
        errors.append("convergence evidence node universe differs")
    if _descendants(evidence_dependencies, "reconciled_product") != (
        expected_evidence_nodes - {"reconciled_product"}
    ):
        errors.append("a product change must invalidate every later evidence node")
    if _descendants(evidence_dependencies, "empirical_performance_result") != {
        "final_release"
    }:
        errors.append("empirical performance evidence must not invalidate correctness")

    expected_transitions = {
        (
            "close_construction",
            "construction",
            "acceptance",
            "construction_rank_zero_and_universe_hash_bound",
            "bind_acceptance_evidence_to_universe",
        ),
        (
            "discharge_obligation",
            "acceptance",
            "acceptance",
            "acceptance_rank_positive",
            "strictly_decrease_acceptance_rank_without_universe_change",
        ),
        (
            "accept",
            "acceptance",
            "accepted",
            "acceptance_rank_zero",
            "retain_exact_universe_binding",
        ),
        (
            "construction_input_change",
            "acceptance",
            "construction",
            "always",
            "invalidate_construction_freeze_and_dependent_evidence",
        ),
        (
            "construction_input_change",
            "accepted",
            "construction",
            "always",
            "invalidate_construction_freeze_and_dependent_evidence",
        ),
        (
            "evidence_input_change",
            "acceptance",
            "acceptance",
            "blueprint_and_construction_inputs_unchanged",
            "invalidate_changed_node_and_transitive_successors",
        ),
        (
            "evidence_input_change",
            "accepted",
            "acceptance",
            "blueprint_and_construction_inputs_unchanged",
            "invalidate_changed_node_and_transitive_successors",
        ),
        (
            "new_semantic_family",
            "any",
            "construction",
            "legal_observable_counterexample",
            "invalidate_blueprint_freeze",
        ),
    }
    transition_rows = protocol.get("transitions")
    if not isinstance(transition_rows, list):
        errors.append("convergence transitions must be a list")
    else:
        transitions = {
            tuple(row.get(field) for field in ("event", "from", "to", "guard", "effect"))
            for row in transition_rows
            if isinstance(row, dict)
        }
        if len(transitions) != len(transition_rows):
            errors.append("convergence transitions must be unique objects")
        if transitions != expected_transitions:
            errors.append("convergence transition relation differs")

    phase_dependencies, phase_errors = _validate_named_dag(
        protocol.get("phase_dag"), "convergence phase DAG"
    )
    errors.extend(phase_errors)
    if set(phase_dependencies) != REQUIRED_CONVERGENCE_PHASES:
        errors.append("convergence phase universe differs")
    required_order = (
        ("basis_and_roadmap_normalization", "release_boundary_adjudication"),
        ("release_boundary_adjudication", "architecture_optimality_synthesis"),
        ("architecture_optimality_synthesis", "registered_semantic_root_closure"),
        ("registered_semantic_root_closure", "constructive_simplification"),
        ("constructive_simplification", "landing_rehearsal"),
        ("landing_rehearsal", "evidence_universe_freeze"),
        ("evidence_universe_freeze", "complete_mutation"),
        ("evidence_universe_freeze", "deterministic_smoke"),
        ("complete_mutation", "complete_correctness"),
        ("deterministic_smoke", "complete_correctness"),
        ("complete_correctness", "empirical_performance_acceptance"),
        ("empirical_performance_acceptance", "portability_and_final_review"),
    )
    for before, after in required_order:
        if after not in _descendants(phase_dependencies, before):
            errors.append(f"convergence phase {before} must precede {after}")

    if _string_set(protocol.get("landing_candidates")) != {
        "incremental",
        "curated_series",
        "one_shot",
    }:
        errors.append("convergence landing candidates are incomplete")
    if protocol.get("snowball_objective_ref") != (
        "optimization_goal.complexity_objective"
    ):
        errors.append("convergence snowball objective must reference the final goal")
    if protocol.get("source_change_after_correctness") != (
        "return_to_construction_and_invalidate_later_evidence"
    ):
        errors.append("post-correctness source change must return to Construction")
    return errors


def validate_convergence_status(manifest: dict, contract: dict) -> list[str]:
    """Validate one downward-closed status projection of the phase DAG."""

    errors: list[str] = []
    status = manifest.get("convergence_status")
    protocol = contract.get("convergence_protocol")
    if not isinstance(status, dict) or status.get("schema_version") != 2:
        return ["security manifest convergence_status schema_version must be 2"]
    if not isinstance(protocol, dict):
        return ["cannot validate convergence status without its protocol"]

    goal = contract.get("optimization_goal")
    recorded_goal_hash = status.get("optimization_goal_sha256")
    if (
        not isinstance(goal, dict)
        or re.fullmatch(r"[0-9a-f]{64}", str(recorded_goal_hash)) is None
        or recorded_goal_hash != canonical_json_sha256(goal)
    ):
        errors.append("convergence status optimization-goal hash binding differs")

    phase_dependencies, phase_errors = _validate_named_dag(
        protocol.get("phase_dag"), "convergence phase DAG"
    )
    errors.extend(phase_errors)
    state = status.get("state")
    if state not in protocol.get("states", []):
        errors.append(f"unknown convergence state {state!r}")

    rank_value = status.get("construction_rank")
    rank_fields = protocol.get("construction_rank")
    rank_ids: list[str] = []
    if (
        not isinstance(rank_value, dict)
        or not isinstance(rank_fields, list)
        or set(rank_value) != set(rank_fields)
    ):
        errors.append("construction rank projection differs from its protocol")
    else:
        for field in rank_fields:
            items = rank_value.get(field)
            if not isinstance(items, list) or not all(
                isinstance(item, str) and item for item in items
            ):
                errors.append(f"construction rank coordinate {field} is invalid")
                continue
            if len(items) != len(set(items)):
                errors.append(f"construction rank coordinate {field} has duplicates")
            rank_ids.extend(items)
        if len(rank_ids) != len(set(rank_ids)):
            errors.append("one construction obligation occupies multiple rank coordinates")

    construction_evidence = contract.get("optimality_protocol", {}).get(
        "construction_evidence", {}
    )
    pending_optimality = {
        requirement
        for requirement in REQUIRED_OPTIMALITY_CERTIFICATE_REQUIREMENTS
        if not isinstance(construction_evidence, dict)
        or construction_evidence.get(requirement) is None
    }
    rank_refinement = _string_set(
        rank_value.get("incomplete_refinement_edges")
        if isinstance(rank_value, dict)
        else None
    )
    ranked_optimality = rank_refinement.intersection(
        REQUIRED_OPTIMALITY_CERTIFICATE_REQUIREMENTS
    )
    if ranked_optimality != pending_optimality:
        errors.append(
            "optimality construction evidence and refinement rank differ: "
            f"evidence={sorted(pending_optimality)}, rank={sorted(ranked_optimality)}"
        )

    completed_value = status.get("completed_phases")
    completed = _string_set(completed_value)
    if not isinstance(completed_value, list) or len(completed) != len(completed_value):
        errors.append("completed convergence phases must be a unique string list")
    if completed.difference(phase_dependencies):
        errors.append("completed convergence phases contain an unknown node")
    for phase in completed.intersection(phase_dependencies):
        missing = phase_dependencies[phase] - completed
        if missing:
            errors.append(
                f"completed convergence phase {phase} lacks predecessors {sorted(missing)}"
            )
    if (
        "architecture_optimality_synthesis" in completed
        and pending_optimality.intersection(ARCHITECTURE_SYNTHESIS_REQUIREMENTS)
    ):
        errors.append(
            "completed architecture synthesis retains construction evidence "
            f"{sorted(pending_optimality.intersection(ARCHITECTURE_SYNTHESIS_REQUIREMENTS))}"
        )
    if (
        "constructive_simplification" in completed
        and pending_optimality.intersection(SIMPLIFICATION_REQUIREMENTS)
    ):
        errors.append(
            "completed simplification retains construction evidence "
            f"{sorted(pending_optimality.intersection(SIMPLIFICATION_REQUIREMENTS))}"
        )

    active = status.get("active_phase")
    if state in {"construction", "acceptance"} and active is None:
        errors.append(f"{state} requires one active convergence phase")
    if active is not None:
        if active not in phase_dependencies:
            errors.append(f"unknown active convergence phase {active!r}")
        elif active in completed:
            errors.append(f"active convergence phase {active} is already completed")
        else:
            missing = phase_dependencies[active] - completed
            if missing:
                errors.append(
                    f"active convergence phase {active} lacks predecessors "
                    f"{sorted(missing)}"
                )

    acceptance_phases = {
        "complete_mutation",
        "deterministic_smoke",
        "complete_correctness",
        "empirical_performance_acceptance",
        "portability_and_final_review",
    }
    universe = status.get("acceptance_universe")
    if state == "construction":
        if universe is not None or completed.intersection(acceptance_phases):
            errors.append("Construction cannot retain acceptance evidence")
        if active in acceptance_phases:
            errors.append("Construction cannot activate an acceptance phase")
    elif state in {"acceptance", "accepted"}:
        if rank_ids:
            errors.append(f"{state} requires a zero construction rank")
        if "evidence_universe_freeze" not in completed:
            errors.append(f"{state} requires a completed evidence universe freeze")
        if (
            not isinstance(universe, dict)
            or re.fullmatch(r"[0-9a-f]{64}", str(universe.get("sha256"))) is None
        ):
            errors.append(f"{state} requires one hash-bound acceptance universe")
    if state == "accepted":
        if "portability_and_final_review" not in completed or active is not None:
            errors.append("Accepted requires every phase complete and no active phase")
        if contract.get("optimality_protocol", {}).get("current_claim") != (
            "globally_optimal_within_declared_model_and_empirical_matrix"
        ):
            errors.append("Accepted requires a certified final optimization witness")

    if "constructive_simplification" in completed and contract.get(
        "optimality_protocol", {}
    ).get("current_claim") != (
        "globally_optimal_within_declared_model_and_empirical_matrix"
    ):
        errors.append(
            "constructive simplification cannot close before the final "
            "optimization certificate"
        )

    blockers = manifest.get("release_blockers")
    if not isinstance(blockers, list):
        errors.append("release blockers must be a list")
    else:
        blocker_ids = [
            blocker.get("id") for blocker in blockers if isinstance(blocker, dict)
        ]
        if (
            len(blocker_ids) != len(blockers)
            or not all(isinstance(item, str) and item for item in blocker_ids)
            or len(blocker_ids) != len(set(blocker_ids))
        ):
            errors.append("release blocker ids must be unique nonempty strings")
        if state == "accepted" and blockers:
            errors.append("Accepted cannot retain a release blocker")
        obligation_owners: dict[str, list[str]] = {}
        for blocker in blockers:
            if not isinstance(blocker, dict):
                continue
            blocker_id = blocker.get("id")
            obligations = blocker.get("construction_obligations", [])
            if not isinstance(obligations, list) or not all(
                isinstance(item, str) and item for item in obligations
            ):
                errors.append(
                    f"release blocker {blocker_id!r} has invalid construction obligations"
                )
                continue
            if len(obligations) != len(set(obligations)):
                errors.append(
                    f"release blocker {blocker_id!r} repeats a construction obligation"
                )
            for obligation in obligations:
                obligation_owners.setdefault(obligation, []).append(str(blocker_id))
        unknown_obligations = set(obligation_owners).difference(rank_ids)
        if unknown_obligations:
            errors.append(
                "release blockers reference unknown construction obligations "
                f"{sorted(unknown_obligations)}"
            )
        missing_owners = set(rank_ids).difference(obligation_owners)
        if missing_owners:
            errors.append(
                "construction obligations lack a release-blocker owner "
                f"{sorted(missing_owners)}"
            )
        duplicate_owners = {
            obligation: owners
            for obligation, owners in obligation_owners.items()
            if len(owners) != 1
        }
        if duplicate_owners:
            errors.append(
                f"construction obligations have duplicate blocker owners {duplicate_owners}"
            )
    return errors


def validate_construction_root_families(manifest: dict, contract: dict) -> list[str]:
    """Bind every open release law to exactly one falsifiable root family."""

    errors: list[str] = []
    rows = manifest.get("construction_root_families")
    if not isinstance(rows, list) or not rows:
        return ["construction root families must be a nonempty list"]
    required_fields = {
        "id",
        "members",
        "root_family_refs",
        "invariant_refs",
        "law",
        "falsifier",
        "closure_phase",
    }
    family_ids: list[str] = []
    member_owners: dict[str, list[str]] = {}
    known_roots = set(contract.get("root_families", {}))
    known_invariants = set(contract.get("target_invariants", {}))
    phases = {
        row.get("id")
        for row in contract.get("convergence_protocol", {}).get("phase_dag", [])
        if isinstance(row, dict)
    }

    for row in rows:
        if not isinstance(row, dict):
            errors.append("each construction root family must be an object")
            continue
        if set(row) != required_fields:
            errors.append("construction root family fields differ")
        family_id = row.get("id")
        if re.fullmatch(r"CRF-[A-Z0-9-]+", str(family_id)) is None:
            errors.append(f"invalid construction root family id {family_id!r}")
            continue
        family_ids.append(family_id)
        members = row.get("members")
        if not _nonempty_unique_strings(members):
            errors.append(f"construction root family {family_id} has invalid members")
        else:
            for member in members:
                member_owners.setdefault(member, []).append(family_id)

        for field, known in (
            ("root_family_refs", known_roots),
            ("invariant_refs", known_invariants),
        ):
            refs = row.get(field)
            if not _nonempty_unique_strings(refs):
                errors.append(f"construction root family {family_id} has invalid {field}")
                continue
            if refs != ["*"]:
                unknown = set(refs).difference(known)
                if unknown:
                    errors.append(
                        f"construction root family {family_id} has unknown {field} "
                        f"{sorted(unknown)}"
                    )
        for field in ("law", "falsifier"):
            if re.fullmatch(r"[a-z][a-z0-9_]+", str(row.get(field))) is None:
                errors.append(
                    f"construction root family {family_id} has invalid {field}"
                )
        if row.get("closure_phase") != "registered_semantic_root_closure":
            errors.append(
                f"construction root family {family_id} closes outside root closure"
            )
        if row.get("closure_phase") not in phases:
            errors.append(
                f"construction root family {family_id} names an unknown phase"
            )

    if len(family_ids) != len(set(family_ids)):
        errors.append("construction root family ids must be unique")
    rank = manifest.get("convergence_status", {}).get("construction_rank", {})
    open_laws = _string_set(
        rank.get("open_release_laws") if isinstance(rank, dict) else None
    )
    mapped_laws = set(member_owners)
    dispositions = manifest.get("convergence_status", {}).get(
        "release_law_dispositions"
    )
    if not isinstance(dispositions, dict):
        errors.append("release-law dispositions must be an object")
        dispositions = {}
    closed_laws = set(dispositions)
    if open_laws.intersection(closed_laws):
        errors.append(
            "one release law is both open and terminally disposed "
            f"{sorted(open_laws.intersection(closed_laws))}"
        )
    if open_laws.union(closed_laws) != mapped_laws:
        errors.append(
            "construction root-family state partition differs from the immutable "
            "release-law basis: "
            f"unclassified={sorted(mapped_laws - open_laws - closed_laws)}, "
            f"unknown={sorted(open_laws.union(closed_laws) - mapped_laws)}"
        )
    for law, record in dispositions.items():
        if not isinstance(record, dict) or set(record) != {
            "root_family_id",
            "disposition",
            "evidence_owner_refs",
        }:
            errors.append(f"release-law disposition {law!r} has invalid fields")
            continue
        if record.get("disposition") not in {
            "proved",
            "superseded",
            "unconstructible",
        }:
            errors.append(f"release-law disposition {law!r} is not terminal")
        evidence_refs = record.get("evidence_owner_refs")
        if not _nonempty_unique_strings(evidence_refs) or not all(
            ":" in reference for reference in evidence_refs
        ):
            errors.append(
                f"release-law disposition {law!r} lacks exact evidence-owner refs"
            )
        owners = member_owners.get(law, [])
        if owners != [record.get("root_family_id")]:
            errors.append(
                f"release-law disposition {law!r} names the wrong root family"
            )
    duplicate_members = {
        member: owners for member, owners in member_owners.items() if len(owners) != 1
    }
    if duplicate_members:
        errors.append(
            f"open release laws have duplicate root families {duplicate_members}"
        )
    return errors


def validate_construction_root_family_canaries(
    manifest: dict, contract: dict
) -> list[str]:
    errors: list[str] = []

    unmapped = copy.deepcopy(manifest)
    unmapped["construction_root_families"][0]["members"].clear()
    observed = validate_construction_root_families(unmapped, contract)
    if not any("state partition differs" in error for error in observed):
        errors.append("construction root-family canary admitted an unmapped law")

    duplicated = copy.deepcopy(manifest)
    member = duplicated["construction_root_families"][0]["members"][0]
    duplicated["construction_root_families"][1]["members"].append(member)
    observed = validate_construction_root_families(duplicated, contract)
    if not any("duplicate root families" in error for error in observed):
        errors.append("construction root-family canary admitted duplicate ownership")

    unknown_invariant = copy.deepcopy(manifest)
    unknown_invariant["construction_root_families"][0]["invariant_refs"] = [
        "T-UNKNOWN"
    ]
    observed = validate_construction_root_families(unknown_invariant, contract)
    if not any("unknown invariant_refs" in error for error in observed):
        errors.append("construction root-family canary admitted an unknown law ref")

    unproved = copy.deepcopy(manifest)
    dispositions = unproved["convergence_status"]["release_law_dispositions"]
    if dispositions:
        first = next(iter(dispositions.values()))
        first["evidence_owner_refs"].clear()
        observed = validate_construction_root_families(unproved, contract)
        if not any("lacks exact evidence-owner refs" in error for error in observed):
            errors.append(
                "construction root-family canary admitted an evidence-free closure"
            )
    return errors


def validate_release_boundary_status(
    manifest: dict, contract: dict, registry: dict
) -> list[str]:
    """Require machine evidence before a compatibility obligation disappears."""

    errors: list[str] = []
    status = manifest.get("release_boundary_status")
    if not isinstance(status, dict) or status.get("schema_version") != 2:
        return ["release boundary status schema_version must be 2"]
    if set(status) != {
        "schema_version",
        "legacy_configuration_forward_compatibility",
        "rust_api_compatibility",
        "landing_input_matrix",
    }:
        errors.append("release boundary status fields differ")

    rank = manifest.get("convergence_status", {}).get("construction_rank", {})
    open_decisions = _string_set(
        rank.get("open_compatibility_and_landing_decisions")
        if isinstance(rank, dict)
        else None
    )
    completed = _string_set(
        manifest.get("convergence_status", {}).get("completed_phases")
    )

    legacy = status.get("legacy_configuration_forward_compatibility")
    if not isinstance(legacy, dict) or set(legacy) != {
        "state",
        "policy_ref",
        "behavior_refs",
    }:
        return errors + ["legacy configuration compatibility evidence fields differ"]
    if legacy.get("state") not in {"open", "proved"}:
        errors.append("legacy configuration compatibility state is invalid")
    if legacy.get("policy_ref") != (
        "architecture_contract.release_surface.compatibility_policy"
    ):
        errors.append("legacy configuration compatibility policy reference differs")
    behavior_refs = legacy.get("behavior_refs")
    if not _nonempty_unique_strings(behavior_refs):
        errors.append("legacy configuration compatibility behavior refs are invalid")
        behavior_refs = []
    known_behaviors = {
        behavior.get("id")
        for behavior in registry.get("behaviors", [])
        if isinstance(behavior, dict) and isinstance(behavior.get("id"), str)
    }
    evidenced_behaviors = {
        evidence.get("behavior_id")
        for evidence in registry.get("unit_evidence", [])
        if isinstance(evidence, dict) and isinstance(evidence.get("behavior_id"), str)
    }
    for evidence in registry.get("cross_crate_evidence", []):
        if isinstance(evidence, dict):
            evidenced_behaviors.update(_string_set(evidence.get("behavior_ids")))
    unknown = set(behavior_refs).difference(known_behaviors)
    unevidenced = set(behavior_refs).difference(evidenced_behaviors)
    if unknown:
        errors.append(
            f"legacy configuration compatibility uses unknown behaviors {sorted(unknown)}"
        )
    if unevidenced:
        errors.append(
            "legacy configuration compatibility uses behaviors without exact evidence "
            f"{sorted(unevidenced)}"
        )

    obligation = "legacy_configuration_forward_compatibility"
    if legacy.get("state") == "proved" and obligation in open_decisions:
        errors.append("proved legacy configuration compatibility remains in the rank")
    if legacy.get("state") == "open" and obligation not in open_decisions:
        errors.append("open legacy configuration compatibility is absent from the rank")
    if legacy.get("state") == "proved":
        policy = contract.get("release_surface", {}).get("compatibility_policy", {})
        if policy != REQUIRED_RELEASE_COMPATIBILITY_POLICY:
            errors.append("proved legacy configuration compatibility has policy drift")

    rust_api = status.get("rust_api_compatibility")
    if not isinstance(rust_api, dict) or set(rust_api) != {
        "state",
        "policy_ref",
        "disposition",
        "landing_obligation",
    }:
        errors.append("Rust API compatibility status fields differ")
        rust_api = {}
    if rust_api.get("state") != "adjudicated":
        errors.append("Rust API compatibility must be adjudicated")
    if rust_api.get("policy_ref") != (
        "architecture_contract.release_surface.rust_api_compatibility"
    ):
        errors.append("Rust API compatibility policy reference differs")
    contract_disposition = (
        contract.get("release_surface", {})
        .get("rust_api_compatibility", {})
        .get("decision_function", {})
        .get("value")
    )
    if (
        rust_api.get("disposition") != "intentional_major"
        or rust_api.get("disposition") != contract_disposition
    ):
        errors.append("Rust API compatibility disposition differs from its contract")
    if rust_api.get("landing_obligation") != "landing_topology":
        errors.append("Rust API compatibility landing obligation differs")
    if "rust_api_semver_disposition" in open_decisions:
        errors.append("adjudicated Rust API compatibility remains in the rank")

    landing = status.get("landing_input_matrix")
    if not isinstance(landing, dict) or set(landing) != {
        "state",
        "policy_ref",
        "selection",
        "construction_obligation",
    }:
        errors.append("landing input matrix status fields differ")
        landing = {}
    if landing.get("state") != "defined":
        errors.append("landing input matrix must be defined")
    if landing.get("policy_ref") != "architecture_contract.landing_protocol":
        errors.append("landing input matrix policy reference differs")
    if landing.get("construction_obligation") != "landing_topology":
        errors.append("landing input matrix construction obligation differs")
    selection = landing.get("selection")
    contract_selection = contract.get("landing_protocol", {}).get("current_selection")
    if selection == "open":
        if "landing_topology" not in open_decisions:
            errors.append("open landing topology is absent from the rank")
        if contract_selection is not None:
            errors.append("open landing topology has a premature selection certificate")
    elif selection == "selected":
        if "landing_topology" in open_decisions:
            errors.append("selected landing topology remains in the rank")
        if contract_selection is None:
            errors.append("selected landing topology lacks a selection certificate")
    else:
        errors.append("landing input matrix selection state is invalid")

    if "release_boundary_adjudication" in completed:
        if legacy.get("state") != "proved":
            errors.append("completed release boundary lacks legacy compatibility proof")
        if rust_api.get("state") != "adjudicated":
            errors.append("completed release boundary lacks the Rust API decision")
        if landing.get("state") != "defined":
            errors.append("completed release boundary lacks landing inputs")
    return errors


def validate_release_boundary_canaries(
    manifest: dict, contract: dict, registry: dict
) -> list[str]:
    errors: list[str] = []

    missing_evidence = copy.deepcopy(manifest)
    missing_evidence["release_boundary_status"][
        "legacy_configuration_forward_compatibility"
    ]["behavior_refs"].clear()
    observed = validate_release_boundary_status(missing_evidence, contract, registry)
    if not any("behavior refs are invalid" in error for error in observed):
        errors.append("release boundary canary admitted a proof without evidence")

    false_close = copy.deepcopy(manifest)
    false_close["convergence_status"]["construction_rank"][
        "open_compatibility_and_landing_decisions"
    ].append("legacy_configuration_forward_compatibility")
    observed = validate_release_boundary_status(false_close, contract, registry)
    if not any("remains in the rank" in error for error in observed):
        errors.append("release boundary canary admitted contradictory compatibility state")

    facade = copy.deepcopy(manifest)
    facade["release_boundary_status"]["rust_api_compatibility"]["disposition"] = (
        "non_authoritative_facade"
    )
    observed = validate_release_boundary_status(facade, contract, registry)
    if not any("disposition differs" in error for error in observed):
        errors.append("release boundary canary admitted a Rust API facade")

    premature_landing = copy.deepcopy(manifest)
    premature_landing["release_boundary_status"]["landing_input_matrix"][
        "selection"
    ] = "selected"
    observed = validate_release_boundary_status(premature_landing, contract, registry)
    if not any("remains in the rank" in error for error in observed):
        errors.append("release boundary canary admitted a premature landing selection")

    unowned_landing = copy.deepcopy(manifest)
    unowned_landing["convergence_status"]["construction_rank"][
        "open_compatibility_and_landing_decisions"
    ].remove("landing_topology")
    observed = validate_release_boundary_status(unowned_landing, contract, registry)
    if not any("absent from the rank" in error for error in observed):
        errors.append("release boundary canary admitted an unowned landing decision")
    return errors


def validate_convergence_canaries(manifest: dict, contract: dict) -> list[str]:
    """Prove the convergence validator rejects the known shortcut classes."""

    errors: list[str] = []

    reordered_goal = copy.deepcopy(contract)
    dimensions = reordered_goal["optimization_goal"]["static_objective"]
    dimensions[0], dimensions[1] = dimensions[1], dimensions[0]
    observed = validate_convergence_status(manifest, reordered_goal)
    if not any("optimization-goal hash binding differs" in error for error in observed):
        errors.append("convergence canary admitted an objective reorder")

    incomplete_goal = copy.deepcopy(contract)
    incomplete_goal["optimization_goal"]["hard_constraints"].pop()
    observed = validate_convergence_status(manifest, incomplete_goal)
    if not any("optimization-goal hash binding differs" in error for error in observed):
        errors.append("convergence canary admitted a missing hard constraint")

    stale_basis, basis_errors = expected_release_basis(manifest, contract)
    if basis_errors:
        errors.append("cannot construct the release-basis negative canary")
    else:
        stale_basis["source_sha256"]["optimization_goal"] = "0" * 64
        observed = validate_release_basis_value(stale_basis, manifest, contract)
        if not any("generated projection" in error for error in observed):
            errors.append("convergence canary admitted a stale release basis")

    stale_matrix, matrix_errors = expected_workload_matrix_evidence(contract)
    if matrix_errors:
        errors.append("cannot construct the workload-matrix negative canary")
    else:
        stale_matrix["matrix_sha256"] = "0" * 64
        observed = validate_workload_matrix_evidence_value(stale_matrix, contract)
        if not any("generated projection" in error for error in observed):
            errors.append("convergence canary admitted a stale workload matrix")

    normal_form_evidence = contract.get("optimality_protocol", {}).get(
        "construction_evidence", {}
    )
    normal_form_artifact = (
        normal_form_evidence.get("normal_form_coverage_proof")
        if isinstance(normal_form_evidence, dict)
        else None
    )
    if isinstance(normal_form_artifact, dict):
        value, load_errors = load_repo_json(
            normal_form_artifact.get("path"), "normal_form_partition_canary"
        )
        if load_errors or value is None:
            errors.append("cannot construct the normal-form coverage negative canary")
        else:
            omitted_atom = copy.deepcopy(value)
            coverage = omitted_atom.get("basis_axis_coverage", {})
            assigned = next(
                (
                    atoms
                    for atoms in coverage.values()
                    if isinstance(atoms, list) and atoms
                ),
                None,
            )
            if assigned is None:
                errors.append("normal-form coverage canary found no assigned atom")
            else:
                assigned.pop()
                observed = validate_normal_form_partition_evidence_value(
                    omitted_atom, manifest, contract
                )
                if not any("basis coverage differs" in error for error in observed):
                    errors.append("convergence canary admitted an omitted basis atom")

    unhashed_progress = copy.deepcopy(manifest)
    evidence = contract["optimality_protocol"]["construction_evidence"]
    requirement = next(iter(sorted(REQUIRED_OPTIMALITY_CERTIFICATE_REQUIREMENTS)))
    ranked = unhashed_progress["convergence_status"]["construction_rank"][
        "incomplete_refinement_edges"
    ]
    if evidence[requirement] is None:
        ranked.remove(requirement)
    else:
        ranked.append(requirement)
    observed = validate_convergence_status(unhashed_progress, contract)
    if not any("construction evidence and refinement rank differ" in error for error in observed):
        errors.append("convergence canary admitted unhashed optimality progress")

    nondecreasing = copy.deepcopy(contract)
    discharge = next(
        row
        for row in nondecreasing["convergence_protocol"]["transitions"]
        if row["event"] == "discharge_obligation"
    )
    discharge["effect"] = "retain_acceptance_rank"
    observed = validate_convergence_protocol(nondecreasing)
    if not any("transition relation differs" in error for error in observed):
        errors.append("convergence canary admitted a nondecreasing acceptance step")

    cleanup_after_mutation = copy.deepcopy(contract)
    phases = {
        row["id"]: row
        for row in cleanup_after_mutation["convergence_protocol"]["phase_dag"]
    }
    phases["constructive_simplification"]["requires"] = ["complete_mutation"]
    observed = validate_convergence_protocol(cleanup_after_mutation)
    if not any("contains a cycle" in error for error in observed):
        errors.append("convergence canary admitted cleanup after final mutation")

    performance_to_correctness = copy.deepcopy(contract)
    evidence_nodes = {
        row["id"]: row
        for row in performance_to_correctness["convergence_protocol"]["evidence_dag"][
            "nodes"
        ]
    }
    evidence_nodes["complete_correctness_result"]["requires"].append(
        "empirical_performance_result"
    )
    observed = validate_convergence_protocol(performance_to_correctness)
    if not any("contains a cycle" in error for error in observed):
        errors.append("convergence canary admitted performance-to-correctness invalidation")

    late_optimality = copy.deepcopy(contract)
    late_phases = {
        row["id"]: row
        for row in late_optimality["convergence_protocol"]["phase_dag"]
    }
    late_phases["registered_semantic_root_closure"]["requires"] = [
        "release_boundary_adjudication"
    ]
    late_phases["architecture_optimality_synthesis"]["requires"] = [
        "complete_correctness"
    ]
    observed = validate_convergence_protocol(late_optimality)
    if not any(
        "contains a cycle" in error or "must precede" in error for error in observed
    ):
        errors.append("convergence canary admitted downstream architecture optimality")

    skipped_predecessor = copy.deepcopy(manifest)
    skipped_predecessor["convergence_status"]["active_phase"] = (
        "constructive_simplification"
    )
    observed = validate_convergence_status(skipped_predecessor, contract)
    if not any("lacks predecessors" in error for error in observed):
        errors.append("convergence canary admitted a phase before its predecessors")

    unmapped_obligation = copy.deepcopy(manifest)
    basis_blocker = next(
        blocker
        for blocker in unmapped_obligation["release_blockers"]
        if blocker["id"] == "REGISTERED-RELEASE-LAW-CLOSURE"
    )
    basis_blocker["construction_obligations"].pop()
    observed = validate_convergence_status(unmapped_obligation, contract)
    if not any("lack a release-blocker owner" in error for error in observed):
        errors.append("convergence canary admitted an unmapped construction obligation")

    stale_acceptance = copy.deepcopy(manifest)
    stale_acceptance["convergence_status"] = {
        "schema_version": 2,
        "state": "accepted",
        "active_phase": None,
        "completed_phases": [],
        "acceptance_universe": None,
        "construction_rank": {
            field: []
            for field in contract["convergence_protocol"]["construction_rank"]
        },
    }
    observed = validate_convergence_status(stale_acceptance, contract)
    if not any("requires every phase complete" in error for error in observed):
        errors.append("convergence canary admitted Accepted without current evidence")
    return errors
def validate_enum_boundary_mapping(
    value: object,
    path_value: str,
    enum_name: str,
    field: str,
) -> list[str]:
    discovered, errors = rust_enum_variants(path_value, enum_name)
    if not isinstance(value, dict) or not all(
        isinstance(key, str) and isinstance(family, str)
        for key, family in value.items()
    ):
        errors.append(f"architecture contract {field} must be a string mapping")
        return errors
    configured = set(value)
    if configured != discovered:
        errors.append(
            f"architecture contract {field} differs from Rust {enum_name}: "
            f"contract={sorted(configured)}, Rust={sorted(discovered)}"
        )
    unknown = set(value.values()).difference(MODEL_BOUNDARY_FAMILIES)
    if unknown:
        errors.append(
            f"architecture contract {field} uses unknown model families: "
            f"{sorted(unknown)}"
        )
    return errors


def rust_self_variant_array(
    path_value: str, enum_name: str, constant_name: str
) -> tuple[list[str], list[str]]:
    try:
        source = repo_path(path_value).read_text()
    except (OSError, ValueError) as error:
        return [], [f"cannot read Rust ordered enum owner {path_value}: {error}"]
    impl_match = re.search(
        rf"impl\s+{re.escape(enum_name)}\s*\{{(?P<body>.*?)\n\}}",
        source,
        flags=re.DOTALL,
    )
    if impl_match is None:
        return [], [f"Rust impl {enum_name} is absent from {path_value}"]
    constant = re.search(
        rf"\bconst\s+{re.escape(constant_name)}\b[^=]*=\s*\[(?P<items>.*?)\]\s*;",
        impl_match.group("body"),
        flags=re.DOTALL,
    )
    if constant is None:
        return [], [
            f"Rust ordered constant {enum_name}::{constant_name} is absent from {path_value}"
        ]
    return re.findall(r"\bSelf::([A-Z][A-Za-z0-9_]*)\b", constant.group("items")), []


def rust_impl_public_methods(path_value: str, type_name: str) -> tuple[set[str], list[str]]:
    """Discover public inherent methods from one ordinary Rust impl block."""

    try:
        source = repo_path(path_value).read_text()
    except (OSError, ValueError) as error:
        return set(), [f"cannot read Rust impl owner {path_value}: {error}"]
    declaration = re.search(rf"\bimpl\s+{re.escape(type_name)}\s*\{{", source)
    if declaration is None:
        return set(), [f"Rust impl {type_name} is absent from {path_value}"]
    cursor = declaration.end()
    depth = 1
    start = cursor
    while cursor < len(source) and depth:
        if source[cursor] == "{":
            depth += 1
        elif source[cursor] == "}":
            depth -= 1
        cursor += 1
    if depth:
        return set(), [f"Rust impl {type_name} has no closing brace in {path_value}"]
    body = source[start : cursor - 1]
    methods = set(
        re.findall(
            r"(?m)^\s*pub\s+(?:async\s+)?fn\s+([a-z][A-Za-z0-9_]*)\b",
            body,
        )
    )
    if not methods:
        return set(), [f"Rust impl {type_name} has no public methods in {path_value}"]
    return methods, []


def validate_impl_method_boundary_mapping(
    value: object,
    path_value: str,
    type_name: str,
    field: str,
) -> list[str]:
    discovered, errors = rust_impl_public_methods(path_value, type_name)
    if not isinstance(value, dict) or not all(
        isinstance(key, str) and isinstance(family, str)
        for key, family in value.items()
    ):
        errors.append(f"architecture contract {field} must be a string mapping")
        return errors
    configured = set(value)
    if configured != discovered:
        errors.append(
            f"architecture contract {field} differs from Rust {type_name} public methods: "
            f"contract={sorted(configured)}, Rust={sorted(discovered)}"
        )
    unknown = set(value.values()).difference(MODEL_BOUNDARY_FAMILIES)
    if unknown:
        errors.append(
            f"architecture contract {field} uses unknown model families: "
            f"{sorted(unknown)}"
        )
    return errors


def validate_architecture_contract(contract: dict, registry: dict) -> list[str]:
    errors: list[str] = []
    if contract.get("schema_version") != 22:
        errors.append("architecture contract schema_version must be 22")
    errors.extend(validate_selected_topology(contract, registry))
    errors.extend(validate_selected_topology_canaries(contract, registry))
    errors.extend(validate_rust_api_compatibility(contract))
    errors.extend(validate_landing_protocol(contract))
    errors.extend(validate_workload_environment_matrix(contract))
    errors.extend(validate_release_protocol_canaries(contract))
    errors.extend(validate_interruption_contract(contract, registry))
    errors.extend(validate_optimization_goal(contract))
    errors.extend(validate_convergence_protocol(contract))
    errors.extend(validate_optimality_protocol(contract))
    errors.extend(validate_optimality_canaries(contract))

    authority = contract.get("authority")
    if not isinstance(authority, dict):
        errors.append("architecture contract authority must be an object")
        authority = {}
    if authority.get("store") != "AuthorityStore":
        errors.append("architecture contract store must be AuthorityStore")
    if authority.get("transaction_owner") != "TxPoolAuthority":
        errors.append("architecture contract transaction owner must be TxPoolAuthority")
    errors.extend(
        require_source_symbols(
            "tx-pool/src/authority/runtime.rs",
            ["struct AuthorityStore", "authority: TxPoolAuthority", "snapshot: Arc<Snapshot>"],
            "single authority store",
        )
    )

    identity = contract.get("identity")
    if not isinstance(identity, dict):
        errors.append("architecture contract identity must be an object")
        identity = {}
    if identity.get("entry_version") != (
        "process_global_non_reused_u128_and_active_compute_identity"
    ):
        errors.append("EntryVersion must remain the sole numeric active-compute identity")
    if identity.get("compute_capability") != (
        "move_only_checked_out_work_bound_to_entry_version"
    ):
        errors.append("compute capability must remain move-only work bound to EntryVersion")
    if identity.get("verification_cache_key") != (
        "inline_witness_hash_32_bytes_and_ScriptVerificationRules"
    ):
        errors.append(
            "verification cache identity must bind the inline witness hash and script rules"
        )
    if "compute_lease" in identity:
        errors.append("architecture contract must not restore a second compute-lease counter")

    if contract.get("proof_policy") != REQUIRED_PROOF_POLICY:
        errors.append(
            "architecture contract must define the exact executable-mathematics proof policy"
        )
    errors.extend(
        require_source_symbols(
            "tx-pool/docs/ARCHITECTURE.md",
            [
                "### 4.5 Executable mathematical proof kernel",
                "The executable reference model, property/concurrency falsifiers",
                "A new view may create a new obligation",
            ],
            "published mathematical proof policy",
        )
    )
    errors.extend(
        require_source_symbols(
            "tx-pool/docs/REVIEW_GUIDE.md",
            [
                "pure reference model and differential/property tests",
                "identify a local finite rank",
                "An unchanged-cut retry",
            ],
            "review mathematical proof policy",
        )
    )
    errors.extend(
        require_source_symbols(
            "tx-pool/src/authority/plan.rs",
            ["struct TxPoolAuthority", "entries: HashMap<RawTxHash, OwnedTx>"],
            "transaction authority",
        )
    )

    owner_algebra = contract.get("owner_algebra")
    if not isinstance(owner_algebra, dict):
        errors.append("architecture contract owner_algebra must be an object")
        owner_algebra = {}
    enum_contracts = (
        ("variants", "OwnedTx"),
        ("preaccepted_phase_variants", "PreAcceptedPhase"),
        ("queued_work_variants", "QueuedWork"),
        ("accepted_statuses", "AcceptedStatus"),
    )
    for field, enum_name in enum_contracts:
        discovered, enum_errors = rust_enum_variants(
            "tx-pool/src/authority/state.rs", enum_name
        )
        errors.extend(enum_errors)
        if _string_set(owner_algebra.get(field)) != discovered:
            errors.append(
                f"architecture contract {field} differs from Rust {enum_name}: "
                f"contract={sorted(_string_set(owner_algebra.get(field)))}, "
                f"Rust={sorted(discovered)}"
            )

    model_boundary = contract.get("model_boundary_algebra")
    if not isinstance(model_boundary, dict):
        errors.append("architecture contract model_boundary_algebra must be an object")
        model_boundary = {}
    for field, path_value, type_name in (
        (
            "service_builder_methods",
            "tx-pool/src/service/builder.rs",
            "TxPoolServiceBuilder",
        ),
        (
            "controller_methods",
            "tx-pool/src/service/controller.rs",
            "TxPoolController",
        ),
        (
            "relay_receiver_methods",
            "tx-pool/src/service.rs",
            "TxVerificationResultReceiver",
        ),
        (
            "recent_reject_methods",
            "tx-pool/src/component/recent_reject.rs",
            "RecentReject",
        ),
        (
            "block_assembler_methods",
            "tx-pool/src/block_assembler/mod.rs",
            "BlockAssembler",
        ),
        (
            "candidate_uncles_methods",
            "tx-pool/src/block_assembler/candidate_uncles.rs",
            "CandidateUncles",
        ),
    ):
        errors.extend(
            validate_impl_method_boundary_mapping(
                model_boundary.get(field), path_value, type_name, field
            )
        )
    for field, path_value, enum_name in (
        ("service_messages", "tx-pool/src/service/message.rs", "Message"),
        ("plug_targets", "tx-pool/src/lib.rs", "PlugTarget"),
        (
            "block_assembler_errors",
            "tx-pool/src/error.rs",
            "BlockAssemblerError",
        ),
        ("chain_controls", "tx-pool/src/service/message.rs", "ChainControl"),
        ("committed_effects", "tx-pool/src/authority/effect.rs", "CommittedEffect"),
        (
            "committed_acceptances",
            "tx-pool/src/authority/effect.rs",
            "CommittedAcceptance",
        ),
        (
            "committed_rejections",
            "tx-pool/src/authority/effect.rs",
            "CommittedRejection",
        ),
        ("chain_removals", "tx-pool/src/authority/chain.rs", "ChainRemoval"),
        ("read_states", "tx-pool/src/authority/read.rs", "AuthorityReadState"),
        ("rpc_statuses", "tx-pool/src/authority/read.rs", "AuthorityRpcStatus"),
        (
            "preaccepted_sources",
            "tx-pool/src/authority/state.rs",
            "PreAcceptedSource",
        ),
        ("proposal_bases", "tx-pool/src/authority/state.rs", "ProposalBase"),
        ("payload_policies", "tx-pool/src/authority/state.rs", "PayloadPolicy"),
        ("chunk_commands", "script/src/types.rs", "ChunkCommand"),
        ("relay_results", "tx-pool/src/service.rs", "TxVerificationResult"),
        (
            "relay_mailbox_dispositions",
            "tx-pool/src/authority/relay.rs",
            "RelayMailboxDisposition",
        ),
        ("callback_events", "tx-pool/src/callback.rs", "CallbackEvent"),
        (
            "template_publications",
            "tx-pool/src/authority/template.rs",
            "TemplatePublication",
        ),
        (
            "template_steps",
            "tx-pool/src/authority/template_driver.rs",
            "AuthorityTemplateStep",
        ),
        (
            "template_roles",
            "tx-pool/src/authority/template_driver.rs",
            "AuthorityTemplateRole",
        ),
        (
            "topology_events",
            "tx-pool/src/authority/topology.rs",
            "AuthorityTopologyEvent",
        ),
        (
            "generation_events",
            "tx-pool/src/authority/service.rs",
            "AuthorityGenerationEvent",
        ),
        (
            "shutdown_statuses",
            "tx-pool/src/authority/topology.rs",
            "AuthorityShutdownStatus",
        ),
    ):
        errors.extend(
            validate_enum_boundary_mapping(
                model_boundary.get(field), path_value, enum_name, field
            )
        )

    effect_endpoint_order, order_errors = rust_self_variant_array(
        "tx-pool/src/authority/effect.rs", "EffectEndpoint", "ORDER"
    )
    errors.extend(order_errors)
    configured_endpoint_order = model_boundary.get("effect_endpoint_order")
    if configured_endpoint_order != effect_endpoint_order:
        errors.append(
            "architecture contract effect_endpoint_order differs from Rust "
            f"EffectEndpoint::ORDER: contract={configured_endpoint_order!r}, "
            f"Rust={effect_endpoint_order!r}"
        )

    used_model_families = {
        family
        for mapping in model_boundary.values()
        if isinstance(mapping, dict)
        for family in mapping.values()
        if isinstance(family, str)
    }
    model_family_evidence = contract.get("model_family_evidence")
    if not isinstance(model_family_evidence, dict):
        errors.append("architecture contract model_family_evidence must be an object")
        model_family_evidence = {}
    configured_families = set(model_family_evidence)
    if configured_families != used_model_families:
        errors.append(
            "architecture contract model_family_evidence differs from used model "
            f"families: evidence={sorted(configured_families)}, "
            f"used={sorted(used_model_families)}"
        )
    behavior_ids = {
        entry.get("id")
        for entry in registry.get("behaviors", [])
        if isinstance(entry, dict) and isinstance(entry.get("id"), str)
    }
    evidenced_behaviors = {
        entry.get("behavior_id")
        for entry in registry.get("unit_evidence", [])
        if isinstance(entry, dict) and isinstance(entry.get("behavior_id"), str)
    }
    for entry in registry.get("workspace_evidence", []):
        if (
            isinstance(entry, dict)
            and entry.get("evidence_kind") != "counterexample"
            and isinstance(entry.get("behavior_ids"), list)
        ):
            evidenced_behaviors.update(
                behavior_id
                for behavior_id in entry["behavior_ids"]
                if isinstance(behavior_id, str)
            )
    for entry in registry.get("integration_evidence", []):
        if isinstance(entry, dict) and isinstance(entry.get("behavior_ids"), list):
            evidenced_behaviors.update(
                behavior_id
                for behavior_id in entry["behavior_ids"]
                if isinstance(behavior_id, str)
            )

    cross_cutting = contract.get("model_cross_cutting_evidence")
    if not isinstance(cross_cutting, dict):
        errors.append(
            "architecture contract model_cross_cutting_evidence must be an object"
        )
        cross_cutting = {}
    if set(cross_cutting) != REQUIRED_MODEL_CROSS_CUTTING_PROTOCOLS:
        errors.append(
            "architecture contract model_cross_cutting_evidence must define "
            f"exactly {sorted(REQUIRED_MODEL_CROSS_CUTTING_PROTOCOLS)}"
        )
    for protocol, binding in cross_cutting.items():
        if not isinstance(binding, dict):
            errors.append(
                f"architecture cross-cutting protocol {protocol!r} must be an object"
            )
            continue
        domains = binding.get("boundary_domains")
        if not isinstance(domains, list) or not domains or not all(
            isinstance(domain, str) for domain in domains
        ):
            errors.append(
                f"architecture cross-cutting protocol {protocol!r} must name one "
                "or more boundary domains"
            )
            domains = []
        elif len(domains) != len(set(domains)):
            errors.append(
                f"architecture cross-cutting protocol {protocol!r} repeats a "
                "boundary domain"
            )
        for domain in domains:
            if not isinstance(model_boundary.get(domain), dict):
                errors.append(
                    f"architecture cross-cutting protocol {protocol!r} uses "
                    f"unknown or non-mapping boundary domain {domain!r}"
                )
        evidence = binding.get("behavior_ids")
        if not isinstance(evidence, list) or not evidence or not all(
            isinstance(behavior_id, str) for behavior_id in evidence
        ):
            errors.append(
                f"architecture cross-cutting protocol {protocol!r} must name one "
                "or more behavior IDs"
            )
            continue
        if len(evidence) != len(set(evidence)):
            errors.append(
                f"architecture cross-cutting protocol {protocol!r} repeats a "
                "behavior ID"
            )
        unknown_behaviors = set(evidence).difference(behavior_ids)
        if unknown_behaviors:
            errors.append(
                f"architecture cross-cutting protocol {protocol!r} uses unknown "
                f"behaviors: {sorted(unknown_behaviors)}"
            )
        unevidenced_behaviors = set(evidence).difference(evidenced_behaviors)
        if unevidenced_behaviors:
            errors.append(
                f"architecture cross-cutting protocol {protocol!r} uses behaviors "
                f"without registered evidence: {sorted(unevidenced_behaviors)}"
            )
    for family, evidence in model_family_evidence.items():
        if not isinstance(evidence, list) or not evidence or not all(
            isinstance(behavior_id, str) for behavior_id in evidence
        ):
            errors.append(
                f"architecture contract model family {family!r} must name one or "
                "more behavior IDs"
            )
            continue
        if len(evidence) != len(set(evidence)):
            errors.append(
                f"architecture contract model family {family!r} repeats behavior IDs"
            )
        unknown_behaviors = set(evidence).difference(behavior_ids)
        if unknown_behaviors:
            errors.append(
                f"architecture contract model family {family!r} uses unknown "
                f"behaviors: {sorted(unknown_behaviors)}"
            )
        unevidenced_behaviors = set(evidence).difference(evidenced_behaviors)
        if unevidenced_behaviors:
            errors.append(
                f"architecture contract model family {family!r} uses behaviors "
                f"without registered evidence: {sorted(unevidenced_behaviors)}"
            )

    script_rules, rule_errors = rust_enum_variants(
        "verification/src/cache.rs", "ScriptVerificationRules"
    )
    errors.extend(rule_errors)
    if not script_rules:
        errors.append("ScriptVerificationRules must have a discovered generation")
    errors.extend(
        require_source_symbols(
            "verification/src/cache.rs",
            [
                "struct TxVerificationCacheKey",
                "witness_hash: [u8; 32]",
                "script_rules: ScriptVerificationRules",
            ],
            "verification cache identity",
        )
    )

    root_families = contract.get("root_families", {})
    if not isinstance(root_families, dict) or set(root_families) != REQUIRED_ROOT_FAMILIES:
        errors.append("architecture contract must define exactly F1-F8")
    target_invariants = contract.get("target_invariants", {})
    required_target_invariants = target_invariant_ids(contract)
    residual_risks = contract.get("residual_risks")
    if not isinstance(residual_risks, dict) or set(residual_risks) != {
        f"R{number}" for number in range(2, 9)
    }:
        errors.append("architecture contract must define exactly stable residual risks R2-R8")

    evidence = invariant_unit_evidence(registry, required_target_invariants)
    if set(evidence) != required_target_invariants:
        errors.append("review evidence must map directly to every target invariant")

    required_links = {
        "authority_document": ["PERFORMANCE.md", "REVIEW_GUIDE.md", "VALIDATION.md"],
        "performance_document": ["ARCHITECTURE.md", "VALIDATION.md"],
        "review_guide": ["ARCHITECTURE.md", "VALIDATION.md"],
        "validation_document": ["ARCHITECTURE.md", "REVIEW_GUIDE.md"],
    }
    for field, links in required_links.items():
        value = contract.get(field)
        if not isinstance(value, str):
            errors.append(f"architecture contract {field} must be a path")
            continue
        try:
            contents = repo_path(value).read_text()
        except (OSError, ValueError) as error:
            errors.append(f"cannot read architecture document {value}: {error}")
            continue
        for link in links:
            if link not in contents:
                errors.append(f"architecture document {value} does not link {link}")
        required_math = {
            "authority_document": [
                "### 4.5 Executable mathematical proof kernel",
                "ObsKernel(CommitBatch_E(Omega, X))",
                "bounded semantic exchange",
            ],
            "performance_document": [
                "bounded semantic exchange",
                "one available wave",
            ],
            "review_guide": [
                "## Mathematical-model gate",
                "model-delta review",
            ],
            "validation_document": ["executable mathematical proof policy"],
        }
        for phrase in required_math[field]:
            if phrase not in contents:
                errors.append(
                    f"architecture document {value} is missing mathematical proof phrase {phrase!r}"
                )

    authority_value = contract.get("authority_document")
    if isinstance(authority_value, str):
        try:
            authority = repo_path(authority_value).read_text()
        except (OSError, ValueError):
            pass
        else:
            for family, name in root_families.items():
                if authority.count(f"| {family} {name} |") != 1:
                    errors.append(
                        f"architecture document must define {family} exactly once"
                    )
            for invariant, name in target_invariants.items():
                if authority.count(f"| {invariant} {name} |") != 1:
                    errors.append(
                        f"architecture document must define {invariant} exactly once"
                    )
            for risk, description in (
                residual_risks.items() if isinstance(residual_risks, dict) else ()
            ):
                if authority.count(f"| {risk} | {description} |") != 1:
                    errors.append(
                        f"architecture document must reproduce residual {risk} exactly once"
                    )
    return errors


def discover_tests(manifest: dict) -> tuple[set[str], int]:
    command = [
        "cargo",
        "nextest",
        "list",
        "-p",
        manifest["package"],
        "--message-format",
        "json",
    ]
    features = manifest.get("features", [])
    if features:
        command.extend(["--features", ",".join(features)])
    completed = subprocess.run(
        command,
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    if completed.returncode != 0:
        sys.stderr.write(completed.stdout)
        sys.stderr.write(completed.stderr)
        raise SystemExit(completed.returncode)
    try:
        listing = json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        raise SystemExit(f"nextest returned invalid JSON: {error}") from error
    suites = listing.get("rust-suites", {})
    tests: set[str] = set()
    for suite in suites.values():
        tests.update(suite.get("testcases", {}))
    return tests, int(listing.get("test-count", len(tests)))


def validate_test_anchors(
    registry: dict, tests: set[str], required_target_invariants: set[str]
) -> list[str]:
    errors: list[str] = []
    evidence = invariant_unit_evidence(registry, required_target_invariants)
    missing_invariants = required_target_invariants.difference(evidence)
    extra_invariants = set(evidence).difference(required_target_invariants)
    if missing_invariants:
        errors.append(f"missing invariant groups: {sorted(missing_invariants)}")
    if extra_invariants:
        errors.append(f"unknown invariant groups: {sorted(extra_invariants)}")
    for invariant, anchors in evidence.items():
        if not anchors:
            errors.append(f"{invariant} has no current test anchor")
        for anchor in anchors:
            matches = sorted(
                test for test in tests if test == anchor or test.endswith(f"::{anchor}")
            )
            if len(matches) != 1:
                errors.append(
                    f"{invariant} anchor {anchor!r} matched {len(matches)} tests: {matches}"
                )
    return errors


def validate_model_test_coverage(registry: dict, tests: set[str]) -> list[str]:
    """Require every discovered mathematical-model test in the evidence graph."""

    discovered = {test for test in tests if test.startswith(MODEL_TEST_PREFIX)}
    registered = {
        entry["test"]
        for entry in registry.get("unit_evidence", [])
        if isinstance(entry, dict)
        and isinstance(entry.get("test"), str)
        and entry["test"].startswith(MODEL_TEST_PREFIX)
    }
    missing = sorted(discovered.difference(registered))
    stale = sorted(registered.difference(discovered))
    errors = []
    if missing:
        errors.append(f"mathematical-model tests absent from behavior evidence: {missing}")
    if stale:
        errors.append(f"behavior evidence names stale mathematical-model tests: {stale}")
    return errors


def registry_path(manifest: dict) -> Path:
    value = manifest.get("behavior_registry")
    if not isinstance(value, str):
        raise SystemExit("manifest behavior_registry must be a repository-relative path")
    try:
        return repo_path(value)
    except ValueError as error:
        raise SystemExit(str(error)) from error


def inventory_path(manifest: dict) -> Path:
    inventory = manifest.get("test_inventory")
    if not isinstance(inventory, dict) or not isinstance(inventory.get("path"), str):
        raise SystemExit("manifest test_inventory.path must be a repository-relative path")
    path = (REPO_ROOT / inventory["path"]).resolve()
    try:
        path.relative_to(REPO_ROOT)
    except ValueError as error:
        raise SystemExit(f"test inventory escapes repository root: {path}") from error
    return path


def load_integration_impact(manifest: dict) -> tuple[set[str], list[str]]:
    impact, errors = load_repo_json(manifest.get("integration_impact"), "integration_impact")
    if impact is None:
        return set(), errors
    if impact.get("schema_version") != 1:
        errors.append("integration impact schema_version must be 1")
    groups = impact.get("groups")
    if not isinstance(groups, dict) or not groups:
        errors.append("integration impact groups must be a non-empty object")
        return set(), errors
    specs: set[str] = set()
    for path_value, names in groups.items():
        if not isinstance(path_value, str):
            errors.append(f"integration impact path must be a string: {path_value!r}")
            continue
        if not isinstance(names, list) or not names or not all(
            isinstance(name, str) and name for name in names
        ):
            errors.append(f"integration impact group {path_value!r} has invalid names")
            continue
        if names != sorted(names):
            errors.append(f"integration impact group {path_value!r} is not sorted")
        duplicates = specs.intersection(names)
        if duplicates:
            errors.append(f"integration impact repeats specs {sorted(duplicates)}")
        specs.update(names)
        try:
            repo_path(path_value).read_text()
        except (OSError, ValueError) as error:
            errors.append(f"cannot read integration impact source {path_value}: {error}")
    return specs, errors


def write_test_inventory(
    path: Path, unit_tests: set[str], managed_integration_specs: set[str]
) -> None:
    sections = (
        ("unit", sorted(unit_tests)),
        ("integration", sorted(managed_integration_specs)),
    )
    rendered: list[str] = []
    for index, (section, names) in enumerate(sections):
        if index:
            rendered.append("")
        rendered.append(f"[{section}]")
        rendered.extend(names)
    path.write_text("\n".join(rendered) + "\n")


def read_test_inventory(path: Path) -> tuple[dict[str, list[str]], list[str]]:
    try:
        lines = path.read_text().splitlines()
    except OSError as error:
        return {}, [f"cannot read test inventory {path}: {error}"]

    sections: dict[str, list[str]] = {}
    current: str | None = None
    errors: list[str] = []
    for line_number, raw_line in enumerate(lines, 1):
        line = raw_line.strip()
        if not line:
            continue
        if line.startswith("[") and line.endswith("]"):
            section = line[1:-1]
            if section not in ("unit", "integration"):
                errors.append(f"unknown test inventory section {line!r}")
                current = None
            elif section in sections:
                errors.append(f"duplicate test inventory section [{section}]")
                current = section
            else:
                sections[section] = []
                current = section
            continue
        if current is None:
            errors.append(
                f"test inventory name outside a section at line {line_number}: {line}"
            )
            continue
        sections[current].append(line)

    if list(sections) != ["unit", "integration"]:
        errors.append("test inventory sections must be ordered [unit], [integration]")
    for section in ("unit", "integration"):
        names = sections.get(section, [])
        if names != sorted(names):
            errors.append(f"[{section}] test inventory is not sorted")
        if len(names) != len(set(names)):
            errors.append(f"[{section}] test inventory contains duplicate names")
    return sections, errors


def load_integration_spec_list(path: Path) -> tuple[set[str], list[str]]:
    try:
        names = [line.strip() for line in path.read_text().splitlines() if line.strip()]
    except OSError as error:
        return set(), [f"cannot read integration spec list {path}: {error}"]
    errors: list[str] = []
    if names != sorted(names):
        errors.append("ckb-test integration spec list is not sorted")
    if len(names) != len(set(names)):
        errors.append("ckb-test integration spec list contains duplicate names")
    return set(names), errors


def validate_integration_impact_completeness(
    managed_specs: set[str], discovered_specs: set[str]
) -> list[str]:
    """Reject the two omission classes that caused the earlier partial run.

    Every registered type physically owned by specs/tx_pool is mandatory. In
    addition, registered public specs in any source file that directly calls a
    tx-pool ingress/query/relay/template boundary are mandatory. Specs with a
    runtime name rather than a same-named public type are conservatively
    mandatory too; today these are the six tx-pool signing/binary variants.
    """

    errors: list[str] = []
    specs_root = REPO_ROOT / "test" / "src" / "specs"
    public_types: dict[str, Path] = {}
    direct_candidates: set[str] = set()
    direct_pattern = re.compile(
        r"submit_transaction|send_transaction|get_raw_tx_pool|tx_pool_info|"
        r"get_pool_tx_detail_info|remove_transaction|clear_tx_pool|"
        r"get_block_template|notify_transaction|send_transaction_result|"
        r"relay_transaction"
    )
    for path in specs_root.rglob("*.rs"):
        source = path.read_text()
        declared = set(re.findall(r"pub struct\s+([A-Za-z_][A-Za-z0-9_]*)", source))
        for name in declared:
            public_types[name] = path
        if direct_pattern.search(source):
            direct_candidates.update(declared.intersection(discovered_specs))

    tx_pool_root = specs_root / "tx_pool"
    tx_pool_candidates = {
        name
        for name, path in public_types.items()
        if name in discovered_specs and path.is_relative_to(tx_pool_root)
    }
    runtime_named = discovered_specs.difference(public_types)
    required = tx_pool_candidates | direct_candidates | runtime_named
    missing = sorted(required.difference(managed_specs))
    if missing:
        errors.append(
            "registered tx-pool/direct-boundary integration specs absent from the "
            f"impact universe: {missing}"
        )
    return errors


def validate_test_inventory(
    manifest: dict,
    registry: dict,
    managed_integration_specs: set[str],
    unit_tests: set[str] | None,
    discovered_integration_specs: set[str] | None,
) -> list[str]:
    path = inventory_path(manifest)
    sections, errors = read_test_inventory(path)
    unit_names = sections.get("unit", [])
    integration_names = sections.get("integration", [])

    expected_units = set(unit_names)
    if unit_tests is not None:
        missing = sorted(expected_units.difference(unit_tests))
        unexpected = sorted(unit_tests.difference(expected_units))
        if missing:
            errors.append(f"unit inventory names no longer discovered: {missing}")
        if unexpected:
            errors.append(f"new unit tests absent from inventory: {unexpected}")

    registered_integration = {
        entry["anchor"] for entry in registry["integration_evidence"]
    }
    inventoried_integration = set(integration_names)
    missing_from_inventory = sorted(
        managed_integration_specs.difference(inventoried_integration)
    )
    stale_inventory = sorted(inventoried_integration.difference(managed_integration_specs))
    if missing_from_inventory:
        errors.append(
            f"managed integration specs absent from inventory: {missing_from_inventory}"
        )
    if stale_inventory:
        errors.append(
            f"integration inventory names absent from impact universe: {stale_inventory}"
        )
    evidence_outside_impact = sorted(registered_integration.difference(managed_integration_specs))
    if evidence_outside_impact:
        errors.append(
            f"security integration evidence absent from impact universe: {evidence_outside_impact}"
        )
    if discovered_integration_specs is not None:
        absent_from_runner = sorted(
            inventoried_integration.difference(discovered_integration_specs)
        )
        if absent_from_runner:
            errors.append(
                f"managed integration specs absent from ckb-test --list-specs: "
                f"{absent_from_runner}"
            )
        errors.extend(
            validate_integration_impact_completeness(
                managed_integration_specs, discovered_integration_specs
            )
        )
    return errors


def main() -> int:
    args = parse_args()
    if args.integration_only and args.integration_spec_list is None:
        raise SystemExit("--integration-only requires --integration-spec-list")
    if args.integration_only and args.update_inventory:
        raise SystemExit("--integration-only cannot be combined with --update-inventory")
    manifest = load_manifest(args.manifest)
    if manifest.get("schema_version") != 12:
        raise SystemExit("security manifest schema_version must be 12")
    if "evidence" in manifest or "source_anchors" in manifest:
        raise SystemExit(
            "security manifest may not duplicate evidence owned by behavior_registry"
        )
    registry = load_registry(registry_path(manifest))
    contract, contract_errors = load_repo_json(
        manifest.get("architecture_contract"), "architecture_contract"
    )
    errors = list(contract_errors)
    required_target_invariants = (
        target_invariant_ids(contract) if contract is not None else set()
    )
    errors.extend(
        validate_registry(
            registry, required_invariants=required_target_invariants or None
        )
    )
    if contract is not None:
        errors.extend(validate_architecture_contract(contract, registry))
        errors.extend(validate_convergence_status(manifest, contract))
        errors.extend(validate_convergence_canaries(manifest, contract))
        errors.extend(validate_construction_root_families(manifest, contract))
        errors.extend(
            validate_construction_root_family_canaries(manifest, contract)
        )
        errors.extend(validate_release_boundary_status(manifest, contract, registry))
        errors.extend(validate_release_boundary_canaries(manifest, contract, registry))
        errors.extend(validate_release_basis_evidence(manifest, contract))
        errors.extend(validate_workload_matrix_evidence(contract))
        errors.extend(validate_normal_form_partition_evidence(manifest, contract))
        mutation_acceptance = manifest.get("mutation_acceptance")
        errors.extend(
            validate_mutation_acceptance(mutation_acceptance, contract, registry)
        )
        if isinstance(mutation_acceptance, dict):
            errors.extend(
                validate_mutation_acceptance_canaries(
                    mutation_acceptance, contract, registry
                )
            )
    managed_integration_specs, impact_errors = load_integration_impact(manifest)
    errors.extend(impact_errors)
    discovered_integration_specs: set[str] | None = None
    if args.integration_spec_list is not None:
        discovered_integration_specs, integration_errors = load_integration_spec_list(
            args.integration_spec_list
        )
        errors.extend(integration_errors)

    test_count: int | None = None
    if args.integration_only:
        tests = None
    else:
        tests, test_count = discover_tests(manifest)
        if args.update_inventory:
            write_test_inventory(
                inventory_path(manifest), tests, managed_integration_specs
            )
        errors.extend(
            validate_test_anchors(registry, tests, required_target_invariants)
        )
        errors.extend(validate_model_test_coverage(registry, tests))
    errors.extend(
        validate_test_inventory(
            manifest,
            registry,
            managed_integration_specs,
            tests,
            discovered_integration_specs,
        )
    )
    blockers = manifest.get("release_blockers", [])
    if args.release and blockers:
        errors.extend(
            f"release blocker {blocker['id']}: {blocker['reason']}" for blocker in blockers
        )
    if errors:
        for error in errors:
            print(f"error: {error}", file=sys.stderr)
        return 1
    if args.integration_only:
        print(
            f"validated {len(managed_integration_specs)} managed integration specs "
            "against ckb-test --list-specs"
        )
        return 0

    if test_count is None:
        print("error: nextest discovery produced no test-count result", file=sys.stderr)
        return 1
    evidence = invariant_unit_evidence(registry, required_target_invariants)
    references = sum(map(len, evidence.values()))
    unique_tests = len(registry["unit_evidence"])
    source_anchors = len(registry["integration_evidence"])
    convergence_status = manifest["convergence_status"]
    convergence_phases = contract["convergence_protocol"]["phase_dag"]
    completed_convergence = convergence_status["completed_phases"]
    construction_rank = convergence_status["construction_rank"]
    rho_c = tuple(
        len(construction_rank[field])
        for field in contract["convergence_protocol"]["construction_rank"]
    )
    print(
        f"validated {references} invariant references covering {unique_tests} unique Rust "
        f"tests, {source_anchors} security integration anchors and "
        f"{len(managed_integration_specs)} managed integration specs against "
        f"{test_count} nextest tests; "
        f"convergence={convergence_status['state']}:"
        f"{convergence_status['active_phase']} "
        f"({len(completed_convergence)}/{len(convergence_phases)} phases); "
        f"rho_C={rho_c}; "
        f"{len(blockers)} explicit release blockers remain"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
