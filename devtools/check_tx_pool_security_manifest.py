#!/usr/bin/env python3
"""Validate the current tx-pool security evidence against nextest discovery."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import subprocess
import sys

from check_tx_pool_review_guide import (
    invariant_unit_evidence,
    load_registry,
    repo_path,
    validate_minimum_command_arms,
    validate_registry,
)


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_MANIFEST = REPO_ROOT / "tx-pool" / "security-regression-manifest.json"
REQUIRED_INVARIANTS = {f"I{number}" for number in range(1, 13)}
REQUIRED_ROOT_FAMILIES = {f"F{number}" for number in range(1, 9)}
REQUIRED_TARGET_INVARIANTS = {f"T{number}" for number in range(1, 14)}
REQUIRED_PREPOOL_STATES = {
    "ResolveQueued",
    "ResolveLeased",
    "Wait",
    "VerifyQueued",
    "VerifyLeased",
    "Ready",
}
REQUIRED_PLAN_OUTCOMES = {"Apply", "Reject", "Backpressure", "Stale", "Duplicate"}
REQUIRED_READY_KEY = [
    "source_class_Remote_lt_Proposal_lt_Recovery",
    "fee_rate_u128_cross_product",
    "absolute_fee",
    "earlier_arrival",
    "smaller_full_hash",
    "entry_version",
]
REQUIRED_LOCK_ORDER = [
    "optional_serial_or_work_or_plan_permit",
    "effect_capacity_hint_released",
    "TxPool_read_or_write",
    "PrePoolKernel",
    "EffectJournal",
]
LEDGER_ROW = re.compile(
    r"^\|\s*(?P<id>\d+)\s*\|.*\|\s*(?P<invariants>"
    r"I(?:[1-9]|1[0-2])(?:,\s*I(?:[1-9]|1[0-2]))*)\s*\|$"
)


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


def validate_architecture_contract(manifest: dict, registry: dict) -> list[str]:
    contract, errors = load_repo_json(
        manifest.get("architecture_contract"), "architecture_contract"
    )
    if contract is None:
        return errors
    if contract.get("schema_version") != 1:
        errors.append("architecture contract schema_version must be 1")
    if contract.get("authorities") != ["TxPool", "PrePoolKernel"]:
        errors.append("architecture contract must declare exactly TxPool then PrePoolKernel")
    if _string_set(contract.get("prepool_states")) != REQUIRED_PREPOOL_STATES:
        errors.append("architecture contract prepool_states differ from the frozen six states")
    if _string_set(contract.get("plan_outcomes")) != REQUIRED_PLAN_OUTCOMES:
        errors.append("architecture contract PlanOutcome set is incomplete")
    if contract.get("ready_key") != REQUIRED_READY_KEY:
        errors.append("architecture contract ReadyKey order differs from the frozen order")
    if contract.get("ready_aging") != {
        "policy": "none",
        "remote_bound": "residency_deadline_and_budget",
        "trusted_bound": "bounded_chain_derived_ingress",
        "acceptance_gate": "P7_saturation_throughput_and_tail_latency",
    }:
        errors.append("architecture contract Ready aging trade-off is not explicit")
    if contract.get("lock_order") != REQUIRED_LOCK_ORDER:
        errors.append("architecture contract authority lock order differs from the frozen order")
    identity = contract.get("identity")
    if not isinstance(identity, dict) or identity != {
        "ownership_key": "full_tx_hash",
        "verification_cache_key": "wtx_hash",
        "proposal_short_id_role": "collision_aware_index_only",
        "entry_version": "process_global_non_reused_u128",
    }:
        errors.append("architecture contract identity domains differ from the frozen model")
    if set(contract.get("root_families", {})) != REQUIRED_ROOT_FAMILIES:
        errors.append("architecture contract must define exactly F1-F8")
    if set(contract.get("target_invariants", {})) != REQUIRED_TARGET_INVARIANTS:
        errors.append("architecture contract must define exactly T1-T13")

    bridge = contract.get("historical_invariant_bridge")
    if not isinstance(bridge, dict) or set(bridge) != REQUIRED_INVARIANTS:
        errors.append("architecture contract must bridge exactly I1-I12")
        bridge = {}
    behavior_ids = {entry["id"] for entry in registry.get("behaviors", [])}
    for invariant, mapping in bridge.items():
        if not isinstance(mapping, dict):
            errors.append(f"architecture bridge {invariant} must be an object")
            continue
        families = _string_set(mapping.get("root_families"))
        targets = _string_set(mapping.get("target_invariants"))
        behaviors = _string_set(mapping.get("behavior_ids"))
        if not families or not families <= REQUIRED_ROOT_FAMILIES:
            errors.append(f"architecture bridge {invariant} has invalid root families")
        if not targets or not targets <= REQUIRED_TARGET_INVARIANTS:
            errors.append(f"architecture bridge {invariant} has invalid target invariants")
        if not behaviors or not behaviors <= behavior_ids:
            errors.append(f"architecture bridge {invariant} has invalid behavior IDs")

    ledger_value = contract.get("historical_ledger")
    if not isinstance(ledger_value, str):
        errors.append("architecture contract historical_ledger must be a path")
        return errors
    try:
        ledger_lines = repo_path(ledger_value).read_text().splitlines()
    except (OSError, ValueError) as error:
        errors.append(f"cannot read architecture historical ledger: {error}")
        return errors
    rows: list[tuple[int, set[str]]] = []
    for line in ledger_lines:
        match = LEDGER_ROW.match(line)
        if match:
            rows.append(
                (
                    int(match.group("id")),
                    {item.strip() for item in match.group("invariants").split(",")},
                )
            )
    row_ids = [row_id for row_id, _ in rows]
    if row_ids != list(range(1, 168)):
        errors.append(
            "historical ledger must contain exactly ordered finding IDs 1-167 with I1-I12 mappings"
        )
    for row_id, invariants in rows:
        if not invariants or not invariants <= set(bridge):
            errors.append(f"historical ledger row {row_id} has no total architecture bridge")

    required_links = {
        "authority_document": ["REVIEW_GUIDE.md", "IMPLEMENTATION_PLAN.md"],
        "audit_document": ["ARCHITECTURE.md"],
        "review_guide": ["ARCHITECTURE.md", "IMPLEMENTATION_PLAN.md"],
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
    missing_invariants = REQUIRED_INVARIANTS.difference(evidence)
    extra_invariants = set(evidence).difference(REQUIRED_INVARIANTS)
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
    if len(specs) != 150:
        errors.append(f"integration impact must contain the managed count 150, found {len(specs)}")
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

    expected_unit_count = manifest["test_inventory"].get("unit_test_count")
    if expected_unit_count != len(unit_names):
        errors.append(
            f"test inventory declares {expected_unit_count} unit tests but contains "
            f"{len(unit_names)} names"
        )
    expected_integration_count = manifest["test_inventory"].get(
        "integration_spec_count"
    )
    if expected_integration_count != len(integration_names):
        errors.append(
            f"test inventory declares {expected_integration_count} integration specs but "
            f"contains {len(integration_names)} names"
        )

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
    if manifest.get("schema_version") != 5:
        raise SystemExit("security manifest schema_version must be 5")
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
        errors.extend(
            validate_minimum_command_arms(registry, tests, "discovered nextest test")
        )
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

    baseline = manifest.get("baseline", {}).get("nextest_count")
    assert test_count is not None
    delta = (
        ""
        if baseline is None
        else f" (baseline {baseline}, delta {test_count - baseline:+d})"
    )
    evidence = invariant_unit_evidence(registry)
    references = sum(map(len, evidence.values()))
    unique_tests = len(registry["unit_evidence"])
    source_anchors = len(registry["integration_evidence"])
    print(
        f"validated {references} invariant references covering {unique_tests} unique Rust "
        f"tests, {source_anchors} security integration anchors and "
        f"{len(managed_integration_specs)} managed integration specs against "
        f"{test_count} nextest tests{delta}; "
        f"{len(blockers)} explicit release blockers remain"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
