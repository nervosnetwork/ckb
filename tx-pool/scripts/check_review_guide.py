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


REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_REGISTRY = REPO_ROOT / "tx-pool" / "review-behaviors.json"
START_MARKER = "<!-- BEGIN GENERATED: TX_POOL_BEHAVIORS -->"
END_MARKER = "<!-- END GENERATED: TX_POOL_BEHAVIORS -->"
BEHAVIOR_ID = re.compile(r"^TP-[A-Z]+-[0-9]{3}$")
INTEGRATION_SPEC = re.compile(r"^[A-Za-z][A-Za-z0-9_]*$")
MINIMUM_TEST_FILTER = re.compile(r"-E\s+['\"]test\(/\(([^)]+)\)/\)['\"]")
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


def load_integration_impact(registry: dict) -> dict:
    value = registry.get("integration_impact")
    if not isinstance(value, str):
        raise SystemExit("behavior registry must declare integration_impact")
    try:
        return json.loads(repo_path(value).read_text())
    except (OSError, ValueError, json.JSONDecodeError) as error:
        raise SystemExit(f"cannot load integration impact {value}: {error}") from error


def impact_specs(impact: dict) -> set[str]:
    groups = impact.get("groups", {})
    if not isinstance(groups, dict):
        return set()
    return {
        name
        for names in groups.values()
        if isinstance(names, list)
        for name in names
        if isinstance(name, str)
    }


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


def validate_minimum_command_arms(
    registry: dict, tests: set[str], evidence_name: str
) -> list[str]:
    """Require every documented nextest alternation arm to select evidence.

    Nextest intentionally treats a zero-match alternation arm as harmless when
    another arm matches. Review commands are security anchors, so that normal
    runner behavior would otherwise hide a renamed or deleted regression.
    """

    errors: list[str] = []
    for behavior in registry.get("behaviors", []):
        behavior_id = behavior.get("id", "<unknown>")
        command = behavior.get("minimum_command")
        if not isinstance(command, str):
            continue
        matches = MINIMUM_TEST_FILTER.findall(command)
        if len(matches) != 1:
            errors.append(
                f"{behavior_id} minimum_command must contain exactly one supported "
                "test(/(arm|...)/) filter"
            )
            continue
        arms = matches[0].split("|")
        if len(arms) != len(set(arms)):
            errors.append(f"{behavior_id} minimum_command repeats a regex arm")
        for arm in arms:
            if not arm:
                errors.append(f"{behavior_id} minimum_command has an empty regex arm")
                continue
            try:
                compiled = re.compile(arm)
            except re.error as error:
                errors.append(
                    f"{behavior_id} minimum_command arm {arm!r} is invalid: {error}"
                )
                continue
            if not any(compiled.search(test) for test in tests):
                errors.append(
                    f"{behavior_id} minimum_command arm {arm!r} matches no "
                    f"{evidence_name}"
                )
    return errors


