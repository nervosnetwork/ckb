#!/usr/bin/env python3
"""Validate the current tx-pool security evidence against nextest discovery."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import subprocess
import sys

from check_tx_pool_review_guide import (
    invariant_unit_evidence,
    load_registry,
    repo_path,
    validate_registry,
)


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_MANIFEST = REPO_ROOT / "tx-pool" / "security-regression-manifest.json"
REQUIRED_INVARIANTS = {f"I{number}" for number in range(1, 13)}


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


def integration_specs(registry: dict) -> set[str]:
    return {entry["anchor"] for entry in registry["integration_evidence"]}


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


def validate_test_inventory(
    manifest: dict,
    registry: dict,
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

    registered_integration = integration_specs(registry)
    inventoried_integration = set(integration_names)
    missing_from_inventory = sorted(
        registered_integration.difference(inventoried_integration)
    )
    stale_inventory = sorted(inventoried_integration.difference(registered_integration))
    if missing_from_inventory:
        errors.append(
            f"managed integration specs absent from inventory: {missing_from_inventory}"
        )
    if stale_inventory:
        errors.append(
            f"integration inventory names absent from behavior registry: {stale_inventory}"
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
    return errors


def main() -> int:
    args = parse_args()
    if args.integration_only and args.integration_spec_list is None:
        raise SystemExit("--integration-only requires --integration-spec-list")
    if args.integration_only and args.update_inventory:
        raise SystemExit("--integration-only cannot be combined with --update-inventory")
    manifest = load_manifest(args.manifest)
    if manifest.get("schema_version") != 4:
        raise SystemExit("security manifest schema_version must be 4")
    if "evidence" in manifest or "source_anchors" in manifest:
        raise SystemExit(
            "security manifest may not duplicate evidence owned by behavior_registry"
        )
    registry = load_registry(registry_path(manifest))
    errors = validate_registry(registry)
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
            write_test_inventory(inventory_path(manifest), tests, integration_specs(registry))
        errors.extend(validate_test_anchors(registry, tests))
    errors.extend(
        validate_test_inventory(
            manifest, registry, tests, discovered_integration_specs
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
            f"validated {len(integration_specs(registry))} managed integration specs "
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
        f"tests and {source_anchors} managed integration specs against "
        f"{test_count} nextest tests{delta}; "
        f"{len(blockers)} explicit release blockers remain"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
