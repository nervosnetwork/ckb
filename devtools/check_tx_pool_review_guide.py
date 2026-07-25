#!/usr/bin/env python3
"""Validate and render the tx-pool test-driven review guide.

The behavior registry is the single machine-readable mapping from a stable
TP-* review behavior to Rust and process-level evidence.  The security and
test-layout validators import the helpers in this file instead of maintaining
parallel evidence lists.
"""

from __future__ import annotations

import argparse
from collections import defaultdict
import json
from pathlib import Path
import re
import sys


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_REGISTRY = REPO_ROOT / "tx-pool" / "review-behaviors.json"
START_MARKER = "<!-- BEGIN GENERATED: TX_POOL_BEHAVIORS -->"
END_MARKER = "<!-- END GENERATED: TX_POOL_BEHAVIORS -->"
BEHAVIOR_ID = re.compile(r"^TP-[A-Z]+-[0-9]{3}$")
REQUIRED_INVARIANTS = {f"I{number}" for number in range(1, 13)}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--registry", type=Path, default=DEFAULT_REGISTRY)
    parser.add_argument("--guide", type=Path)
    parser.add_argument(
        "--write",
        action="store_true",
        help="rewrite only the generated guide section from the registry",
    )
    return parser.parse_args()


def load_registry(path: Path = DEFAULT_REGISTRY) -> dict:
    try:
        return json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise SystemExit(f"cannot load tx-pool behavior registry {path}: {error}") from error


def repo_path(value: str) -> Path:
    path = (REPO_ROOT / value).resolve()
    try:
        path.relative_to(REPO_ROOT)
    except ValueError as error:
        raise ValueError(f"path escapes repository root: {value}") from error
    return path


def _nonempty_strings(value: object) -> bool:
    return (
        isinstance(value, list)
        and bool(value)
        and all(isinstance(item, str) and bool(item.strip()) for item in value)
    )


