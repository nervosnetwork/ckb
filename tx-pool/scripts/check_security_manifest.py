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
                variants.add(names[-1])
    tail = body[segment_start:]
    names = re.findall(r"(?m)^\s*(?:#\[[^\n]*\]\s*)*([A-Z][A-Za-z0-9_]*)\b", tail)
    if names:
        variants.add(names[-1])
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


def validate_architecture_contract(manifest: dict, registry: dict) -> list[str]:
    contract, errors = load_repo_json(
        manifest.get("architecture_contract"), "architecture_contract"
    )
    if contract is None:
        return errors
    if contract.get("schema_version") != 4:
        errors.append("architecture contract schema_version must be 4")

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

    if set(contract.get("root_families", {})) != REQUIRED_ROOT_FAMILIES:
        errors.append("architecture contract must define exactly F1-F8")
    if set(contract.get("target_invariants", {})) != REQUIRED_TARGET_INVARIANTS:
        errors.append("architecture contract must define exactly T1-T13")
    residual_risks = contract.get("residual_risks")
    if not isinstance(residual_risks, dict) or set(residual_risks) != {
        f"R{number}" for number in range(1, 10)
    }:
        errors.append("architecture contract must define exactly current residual risks R1-R9")

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

    authority_value = contract.get("authority_document")
    if isinstance(authority_value, str):
        try:
            authority = repo_path(authority_value).read_text()
        except (OSError, ValueError):
            pass
        else:
            target_invariants = contract.get("target_invariants", {})
            for invariant, name in target_invariants.items():
                if authority.count(f"| {invariant} {name} |") != 1:
                    errors.append(
                        f"architecture document must define {invariant} exactly once"
                    )
            for risk in residual_risks if isinstance(residual_risks, dict) else ():
                if authority.count(f"| {risk} |") != 1:
                    errors.append(
                        f"architecture document must define residual {risk} exactly once"
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
