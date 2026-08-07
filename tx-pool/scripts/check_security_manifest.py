#!/usr/bin/env python3
"""Validate the current tx-pool security evidence against nextest discovery."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import subprocess
import sys

from check_review_guide import (
    invariant_unit_evidence,
    load_registry,
    repo_path,
    validate_registry,
)


REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_MANIFEST = REPO_ROOT / "tx-pool" / "security-regression-manifest.json"
REQUIRED_ROOT_FAMILIES = {f"F{number}" for number in range(1, 9)}
REQUIRED_TARGET_INVARIANTS = {f"T{number}" for number in range(1, 14)}
REQUIRED_PROOF_POLICY = {
    "primary_evidence": "executable_mathematical_model_and_mechanical_check_before_prose",
    "system_transition": "total_Step_over_AuthoritySlot_P_D_L_with_KernelStep_over_Omega_A_K",
    "batch_equivalence": "ObsKernel_CommitBatch_Omega_equals_no_interleave_canonical_KernelStep_fold",
    "conservation": "exact_owner_charge_effect_capability_and_compute_permit_conservation",
    "progress": "per_obligation_rank_or_monotonic_level_under_named_fairness_premises",
    "model_review": "phase_boundary_model_delta_and_refinement_audit",
    "prose_role": "trusted_boundaries_assumptions_and_rationale_only",
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


def validate_architecture_contract(manifest: dict, registry: dict) -> list[str]:
    contract, errors = load_repo_json(
        manifest.get("architecture_contract"), "architecture_contract"
    )
    if contract is None:
        return errors
    if contract.get("schema_version") != 8:
        errors.append("architecture contract schema_version must be 8")

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
    if (
        not isinstance(target_invariants, dict)
        or set(target_invariants) != REQUIRED_TARGET_INVARIANTS
    ):
        errors.append("architecture contract must define exactly T1-T13")
    residual_risks = contract.get("residual_risks")
    if not isinstance(residual_risks, dict) or set(residual_risks) != {
        f"R{number}" for number in range(2, 9)
    }:
        errors.append("architecture contract must define exactly stable residual risks R2-R8")

    evidence = invariant_unit_evidence(registry)
    if set(evidence) != REQUIRED_TARGET_INVARIANTS:
        errors.append("review evidence must map directly to exactly T1-T13")

    required_links = {
        "authority_document": ["REVIEW_GUIDE.md", "VALIDATION.md"],
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


def validate_test_anchors(registry: dict, tests: set[str]) -> list[str]:
    errors: list[str] = []
    evidence = invariant_unit_evidence(registry)
    missing_invariants = REQUIRED_TARGET_INVARIANTS.difference(evidence)
    extra_invariants = set(evidence).difference(REQUIRED_TARGET_INVARIANTS)
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
    if manifest.get("schema_version") != 7:
        raise SystemExit("security manifest schema_version must be 7")
    if "evidence" in manifest or "source_anchors" in manifest:
        raise SystemExit(
            "security manifest may not duplicate evidence owned by behavior_registry"
        )
    registry = load_registry(registry_path(manifest))
    errors = validate_registry(registry)
    errors.extend(validate_architecture_contract(manifest, registry))
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
        errors.extend(validate_test_anchors(registry, tests))
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
    evidence = invariant_unit_evidence(registry)
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