def validate_registry(registry: dict) -> list[str]:
    errors: list[str] = []
    if registry.get("schema_version") != 1:
        errors.append("behavior registry schema_version must be 1")

    guide = registry.get("guide")
    if not isinstance(guide, str):
        errors.append("behavior registry guide must be a repository-relative path")
    else:
        try:
            guide_path = repo_path(guide)
        except ValueError as error:
            errors.append(str(error))
        else:
            if not guide_path.is_file():
                errors.append(f"review guide does not exist: {guide}")

    behavior_ids: set[str] = set()
    behaviors = registry.get("behaviors")
    if not isinstance(behaviors, list) or not behaviors:
        return errors + ["behavior registry must contain a non-empty behaviors list"]
    for entry in behaviors:
        if not isinstance(entry, dict):
            errors.append(f"invalid behavior entry: {entry!r}")
            continue
        behavior_id = entry.get("id")
        if not isinstance(behavior_id, str) or not BEHAVIOR_ID.fullmatch(behavior_id):
            errors.append(f"invalid behavior ID: {behavior_id!r}")
            continue
        if behavior_id in behavior_ids:
            errors.append(f"duplicate behavior ID: {behavior_id}")
        behavior_ids.add(behavior_id)
        for field in (
            "title",
            "required_behavior",
            "hostile_case",
            "minimum_command",
            "performance_bound",
        ):
            value = entry.get(field)
            if not isinstance(value, str) or not value.strip():
                errors.append(f"{behavior_id} has no {field}")
        for field in ("change_surfaces", "reviewer_questions"):
            if not _nonempty_strings(entry.get(field)):
                errors.append(f"{behavior_id} has no reviewable {field}")
        for surface in entry.get("change_surfaces", []):
            if not isinstance(surface, str):
                continue
            try:
                path = repo_path(surface)
            except ValueError as error:
                errors.append(f"{behavior_id}: {error}")
                continue
            if not path.exists():
                errors.append(f"{behavior_id} change surface does not exist: {surface}")

    seen_tests: set[str] = set()
    seen_specs: set[str] = set()
    seen_source_anchors: set[tuple[str, str]] = set()
    referenced_behaviors: set[str] = set()
    covered_invariants: set[str] = set()
    unit_evidence = registry.get("unit_evidence")
    if not isinstance(unit_evidence, list) or not unit_evidence:
        errors.append("behavior registry must contain unit_evidence")
        unit_evidence = []
    for entry in unit_evidence:
        if not isinstance(entry, dict):
            errors.append(f"invalid unit evidence entry: {entry!r}")
            continue
        test = entry.get("test")
        behavior_id = entry.get("behavior_id")
        invariants = entry.get("invariants")
        if not isinstance(test, str) or not test.strip():
            errors.append(f"unit evidence has no test anchor: {entry!r}")
        elif test in seen_tests:
            errors.append(f"duplicate unit evidence anchor: {test}")
        else:
            seen_tests.add(test)
        if behavior_id not in behavior_ids:
            errors.append(f"unit evidence {test!r} uses unknown behavior {behavior_id!r}")
        else:
            referenced_behaviors.add(behavior_id)
        if not _nonempty_strings(invariants):
            errors.append(f"unit evidence {test!r} has no invariants")
        else:
            unknown = set(invariants).difference(REQUIRED_INVARIANTS)
            if unknown:
                errors.append(f"unit evidence {test!r} has unknown invariants {sorted(unknown)}")
            covered_invariants.update(invariants)

    integration_evidence = registry.get("integration_evidence")
    if not isinstance(integration_evidence, list):
        errors.append("behavior registry integration_evidence must be a list")
        integration_evidence = []
    for entry in integration_evidence:
        if not isinstance(entry, dict):
            errors.append(f"invalid integration evidence entry: {entry!r}")
            continue
        spec_id = entry.get("id")
        behavior_id = entry.get("behavior_id")
        path_value = entry.get("path")
        anchor = entry.get("anchor")
        invariants = entry.get("invariants")
        if not isinstance(spec_id, str) or not spec_id.strip():
            errors.append(f"integration evidence has no ID: {entry!r}")
        elif spec_id in seen_specs:
            errors.append(f"duplicate integration evidence ID: {spec_id}")
        else:
            seen_specs.add(spec_id)
        if behavior_id not in behavior_ids:
            errors.append(f"integration evidence {spec_id!r} uses unknown behavior {behavior_id!r}")
        else:
            referenced_behaviors.add(behavior_id)
        if not _nonempty_strings(invariants):
            errors.append(f"integration evidence {spec_id!r} has no invariants")
        else:
            unknown = set(invariants).difference(REQUIRED_INVARIANTS)
            if unknown:
                errors.append(
                    f"integration evidence {spec_id!r} has unknown invariants {sorted(unknown)}"
                )
            covered_invariants.update(invariants)
        if not isinstance(path_value, str) or not isinstance(anchor, str):
            errors.append(f"integration evidence {spec_id!r} has no path/anchor")
            continue
        source_key = (path_value, anchor)
        if source_key in seen_source_anchors:
            errors.append(f"duplicate integration source anchor: {source_key}")
        seen_source_anchors.add(source_key)
        try:
            path = repo_path(path_value)
            source = path.read_text()
        except (ValueError, OSError) as error:
            errors.append(f"integration evidence {spec_id!r} cannot read {path_value}: {error}")
            continue
        if anchor not in source:
            errors.append(
                f"integration evidence {spec_id!r} anchor {anchor!r} is absent from {path_value}"
            )

    unreferenced = behavior_ids.difference(referenced_behaviors)
    if unreferenced:
        errors.append(f"behaviors without executable evidence: {sorted(unreferenced)}")
    missing_invariants = REQUIRED_INVARIANTS.difference(covered_invariants)
    if missing_invariants:
        errors.append(f"invariants without executable evidence: {sorted(missing_invariants)}")
    return errors


def behavior_ids(registry: dict) -> set[str]:
    return {entry["id"] for entry in registry["behaviors"]}


def invariant_unit_evidence(registry: dict) -> dict[str, list[str]]:
    evidence = {invariant: [] for invariant in sorted(REQUIRED_INVARIANTS)}
    for entry in registry["unit_evidence"]:
        for invariant in entry["invariants"]:
            evidence[invariant].append(entry["test"])
    return evidence


def _invariant_key(value: str) -> int:
    return int(value[1:])


def _markdown(value: str) -> str:
    return value.replace("|", "\\|").replace("\n", "<br>")


