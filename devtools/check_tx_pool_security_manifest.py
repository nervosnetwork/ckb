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
    errors = validate_test_anchors(manifest, tests)
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
    print(
        f"validated {sum(map(len, manifest['evidence'].values()))} invariant anchors "
        f"against {test_count} nextest tests{delta}; "
        f"{len(blockers)} explicit release blockers remain"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
