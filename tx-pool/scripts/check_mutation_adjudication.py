#!/usr/bin/env python3
"""Validate the root-proof partition of the completed V1 mutation result.

The architecture contract owns semantic families and regex selectors. Exact
candidate rows, outcome counts and test names are derived from the existing
locks and generated test inventory; none may be copied into the contract.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import sys


REPO_ROOT = Path(__file__).resolve().parents[2]
CONTRACT = REPO_ROOT / "tx-pool" / "architecture-contract.json"
REGISTRY = REPO_ROOT / "tx-pool" / "review-behaviors.json"
INVENTORY = REPO_ROOT / "tx-pool" / "test-inventory.txt"
UNIT_START = '  "unit_evidence": ['
UNIT_END = '\n  ],\n  "workspace_evidence":'
DERIVED_FIELD_NAMES = {"candidate", "candidate_count", "count", "sha256"}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--write-evidence",
        action="store_true",
        help="mechanically synchronize generated model tests into unit_evidence",
    )
    return parser.parse_args()


def fail(message: str) -> None:
    raise SystemExit(message)


def load_json(path: Path, label: str) -> dict:
    try:
        value = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot load {label} {path}: {error}")
    if not isinstance(value, dict):
        fail(f"{label} must be a JSON object: {path}")
    return value


def repo_path(value: object, label: str) -> Path:
    if not isinstance(value, str) or not value:
        fail(f"{label} must be a repository-relative path")
    path = (REPO_ROOT / value).resolve()
    try:
        path.relative_to(REPO_ROOT)
    except ValueError as error:
        fail(f"{label} escapes the repository: {value}")
    return path


def unit_tests() -> set[str]:
    try:
        lines = INVENTORY.read_text().splitlines()
    except OSError as error:
        fail(f"cannot load generated test inventory: {error}")
    try:
        start = lines.index("[unit]") + 1
        end = lines.index("[integration]")
    except ValueError as error:
        fail("test inventory must contain ordered [unit] and [integration] sections")
    tests = {line.strip() for line in lines[start:end] if line.strip()}
    if not tests:
        fail("generated unit-test inventory is empty")
    return tests


def string_list(value: object, label: str, *, allow_empty: bool = False) -> list[str]:
    if not isinstance(value, list) or (not value and not allow_empty) or not all(
        isinstance(item, str) and item for item in value
    ):
        fail(f"{label} must be a {'possibly empty ' if allow_empty else 'nonempty '}string list")
    if len(value) != len(set(value)):
        fail(f"{label} contains duplicates")
    return value


def contains_derived_fact(value: object, path: str = "mutation_adjudication") -> str | None:
    if isinstance(value, dict):
        for key, nested in value.items():
            if key in DERIVED_FIELD_NAMES:
                return f"{path}.{key}"
            found = contains_derived_fact(nested, f"{path}.{key}")
            if found is not None:
                return found
    elif isinstance(value, list):
        for index, nested in enumerate(value):
            found = contains_derived_fact(nested, f"{path}[{index}]")
            if found is not None:
                return found
    return None


def compile_patterns(values: object, label: str, *, allow_empty: bool = False) -> list[re.Pattern[str]]:
    patterns = string_list(values, label, allow_empty=allow_empty)
    compiled: list[re.Pattern[str]] = []
    for pattern in patterns:
        if not pattern.startswith("^") or not pattern.endswith("$"):
            fail(f"{label} patterns must be anchored: {pattern!r}")
        try:
            compiled.append(re.compile(pattern))
        except re.error as error:
            fail(f"invalid {label} pattern {pattern!r}: {error}")
    return compiled


def matched_tests(patterns: list[re.Pattern[str]], tests: set[str], label: str) -> set[str]:
    matched: set[str] = set()
    for pattern in patterns:
        current = {test for test in tests if pattern.fullmatch(test)}
        if not current:
            fail(f"{label} pattern matches zero generated tests: {pattern.pattern!r}")
        matched.update(current)
    return matched


def evidence_map(registry: dict) -> dict[str, dict]:
    entries = registry.get("unit_evidence")
    if not isinstance(entries, list):
        fail("review behavior registry has no unit_evidence list")
    result: dict[str, dict] = {}
    for entry in entries:
        if not isinstance(entry, dict) or not isinstance(entry.get("test"), str):
            fail(f"invalid unit evidence row: {entry!r}")
        test = entry["test"]
        if test in result:
            fail(f"duplicate unit evidence test: {test}")
        result[test] = entry
    return result


def family_inputs(contract: dict, registry: dict, tests: set[str]) -> tuple[list[dict], dict[str, dict]]:
    adjudication = contract.get("mutation_adjudication")
    if not isinstance(adjudication, dict) or adjudication.get("schema_version") != 1:
        fail("architecture contract mutation_adjudication schema_version must be 1")
    copied = contains_derived_fact(adjudication)
    if copied is not None:
        fail(f"mutation adjudication copies a generated fact at {copied}")
    if adjudication.get("unacceptable_outcomes") != ["MissedMutant", "Timeout"]:
        fail("mutation adjudication must classify exactly MissedMutant and Timeout")
    families = adjudication.get("families")
    if not isinstance(families, list) or not families:
        fail("mutation adjudication must define nonempty proof families")

    behavior_ids = {
        entry.get("id")
        for entry in registry.get("behaviors", [])
        if isinstance(entry, dict) and isinstance(entry.get("id"), str)
    }
    invariant_ids = set(contract.get("target_invariants", {}))
    refinement = contract.get("refinement_inventory", {})
    model_roles = set(refinement.get("model_roots", {}).values())
    production_roles = set(refinement.get("production_roots", {}).values())
    registered = evidence_map(registry)
    seen_ids: set[str] = set()
    generated_owner: dict[str, dict] = {}

    required_text = {
        "semantic_fact",
        "falsifier",
        "parallelism",
        "added_cost",
        "deletion_counterexample",
    }
    for family in families:
        if not isinstance(family, dict):
            fail(f"invalid mutation adjudication family: {family!r}")
        family_id = family.get("id")
        if not isinstance(family_id, str) or not re.fullmatch(r"V1-B3-[A-Z0-9-]+", family_id):
            fail(f"invalid mutation adjudication family ID: {family_id!r}")
        if family_id in seen_ids:
            fail(f"duplicate mutation adjudication family ID: {family_id}")
        seen_ids.add(family_id)
        for field in required_text:
            if not isinstance(family.get(field), str) or not family[field].strip():
                fail(f"mutation adjudication family {family_id} has no {field}")
        if family.get("production_refinement") not in {"pending", "implemented"}:
            fail(f"mutation adjudication family {family_id} has invalid production_refinement")

        family_behaviors = string_list(family.get("behavior_ids"), f"{family_id}.behavior_ids")
        unknown = set(family_behaviors).difference(behavior_ids)
        if unknown:
            fail(f"mutation adjudication family {family_id} has unknown behaviors: {sorted(unknown)}")
        primary = family.get("primary_behavior_id")
        if primary not in family_behaviors:
            fail(f"mutation adjudication family {family_id} primary behavior is not in behavior_ids")
        invariants = string_list(family.get("invariants"), f"{family_id}.invariants")
        unknown = set(invariants).difference(invariant_ids)
        if unknown:
            fail(f"mutation adjudication family {family_id} has unknown invariants: {sorted(unknown)}")
        unknown = set(string_list(family.get("model_roles"), f"{family_id}.model_roles")).difference(model_roles)
        if unknown:
            fail(f"mutation adjudication family {family_id} has unknown model roles: {sorted(unknown)}")
        unknown = set(string_list(family.get("production_roles"), f"{family_id}.production_roles")).difference(production_roles)
        if unknown:
            fail(f"mutation adjudication family {family_id} has unknown production roles: {sorted(unknown)}")

        evidence_patterns = compile_patterns(family.get("evidence_test_patterns"), f"{family_id}.evidence_test_patterns")
        generated_patterns = compile_patterns(
            family.get("generated_test_patterns"),
            f"{family_id}.generated_test_patterns",
            allow_empty=True,
        )
        evidence_tests = matched_tests(evidence_patterns, tests, f"{family_id}.evidence_test_patterns")
        generated_tests = (
            matched_tests(generated_patterns, tests, f"{family_id}.generated_test_patterns")
            if generated_patterns
            else set()
        )
        if not generated_tests.issubset(evidence_tests):
            fail(f"mutation adjudication family {family_id} generated tests are outside its evidence")
        for test in evidence_tests:
            entry = registered.get(test)
            if entry is None:
                fail(f"mutation adjudication evidence is absent from review registry: {test}")
            if entry.get("behavior_id") not in family_behaviors:
                fail(f"mutation adjudication evidence {test} has the wrong behavior owner")
        for test in generated_tests:
            owner = generated_owner.get(test)
            if owner is not None:
                fail(f"generated model test belongs to two proof families: {test}")
            generated_owner[test] = {
                "test": test,
                "behavior_id": primary,
                "invariants": invariants,
            }
            if registered[test] != generated_owner[test]:
                fail(f"generated model evidence is stale; run --write-evidence: {test}")

        selectors = family.get("mutation_selectors")
        if not isinstance(selectors, list) or not selectors:
            fail(f"mutation adjudication family {family_id} has no mutation_selectors")
        for selector in selectors:
            if not isinstance(selector, dict) or set(selector) != {"obligation_id", "function_pattern"}:
                fail(f"invalid mutation selector in {family_id}: {selector!r}")
            if not isinstance(selector.get("obligation_id"), str):
                fail(f"mutation selector in {family_id} has no obligation_id")
            compile_patterns([selector.get("function_pattern")], f"{family_id}.function_pattern")
        family["_evidence_tests"] = evidence_tests
    return families, generated_owner


def synchronize_generated_evidence(registry: dict, generated: dict[str, dict]) -> str:
    source = REGISTRY.read_text()
    try:
        prefix, remainder = source.split(UNIT_START, 1)
        body, suffix = remainder.split(UNIT_END, 1)
    except ValueError:
        fail("cannot locate unit_evidence section in review behavior registry")
    generated_tests = set(generated)
    kept: list[str] = []
    for line in body.splitlines():
        stripped = line.strip().removesuffix(",")
        if stripped.startswith("{"):
            try:
                entry = json.loads(stripped)
            except json.JSONDecodeError:
                entry = None
            if isinstance(entry, dict) and entry.get("test") in generated_tests:
                continue
        kept.append(line)
    while kept and not kept[-1].strip():
        kept.pop()
    for index in range(len(kept) - 1, -1, -1):
        if kept[index].strip().startswith("{"):
            kept[index] = kept[index].rstrip().removesuffix(",") + ","
            break
    if generated:
        kept.append("")
        rows = [generated[test] for test in sorted(generated)]
        for index, row in enumerate(rows):
            rendered = json.dumps(row, separators=(", ", ": "))
            comma = "," if index + 1 < len(rows) else ""
            kept.append(f"    {rendered}{comma}")
    return prefix + UNIT_START + "\n" + "\n".join(kept) + UNIT_END + suffix


def assign_mutation_rows(contract: dict, families: list[dict]) -> tuple[dict[str, int], str]:
    adjudication = contract["mutation_adjudication"]
    candidate_path = repo_path(adjudication.get("candidate_lock"), "mutation candidate lock")
    result_path = repo_path(adjudication.get("result_lock"), "mutation result lock")
    candidate_lock = load_json(candidate_path, "mutation candidate lock")
    result_lock = load_json(result_path, "mutation result lock")
    candidates = candidate_lock.get("rows")
    results = result_lock.get("rows")
    if not isinstance(candidates, list) or not isinstance(results, list):
        fail("mutation locks must contain row lists")
    candidate_by_name = {
        row.get("candidate"): row
        for row in candidates
        if isinstance(row, dict) and isinstance(row.get("candidate"), str)
    }
    if len(candidate_by_name) != len(candidates):
        fail("mutation candidate lock contains invalid or duplicate rows")
    unacceptable = set(adjudication["unacceptable_outcomes"])
    rows: list[dict] = []
    for result in results:
        if not isinstance(result, dict) or result.get("summary") not in unacceptable:
            continue
        name = result.get("candidate")
        candidate = candidate_by_name.get(name)
        if candidate is None:
            fail(f"mutation result row is absent from candidate lock: {name!r}")
        rows.append({**candidate, "outcome": result["summary"]})
    if not rows:
        fail("mutation result contains no rows requiring adjudication")

    counts = {family["id"]: 0 for family in families}
    digest_rows: list[str] = []
    selector_hits: dict[tuple[str, int], int] = {}
    for row in rows:
        matches: list[str] = []
        for family in families:
            for index, selector in enumerate(family["mutation_selectors"]):
                if row.get("obligation_id") != selector["obligation_id"]:
                    continue
                if re.fullmatch(selector["function_pattern"], row.get("function", "")):
                    matches.append(family["id"])
                    selector_hits[(family["id"], index)] = selector_hits.get((family["id"], index), 0) + 1
        if len(matches) != 1:
            fail(
                "mutation adjudication must assign each unacceptable row exactly once: "
                f"candidate={row.get('candidate')!r}, families={matches}"
            )
        family_id = matches[0]
        counts[family_id] += 1
        digest_rows.append(f"{row['candidate']}\t{row['outcome']}\t{family_id}")
    for family in families:
        for index, _selector in enumerate(family["mutation_selectors"]):
            if selector_hits.get((family["id"], index), 0) == 0:
                fail(f"mutation selector matches zero unacceptable rows: {family['id']}[{index}]")
    digest = hashlib.sha256("\n".join(sorted(digest_rows)).encode()).hexdigest()
    return counts, digest


def main() -> int:
    args = parse_args()
    contract = load_json(CONTRACT, "architecture contract")
    registry = load_json(REGISTRY, "review behavior registry")
    tests = unit_tests()

    if args.write_evidence:
        # Derive rows without requiring the not-yet-synchronized registry.
        adjudication = contract.get("mutation_adjudication")
        if not isinstance(adjudication, dict):
            fail("architecture contract has no mutation_adjudication")
        generated: dict[str, dict] = {}
        for family in adjudication.get("families", []):
            if not isinstance(family, dict):
                continue
            patterns = compile_patterns(
                family.get("generated_test_patterns"),
                f"{family.get('id')}.generated_test_patterns",
                allow_empty=True,
            )
            selected = matched_tests(patterns, tests, str(family.get("id"))) if patterns else set()
            for test in selected:
                if test in generated:
                    fail(f"generated model test belongs to two proof families: {test}")
                generated[test] = {
                    "test": test,
                    "behavior_id": family.get("primary_behavior_id"),
                    "invariants": family.get("invariants"),
                }
        rendered = synchronize_generated_evidence(registry, generated)
        if rendered != REGISTRY.read_text():
            REGISTRY.write_text(rendered)
        registry = load_json(REGISTRY, "review behavior registry")

    families, _generated = family_inputs(contract, registry, tests)
    counts, digest = assign_mutation_rows(contract, families)
    evidence_count = len(set().union(*(family["_evidence_tests"] for family in families)))
    summary = ", ".join(f"{family['id']}={counts[family['id']]}" for family in families)
    print(
        "validated mutation root adjudication: "
        f"families={len(families)}, evidence_tests={evidence_count}, "
        f"coverage_sha256={digest}; {summary}"
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except SystemExit as error:
        if isinstance(error.code, str):
            print(f"error: {error.code}", file=sys.stderr)
            raise SystemExit(1)
        raise