def validate_registry(registry: dict, impact: dict | None = None) -> list[str]:
    errors: list[str] = []
    if registry.get("schema_version") != 2:
        errors.append("behavior registry schema_version must be 2")

    if impact is None:
        try:
            impact = load_integration_impact(registry)
        except SystemExit as error:
            errors.append(str(error))
            impact = {}
    if impact.get("schema_version") != 1:
        errors.append("integration impact schema_version must be 1")
    groups = impact.get("groups")
    if not isinstance(groups, dict) or not groups:
        errors.append("integration impact groups must be a non-empty object")
        groups = {}
    seen_impact: set[str] = set()
    for path_value, names in groups.items():
        if not isinstance(path_value, str):
            errors.append(f"integration impact path is invalid: {path_value!r}")
            continue
        try:
            path = repo_path(path_value)
        except ValueError as error:
            errors.append(str(error))
            continue
        if not path.is_file():
            errors.append(f"integration impact source does not exist: {path_value}")
        if not _nonempty_strings(names):
            errors.append(f"integration impact group {path_value} has no specs")
            continue
        if names != sorted(names):
            errors.append(f"integration impact group {path_value} is not sorted")
        for name in names:
            if not INTEGRATION_SPEC.fullmatch(name):
                errors.append(f"invalid integration impact spec: {name!r}")
            if name in seen_impact:
                errors.append(f"duplicate integration impact spec: {name}")
            seen_impact.add(name)
    if len(seen_impact) != 150:
        errors.append(
            f"integration impact must contain the managed count 150, found {len(seen_impact)}"
        )

    runner = registry.get("integration_runner")
    if not isinstance(runner, dict):
        errors.append("behavior registry must declare integration_runner")
    else:
        make_target = runner.get("make_target")
        arguments_variable = runner.get("arguments_variable")
        common_arguments = runner.get("common_arguments")
        if not isinstance(make_target, str) or not make_target.strip():
            errors.append("integration_runner has no make_target")
        if not isinstance(arguments_variable, str) or not arguments_variable.strip():
            errors.append("integration_runner has no arguments_variable")
        if not _nonempty_strings(common_arguments):
            errors.append("integration_runner has no common_arguments")
        try:
            makefile = (REPO_ROOT / "Makefile").read_text()
        except OSError as error:
            errors.append(f"cannot read Makefile for integration runner: {error}")
        else:
            if isinstance(make_target, str) and re.search(
                rf"(?m)^{re.escape(make_target)}\s*:", makefile
            ) is None:
                errors.append(f"integration make target is absent: {make_target}")
            if (
                isinstance(arguments_variable, str)
                and arguments_variable not in makefile
            ):
                errors.append(
                    f"integration argument variable is absent from Makefile: "
                    f"{arguments_variable}"
                )

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

    unit_evidence_by_test = {
        entry["test"]: entry
        for entry in unit_evidence
        if isinstance(entry, dict) and isinstance(entry.get("test"), str)
    }
    errors.extend(
        validate_minimum_command_arms(
            registry, seen_tests, "registered unit evidence anchor"
        )
    )

    integration_evidence = registry.get("integration_evidence")
    if not isinstance(integration_evidence, list):
        errors.append("behavior registry integration_evidence must be a list")
        integration_evidence = []
    for entry in integration_evidence:
        if not isinstance(entry, dict):
            errors.append(f"invalid integration evidence entry: {entry!r}")
            continue
        spec_id = entry.get("id")
        integration_behavior_ids = entry.get("behavior_ids")
        path_value = entry.get("path")
        anchor = entry.get("anchor")
        invariants = entry.get("invariants")
        unit_anchors = entry.get("unit_anchors")
        boundary = entry.get("boundary")
        if not isinstance(spec_id, str) or not spec_id.strip():
            errors.append(f"integration evidence has no ID: {entry!r}")
        elif spec_id in seen_specs:
            errors.append(f"duplicate integration evidence ID: {spec_id}")
        else:
            seen_specs.add(spec_id)
        if not _nonempty_strings(integration_behavior_ids):
            errors.append(f"integration evidence {spec_id!r} has no behavior_ids")
            integration_behavior_ids = []
        elif len(integration_behavior_ids) != len(set(integration_behavior_ids)):
            errors.append(f"integration evidence {spec_id!r} repeats behavior IDs")
        for behavior_id in integration_behavior_ids:
            if behavior_id not in behavior_ids:
                errors.append(
                    f"integration evidence {spec_id!r} uses unknown behavior "
                    f"{behavior_id!r}"
                )
            else:
                referenced_behaviors.add(behavior_id)
        if not isinstance(boundary, str) or not boundary.strip():
            errors.append(f"integration evidence {spec_id!r} has no boundary assertion")
        if not _nonempty_strings(unit_anchors):
            errors.append(f"integration evidence {spec_id!r} has no paired unit anchors")
            unit_anchors = []
        elif len(unit_anchors) != len(set(unit_anchors)):
            errors.append(f"integration evidence {spec_id!r} repeats unit anchors")
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
        if INTEGRATION_SPEC.fullmatch(anchor) is None:
            errors.append(
                f"integration evidence {spec_id!r} has invalid runner name {anchor!r}"
            )
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
        paired_invariants: set[str] = set()
        for unit_anchor in unit_anchors:
            unit_entry = unit_evidence_by_test.get(unit_anchor)
            if unit_entry is None:
                errors.append(
                    f"integration evidence {spec_id!r} pairs unknown unit anchor "
                    f"{unit_anchor!r}"
                )
                continue
            if unit_entry["behavior_id"] not in integration_behavior_ids:
                errors.append(
                    f"integration evidence {spec_id!r} pairs unit anchor {unit_anchor!r} "
                    f"from unrelated behavior {unit_entry['behavior_id']!r}"
                )
            paired_invariants.update(unit_entry["invariants"])
        if isinstance(invariants, list):
            uncovered = sorted(set(invariants).difference(paired_invariants))
            if uncovered:
                errors.append(
                    f"integration evidence {spec_id!r} has invariants without exact "
                    f"paired unit coverage: {uncovered}"
                )

    evidence_anchors = {anchor for _, anchor in seen_source_anchors}
    missing_impact = evidence_anchors.difference(seen_impact)
    if missing_impact:
        errors.append(
            f"security integration evidence absent from impact universe: "
            f"{sorted(missing_impact)}"
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


def integration_command(registry: dict, specs: list[str]) -> str:
    runner = registry["integration_runner"]
    arguments = [*runner["common_arguments"], *specs]
    return (
        f"make {runner['make_target']} {runner['arguments_variable']}="
        f"'{ ' '.join(arguments) }'"
    )


def render_generated(registry: dict, impact: dict) -> str:
    units: dict[str, list[dict]] = defaultdict(list)
    specs: dict[str, list[dict]] = defaultdict(list)
    for entry in registry["unit_evidence"]:
        units[entry["behavior_id"]].append(entry)
    for entry in registry["integration_evidence"]:
        for behavior_id in entry["behavior_ids"]:
            specs[behavior_id].append(entry)

    all_impact_specs = sorted(impact_specs(impact))
    lines = [
        "### Managed process suite",
        "",
        f"The {len(registry['integration_evidence'])} focused security anchors are the minimum process gate for the mapped behavior rows:",
        "",
        f"`{integration_command(registry, [entry['anchor'] for entry in registry['integration_evidence']])}`",
        "",
        f"The complete tx-pool impact universe contains {len(all_impact_specs)} specs. P6 and release CI run the exact inventory through:",
        "",
        f"`{integration_command(registry, all_impact_specs)}`",
        "",
        "The security validator checks the same `[integration]` inventory against the executable `ckb-test --list-specs` output in integration CI. The universe deliberately includes mining, RPC, relay, fork/reorg, DAO and hardfork transaction-ingress boundaries instead of treating `test/src/specs/tx_pool` as complete.",
        "",
        "| Integration source | Managed specs |",
        "|---|---|",
    ]
    for path_value, names in impact["groups"].items():
        lines.append(
            f"| `{path_value}` | " + ", ".join(f"`{name}`" for name in names) + " |"
        )
    lines.extend([
        "",
        "### Behavior index",
        "",
        "| ID | Change surfaces | Required behavior | Hostile/failure case | Invariants | Reviewer gate | Performance bound |",
        "|---|---|---|---|---|---|---|",
    ])
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
                    f"- `{evidence['id']}`: `{evidence['path']}::{evidence['anchor']}` "
                    f"({invariants}) — {_markdown(evidence['boundary'])} "
                    f"Paired units: {', '.join(f'`{anchor}`' for anchor in evidence['unit_anchors'])}. "
                    f"Command: `{integration_command(registry, [evidence['anchor']])}`"
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
    impact = load_integration_impact(registry)
    errors = validate_registry(registry, impact)
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
    expected = render_generated(registry, impact)
    if args.write:
        rewritten = guide[:start] + "\n\n" + expected + "\n\n" + guide[end:]
        guide_path.write_text(rewritten)
    elif actual != expected:
        print(
            "error: REVIEW_GUIDE.md generated evidence drifted; run "
            "python3 tx-pool/scripts/check_review_guide.py --write",
            file=sys.stderr,
        )
        return 1

    references = sum(len(entry["invariants"]) for entry in registry["unit_evidence"])
    print(
        f"validated {len(registry['behaviors'])} tx-pool behaviors, "
        f"{len(registry['unit_evidence'])} unique Rust tests / {references} invariant "
        f"references, {len(registry['integration_evidence'])} security integration anchors, "
        f"and {len(impact_specs(impact))} managed integration specs"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
