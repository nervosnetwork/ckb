#!/usr/bin/env python3
"""Validate the current tx-pool security evidence against nextest discovery."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import subprocess
import sys


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


def validate_test_anchors(manifest: dict, tests: set[str]) -> list[str]:
    errors: list[str] = []
    evidence = manifest.get("evidence", {})
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


def write_test_inventory(path: Path, tests: set[str]) -> None:
    path.write_text("".join(f"{test}\n" for test in sorted(tests)))


def validate_test_inventory(manifest: dict, tests: set[str]) -> list[str]:
    path = inventory_path(manifest)
    try:
        lines = [line for line in path.read_text().splitlines() if line]
    except OSError as error:
        return [f"cannot read test inventory {path}: {error}"]

    errors: list[str] = []
    if lines != sorted(lines):
        errors.append("test inventory is not sorted")
    if len(lines) != len(set(lines)):
        errors.append("test inventory contains duplicate names")
    expected_count = manifest["test_inventory"].get("test_count")
    if expected_count != len(lines):
        errors.append(
            f"test inventory declares {expected_count} tests but contains {len(lines)} names"
        )

    expected = set(lines)
    missing = sorted(expected.difference(tests))
    unexpected = sorted(tests.difference(expected))
    if missing:
        errors.append(f"test inventory names no longer discovered: {missing}")
    if unexpected:
        errors.append(f"new tests absent from test inventory: {unexpected}")
    return errors


def validate_source_anchors(manifest: dict) -> list[str]:
    errors: list[str] = []
    for entry in manifest.get("source_anchors", []):
        path = REPO_ROOT / entry["path"]
        try:
            source = path.read_text()
        except OSError as error:
            errors.append(f"{entry['id']}: cannot read {path}: {error}")
            continue
        if entry["anchor"] not in source:
            errors.append(
                f"{entry['id']}: {entry['anchor']!r} is absent from {entry['path']}"
            )
    return errors


def main() -> int:
    args = parse_args()
    manifest = load_manifest(args.manifest)
    tests, test_count = discover_tests(manifest)
    if args.update_inventory:
        write_test_inventory(inventory_path(manifest), tests)
    errors = validate_test_anchors(manifest, tests)
    errors.extend(validate_test_inventory(manifest, tests))
    errors.extend(validate_source_anchors(manifest))
    blockers = manifest.get("release_blockers", [])
    if args.release and blockers:
        errors.extend(
            f"release blocker {blocker['id']}: {blocker['reason']}" for blocker in blockers
        )
    if errors:
        for error in errors:
            print(f"error: {error}", file=sys.stderr)
        return 1
    baseline = manifest.get("baseline", {}).get("nextest_count")
    delta = "" if baseline is None else f" (baseline {baseline}, delta {test_count - baseline:+d})"
    references = sum(map(len, manifest["evidence"].values()))
    unique_tests = len(
        {anchor for anchors in manifest["evidence"].values() for anchor in anchors}
    )
    source_anchors = len(manifest.get("source_anchors", []))
    print(
        f"validated {references} invariant references covering {unique_tests} unique Rust "
        f"tests and {source_anchors} integration source anchors against "
        f"{test_count} nextest tests{delta}; "
        f"{len(blockers)} explicit release blockers remain"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
