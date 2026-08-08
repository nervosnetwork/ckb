#!/usr/bin/env python3
"""Validate the current tx-pool security evidence against nextest discovery."""

from __future__ import annotations

import argparse
import copy
import json
from pathlib import Path
import re
import subprocess
import sys

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
MUTATION_SELECTOR_KINDS = {"all_methods", "method", "function", "struct_fields"}


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


def validate_selected_topology(contract: dict, registry: dict) -> list[str]:
    errors: list[str] = []
    topology = contract.get("selected_topology")
    slices_contract = contract.get("implementation_slices")
    release_surface = contract.get("release_surface")
    if not isinstance(topology, dict) or topology.get("schema_version") != 1:
        return ["architecture contract selected_topology schema_version must be 1"]
    if not isinstance(slices_contract, dict) or slices_contract.get("schema_version") != 1:
        return ["architecture contract implementation_slices schema_version must be 1"]
    if not isinstance(release_surface, dict) or release_surface.get("schema_version") != 2:
        return ["architecture contract release_surface schema_version must be 2"]
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
    if contract.get("schema_version") != 13:
        errors.append("architecture contract schema_version must be 13")
    errors.extend(validate_selected_topology(contract, registry))
    errors.extend(validate_selected_topology_canaries(contract, registry))
    errors.extend(validate_interruption_contract(contract, registry))

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
    if manifest.get("schema_version") != 8:
        raise SystemExit("security manifest schema_version must be 8")
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
    print(
        f"validated {references} invariant references covering {unique_tests} unique Rust "
        f"tests, {source_anchors} security integration anchors and "
        f"{len(managed_integration_specs)} managed integration specs against "
        f"{test_count} nextest tests; "
        f"{len(blockers)} explicit release blockers remain"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
