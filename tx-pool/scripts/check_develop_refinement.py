#!/usr/bin/env python3
"""Verify the immutable develop call graphs behind the negative model witnesses."""

from __future__ import annotations

import json
from pathlib import Path
import re
import subprocess
import sys

from check_production_contracts import mask_rust_non_code, matching_brace


REPO_ROOT = Path(__file__).resolve().parents[2]
CONTRACT = REPO_ROOT / "tx-pool" / "architecture-contract.json"
REGISTRY = REPO_ROOT / "tx-pool" / "review-behaviors.json"
EXPECTED_CLASSIFICATIONS = {
    "single_authority_required",
    "local_correction_sufficient",
    "intentional_compatibility",
}
EXPECTED_FAMILIES = {f"F{number}" for number in range(1, 9)}


def git_text(*arguments: str) -> tuple[str | None, str | None]:
    result = subprocess.run(
        ["git", *arguments],
        cwd=REPO_ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        return None, result.stderr.strip() or result.stdout.strip()
    return result.stdout, None


def function_body(source: str, symbol: str) -> tuple[str | None, str | None]:
    masked = mask_rust_non_code(source)
    matches = list(re.finditer(rf"\bfn\s+{re.escape(symbol)}\b", masked))
    if len(matches) != 1:
        return None, f"function {symbol!r} matched {len(matches)} declarations"
    opening = masked.find("{", matches[0].end())
    if opening < 0:
        return None, f"function {symbol!r} has no body"
    closing = matching_brace(masked, opening)
    if closing is None:
        return None, f"function {symbol!r} has an unmatched body"
    return masked[opening + 1 : closing], None


def validate_parser_canary() -> list[str]:
    source = """
        fn ordered_canary() {
            first();
            // forbidden_comment_only();
            second();
        }
    """
    body, error = function_body(source, "ordered_canary")
    if error is not None or body is None:
        return [f"develop refinement parser canary failed: {error}"]
    if "first()" not in body or "second()" not in body:
        return ["develop refinement parser canary lost executable calls"]
    if "forbidden_comment_only" in body:
        return ["develop refinement parser canary treated a comment as code"]
    if body.find("first()") >= body.find("second()"):
        return ["develop refinement parser canary lost source order"]
    return []


def validate_source_fact(
    revision: str, case_id: str, fact: object
) -> list[str]:
    if not isinstance(fact, dict):
        return [f"develop case {case_id} source fact must be an object"]
    path = fact.get("path")
    symbol = fact.get("symbol")
    required = fact.get("required", [])
    ordered = fact.get("ordered", [])
    forbidden = fact.get("forbidden", [])
    errors: list[str] = []
    if not isinstance(path, str) or not path.startswith("tx-pool/src/"):
        return [f"develop case {case_id} has invalid source path {path!r}"]
    if symbol is not None and not isinstance(symbol, str):
        errors.append(f"develop case {case_id} source symbol must be a string")
    for field, values in (
        ("required", required),
        ("ordered", ordered),
        ("forbidden", forbidden),
    ):
        if not isinstance(values, list) or not all(
            isinstance(value, str) and value for value in values
        ):
            errors.append(
                f"develop case {case_id} source {field} must be a string list"
            )
    if errors:
        return errors
    if not required and not ordered and not forbidden:
        return [f"develop case {case_id} source fact has no falsifiable predicate"]
    source, git_error = git_text("show", f"{revision}:{path}")
    if source is None:
        return [
            f"develop case {case_id} cannot read {revision}:{path}: {git_error}; "
            "the immutable baseline history is required"
        ]
    subject = mask_rust_non_code(source)
    if symbol is not None:
        subject, parse_error = function_body(source, symbol)
        if subject is None:
            return [f"develop case {case_id} {path}: {parse_error}"]
    for value in required:
        if value not in subject:
            errors.append(
                f"develop case {case_id} {path}::{symbol or '<file>'} "
                f"is missing required source fact {value!r}"
            )
    cursor = 0
    for value in ordered:
        position = subject.find(value, cursor)
        if position < 0:
            errors.append(
                f"develop case {case_id} {path}::{symbol or '<file>'} "
                f"does not contain ordered source fact {value!r} after offset {cursor}"
            )
            break
        cursor = position + len(value)
    for value in forbidden:
        if value in subject:
            errors.append(
                f"develop case {case_id} {path}::{symbol or '<file>'} "
                f"contains forbidden source fact {value!r}"
            )
    return errors


def validate() -> tuple[int, int, list[str]]:
    errors: list[str] = []
    try:
        contract = json.loads(CONTRACT.read_text())
        registry = json.loads(REGISTRY.read_text())
    except (OSError, json.JSONDecodeError) as error:
        return 0, 0, [f"cannot load develop refinement inputs: {error}"]

    refinement = contract.get("develop_refinement")
    if not isinstance(refinement, dict) or refinement.get("schema_version") != 1:
        return 0, 0, ["architecture contract develop_refinement schema must be 1"]
    baseline = refinement.get("baseline")
    if not isinstance(baseline, dict):
        return 0, 0, ["develop refinement baseline must be an object"]
    revision = baseline.get("revision")
    expected_tree = baseline.get("tree")
    if not isinstance(revision, str) or not re.fullmatch(r"[0-9a-f]{40}", revision):
        errors.append("develop refinement revision must be one full commit hash")
    if not isinstance(expected_tree, str) or not re.fullmatch(
        r"[0-9a-f]{40}", expected_tree
    ):
        errors.append("develop refinement tree must be one full tree hash")
    if errors:
        return 0, 0, errors
    actual_revision, revision_error = git_text("rev-parse", f"{revision}^{{commit}}")
    if actual_revision is None:
        errors.append(
            f"immutable develop baseline {revision} is unavailable: {revision_error}; "
            "fetch complete Git history"
        )
    elif actual_revision.strip() != revision:
        errors.append(
            f"develop baseline resolved to {actual_revision.strip()}, expected {revision}"
        )
    actual_tree, tree_error = git_text("rev-parse", f"{revision}^{{tree}}")
    if actual_tree is None:
        errors.append(f"cannot resolve develop baseline tree: {tree_error}")
    elif actual_tree.strip() != expected_tree:
        errors.append(
            f"develop baseline tree is {actual_tree.strip()}, expected {expected_tree}"
        )

    if not isinstance(registry, dict):
        return 0, 0, ["develop refinement registry must be an object"]
    behavior_ids = {
        behavior.get("id")
        for behavior in registry.get("behaviors", [])
        if isinstance(behavior, dict) and isinstance(behavior.get("id"), str)
    }
    evidence_entries = [
        evidence
        for evidence in registry.get("unit_evidence", [])
        if isinstance(evidence, dict)
        and isinstance(evidence.get("test"), str)
        and isinstance(evidence.get("behavior_id"), str)
    ]
    evidence_names = [evidence["test"] for evidence in evidence_entries]
    duplicate_evidence = sorted(
        name for name in set(evidence_names) if evidence_names.count(name) != 1
    )
    if duplicate_evidence:
        errors.append(f"unit evidence repeats tests: {duplicate_evidence}")
    unit_evidence = {
        evidence["test"]: evidence
        for evidence in evidence_entries
    }
    target_invariants = contract.get("target_invariants", {})
    if not isinstance(target_invariants, dict):
        errors.append("architecture target_invariants must be an object")
        target_invariants = {}
    invariant_by_observation = {
        observation: invariant
        for invariant, observation in target_invariants.items()
        if isinstance(invariant, str) and isinstance(observation, str)
    }
    if len(invariant_by_observation) != len(target_invariants):
        errors.append(
            "architecture target_invariants must map unique string observations"
        )

    cases = refinement.get("cases")
    if not isinstance(cases, list) or not cases:
        return 0, 0, [*errors, "develop refinement cases must be a non-empty list"]
    seen_ids: set[str] = set()
    covered_families: set[str] = set()
    classifications: set[str] = set()
    for case in cases:
        if not isinstance(case, dict):
            errors.append("develop refinement case must be an object")
            continue
        case_id = case.get("id")
        if not isinstance(case_id, str) or not case_id:
            errors.append("develop refinement case id must be a non-empty string")
            continue
        if case_id in seen_ids:
            errors.append(f"duplicate develop refinement case {case_id}")
        seen_ids.add(case_id)
        families = case.get("families")
        if not isinstance(families, list) or not families or not all(
            isinstance(family, str) for family in families
        ):
            errors.append(f"develop case {case_id} must name one or more families")
            families = []
        unknown_families = set(families).difference(EXPECTED_FAMILIES)
        if unknown_families:
            errors.append(
                f"develop case {case_id} names unknown families {sorted(unknown_families)}"
            )
        covered_families.update(families)
        classification = case.get("classification")
        if classification not in EXPECTED_CLASSIFICATIONS:
            errors.append(
                f"develop case {case_id} has invalid classification {classification!r}"
            )
        else:
            classifications.add(classification)
        observations = case.get("observations")
        if not isinstance(observations, list) or not observations or not all(
            isinstance(observation, str) and observation
            for observation in observations
        ):
            errors.append(f"develop case {case_id} must name semantic observations")
            observations = []
        unknown_observations = set(observations).difference(invariant_by_observation)
        if unknown_observations:
            errors.append(
                f"develop case {case_id} names unknown observations "
                f"{sorted(unknown_observations)}"
            )
        current_behaviors = case.get("current_behaviors")
        if (
            not isinstance(current_behaviors, list)
            or not current_behaviors
            or not all(
                isinstance(behavior, str) and behavior
                for behavior in current_behaviors
            )
        ):
            errors.append(f"develop case {case_id} must name current behaviors")
            current_behaviors = []
        unknown_behaviors = set(current_behaviors).difference(behavior_ids)
        if unknown_behaviors:
            errors.append(
                f"develop case {case_id} names unknown behaviors {sorted(unknown_behaviors)}"
            )
        expected_invariants = {
            invariant_by_observation[observation]
            for observation in observations
            if observation in invariant_by_observation
        }
        counterexample = case.get("counterexample_test")
        if not isinstance(counterexample, str) or not counterexample:
            errors.append(
                f"develop case {case_id} must name one counterexample test"
            )
            registered_evidence = None
        else:
            registered_evidence = unit_evidence.get(counterexample)
        if (
            isinstance(counterexample, str)
            and counterexample
            and registered_evidence is None
        ):
            errors.append(
                f"develop case {case_id} counterexample {counterexample!r} is not registered"
            )
        else:
            registered_behavior = registered_evidence.get("behavior_id")
            if registered_behavior not in current_behaviors:
                errors.append(
                    f"develop case {case_id} counterexample is registered to "
                    f"{registered_behavior}, outside current_behaviors"
                )
            registered_invariants = set(registered_evidence.get("invariants", []))
            if registered_invariants != expected_invariants:
                errors.append(
                    f"develop case {case_id} observations map to "
                    f"{sorted(expected_invariants)}, but its counterexample registers "
                    f"{sorted(registered_invariants)}"
                )
        theorem_tests = case.get("current_theorem_tests")
        if not isinstance(theorem_tests, list) or not theorem_tests or not all(
            isinstance(test, str) and test for test in theorem_tests
        ):
            errors.append(
                f"develop case {case_id} must name current theorem tests"
            )
            theorem_tests = []
        if len(theorem_tests) != len(set(theorem_tests)):
            errors.append(f"develop case {case_id} repeats a current theorem test")
        theorem_invariants: set[str] = set()
        for test in theorem_tests:
            theorem_evidence = unit_evidence.get(test)
            if theorem_evidence is None:
                errors.append(
                    f"develop case {case_id} current theorem {test!r} is not registered"
                )
                continue
            theorem_behavior = theorem_evidence.get("behavior_id")
            if theorem_behavior not in current_behaviors:
                errors.append(
                    f"develop case {case_id} current theorem {test!r} belongs to "
                    f"{theorem_behavior}, outside current_behaviors"
                )
            theorem_invariants.update(theorem_evidence.get("invariants", []))
        missing_theorem_invariants = expected_invariants.difference(theorem_invariants)
        if missing_theorem_invariants:
            errors.append(
                f"develop case {case_id} current theorems do not cover "
                f"{sorted(missing_theorem_invariants)}"
            )
        facts = case.get("source_facts")
        if not isinstance(facts, list) or not facts:
            errors.append(f"develop case {case_id} must name source facts")
        else:
            for fact in facts:
                errors.extend(validate_source_fact(revision, case_id, fact))

    missing_families = EXPECTED_FAMILIES.difference(covered_families)
    if missing_families:
        errors.append(
            f"develop refinement does not cover families {sorted(missing_families)}"
        )
    if "single_authority_required" not in classifications:
        errors.append("develop refinement has no single-authority necessity witness")
    if "local_correction_sufficient" not in classifications:
        errors.append("develop refinement has no local-correction adjudication")
    return len(cases), len(covered_families), errors


def main() -> int:
    canary_errors = validate_parser_canary()
    if canary_errors:
        for error in canary_errors:
            print(f"error: {error}", file=sys.stderr)
        return 1
    cases, families, errors = validate()
    if errors:
        for error in errors:
            print(f"error: {error}", file=sys.stderr)
        return 1
    print(
        f"validated {cases} immutable develop cases across {families} root families"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