def render_generated(registry: dict) -> str:
    units: dict[str, list[dict]] = defaultdict(list)
    specs: dict[str, list[dict]] = defaultdict(list)
    for entry in registry["unit_evidence"]:
        units[entry["behavior_id"]].append(entry)
    for entry in registry["integration_evidence"]:
        specs[entry["behavior_id"]].append(entry)

    lines = [
        "### Behavior index",
        "",
        "| ID | Change surfaces | Required behavior | Hostile/failure case | Invariants | Reviewer gate | Performance bound |",
        "|---|---|---|---|---|---|---|",
    ]
    for behavior in registry["behaviors"]:
        behavior_id = behavior["id"]
        invariants = sorted(
            {
                invariant
                for evidence in (*units[behavior_id], *specs[behavior_id])
                for invariant in evidence["invariants"]
            },
            key=_invariant_key,
        )
        surfaces = "<br>".join(f"`{_markdown(path)}`" for path in behavior["change_surfaces"])
        questions = "<br>".join(
            f"- {_markdown(question)}" for question in behavior["reviewer_questions"]
        )
        lines.append(
            "| "
            + " | ".join(
                (
                    f"`{behavior_id}` {behavior['title']}",
                    surfaces,
                    _markdown(behavior["required_behavior"]),
                    _markdown(behavior["hostile_case"]),
                    ", ".join(invariants),
                    questions,
                    _markdown(behavior["performance_bound"]),
                )
            )
            + " |"
        )

    lines.extend(("", "### Executable evidence", ""))
    for behavior in registry["behaviors"]:
        behavior_id = behavior["id"]
        lines.extend((f"#### `{behavior_id}` — {behavior['title']}", ""))
        lines.append(f"Minimum command: `{behavior['minimum_command']}`")
        lines.append("")
        lines.append("Rust evidence:")
        lines.append("")
        for evidence in sorted(units[behavior_id], key=lambda value: value["test"]):
            invariants = ", ".join(sorted(evidence["invariants"], key=_invariant_key))
            lines.append(f"- `{evidence['test']}` ({invariants})")
        if specs[behavior_id]:
            lines.extend(("", "Process-level evidence:", ""))
            for evidence in sorted(specs[behavior_id], key=lambda value: value["id"]):
                invariants = ", ".join(
                    sorted(evidence["invariants"], key=_invariant_key)
                )
                lines.append(
                    f"- `{evidence['id']}`: `{evidence['path']}::{evidence['anchor']}` ({invariants})"
                )
        lines.append("")
    return "\n".join(lines).rstrip()


def generated_region(guide: str) -> tuple[int, int, str] | None:
    if guide.count(START_MARKER) != 1 or guide.count(END_MARKER) != 1:
        return None
    start = guide.index(START_MARKER) + len(START_MARKER)
    end = guide.index(END_MARKER)
    if end < start:
        return None
    return start, end, guide[start:end].strip("\n")


def main() -> int:
    args = parse_args()
    registry = load_registry(args.registry)
    errors = validate_registry(registry)
    if errors:
        for error in errors:
            print(f"error: {error}", file=sys.stderr)
        return 1

    guide_path = args.guide or repo_path(registry["guide"])
    try:
        guide = guide_path.read_text()
    except OSError as error:
        print(f"error: cannot read review guide {guide_path}: {error}", file=sys.stderr)
        return 1
    region = generated_region(guide)
    if region is None:
        print("error: review guide must contain exactly one ordered generated region", file=sys.stderr)
        return 1
    start, end, actual = region
    expected = render_generated(registry)
    if args.write:
        rewritten = guide[:start] + "\n\n" + expected + "\n\n" + guide[end:]
        guide_path.write_text(rewritten)
    elif actual != expected:
        print(
            "error: REVIEW_GUIDE.md generated evidence drifted; run "
            "python3 devtools/check_tx_pool_review_guide.py --write",
            file=sys.stderr,
        )
        return 1

    references = sum(len(entry["invariants"]) for entry in registry["unit_evidence"])
    print(
        f"validated {len(registry['behaviors'])} tx-pool behaviors, "
        f"{len(registry['unit_evidence'])} unique Rust tests / {references} invariant "
        f"references, and {len(registry['integration_evidence'])} integration specs"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
