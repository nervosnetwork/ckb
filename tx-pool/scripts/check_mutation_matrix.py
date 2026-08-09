#!/usr/bin/env python3
"""Validate, rediscover and reconcile the V1 mutation evidence universe."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import shlex
import subprocess
import sys

from check_mutation_adjudication import (
    equivalence_proof_index,
    run_equivalence_canaries,
)
from check_review_guide import load_registry, target_invariant_ids
from check_security_manifest import (
    DEFAULT_MANIFEST,
    REPO_ROOT,
    inventory_path,
    load_manifest,
    load_repo_json,
    read_test_inventory,
    registry_path,
    resolve_mutation_owner,
    validate_mutation_acceptance,
)


DEFAULT_LOCK = REPO_ROOT / "tx-pool" / "mutation-acceptance-lock.json"
DEFAULT_RESULT_LOCK = REPO_ROOT / "tx-pool" / "mutation-result-lock.json"
GENERATOR_PATH = "tx-pool/scripts/check_mutation_matrix.py"
MUTANT_OUTCOME_SUMMARIES = {
    "CaughtMutant",
    "MissedMutant",
    "Timeout",
    "Unviable",
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument("--lock", type=Path, default=DEFAULT_LOCK)
    parser.add_argument("--result-lock", type=Path, default=DEFAULT_RESULT_LOCK)
    parser.add_argument(
        "--rediscover",
        action="store_true",
        help="run cargo-mutants discovery instead of validating checked-in locks",
    )
    parser.add_argument(
        "--write-lock",
        action="store_true",
        help="write the deterministic row-level lock after semantic review",
    )
    parser.add_argument(
        "--write-config",
        type=Path,
        help="write an exact cargo-mutants config for the locked rows",
    )
    parser.add_argument(
        "--resume-outcomes",
        type=Path,
        action="append",
        default=[],
        help="exclude exact rows already present in a prior outcome file",
    )
    parser.add_argument(
        "--verify-outcomes",
        type=Path,
        action="append",
        default=[],
        help="merge and reconcile one cargo-mutants outcome set",
    )
    parser.add_argument(
        "--write-result-lock",
        action="store_true",
        help="write the portable complete result projection",
    )
    parser.add_argument(
        "--require-accepted",
        action="store_true",
        help="fail unless every result has a machine-accepted disposition",
    )
    parser.add_argument("--print-json", action="store_true")
    return parser.parse_args()


def fail(message: str) -> None:
    raise SystemExit(f"error: {message}")


def canonical_json(value: object) -> bytes:
    return json.dumps(
        value, ensure_ascii=True, sort_keys=True, separators=(",", ":")
    ).encode()


def digest(value: object) -> str:
    return hashlib.sha256(canonical_json(value)).hexdigest()


def file_digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def repo_relative(path: Path) -> str:
    try:
        return path.resolve().relative_to(REPO_ROOT).as_posix()
    except ValueError as error:
        fail(f"path must stay inside the repository: {path}: {error}")


def run(argv: list[str]) -> str:
    completed = subprocess.run(
        argv,
        cwd=REPO_ROOT,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if completed.returncode:
        fail(
            f"command failed ({completed.returncode}): {shlex.join(argv)}\n"
            f"{completed.stderr.strip()}"
        )
    return completed.stdout


def require_clean_worktree(allowed_generated: set[str] | None = None) -> None:
    status = run(["git", "status", "--porcelain=v1", "--untracked-files=all"])
    allowed_generated = allowed_generated or set()
    unexpected = []
    for line in status.splitlines():
        path = line[3:]
        if " -> " in path:
            path = path.rsplit(" -> ", 1)[1]
        if path not in allowed_generated:
            unexpected.append(line)
    if unexpected:
        fail(
            "--write-lock requires a clean recoverable Git checkpoint; "
            f"current changes:\n{chr(10).join(unexpected)}"
        )


def cargo_mutants_version() -> str:
    version = run(["cargo", "mutants", "--version"]).strip()
    if not re.fullmatch(r"cargo-mutants \d+\.\d+\.\d+", version):
        fail(f"unexpected cargo-mutants version string: {version!r}")
    return version


def bare_symbol(symbol: str) -> str:
    for prefix in ("struct ", "enum ", "fn "):
        if symbol.startswith(prefix):
            return symbol.removeprefix(prefix)
    return symbol


def load_inputs(manifest_path: Path) -> tuple[dict, dict, dict, dict]:
    manifest = load_manifest(manifest_path)
    contract, contract_errors = load_repo_json(
        manifest.get("architecture_contract"), "architecture_contract"
    )
    if contract_errors or contract is None:
        fail("; ".join(contract_errors))
    registry = load_registry(registry_path(manifest))
    acceptance = manifest.get("mutation_acceptance")
    acceptance_errors = validate_mutation_acceptance(acceptance, contract, registry)
    if acceptance_errors:
        fail("; ".join(acceptance_errors))
    return manifest, contract, registry, acceptance


def resolve_obligations(
    acceptance: dict, contract: dict, registry: dict
) -> list[dict]:
    resolved: list[dict] = []
    for obligation in acceptance["obligations"]:
        owner, errors = resolve_mutation_owner(
            obligation["owner_ref"], contract, registry
        )
        if errors or owner is None:
            fail("; ".join(errors))
        resolved.append({**obligation, "owner": owner})
    return resolved


def list_candidates(
    package: str,
    features: list[str],
    paths: list[str],
    config: Path | None = None,
) -> list[dict]:
    argv = ["cargo", "mutants"]
    if config is not None:
        argv.extend(["--config", str(config)])
    argv.extend(["-p", package])
    if features:
        argv.extend(["--features", ",".join(features)])
    argv.extend(["--list", "--json"])
    for path in paths:
        argv.extend(["-f", path])
    try:
        value = json.loads(run(argv))
    except json.JSONDecodeError as error:
        fail(f"cargo-mutants emitted invalid JSON: {error}")
    if not isinstance(value, list):
        fail("cargo-mutants candidate output must be a JSON array")
    names: set[str] = set()
    for candidate in value:
        if not isinstance(candidate, dict):
            fail(f"invalid cargo-mutants candidate: {candidate!r}")
        name = candidate.get("name")
        function = candidate.get("function")
        replacement = candidate.get("replacement")
        if (
            not isinstance(name, str)
            or not isinstance(function, dict)
            or not isinstance(replacement, str)
        ):
            fail(f"candidate lacks exact name/function attribution: {candidate!r}")
        function_name = function.get("function_name")
        if not isinstance(function_name, str) or not function_name:
            fail(f"candidate lacks exact function name: {name}")
        if name in names:
            fail(f"duplicate cargo-mutants candidate name: {name}")
        names.add(name)
    return value


def obligation_matches(candidate: dict, obligation: dict) -> bool:
    function_name = candidate["function"]["function_name"]
    symbol = bare_symbol(obligation["owner"]["symbol"])
    selector = obligation["selector"]
    if selector["kind"] == "all_methods":
        return function_name.startswith(f"{symbol}::")
    if selector["kind"] == "method":
        return function_name == f"{symbol}::{selector['name']}"
    if selector["kind"] == "function":
        return function_name == symbol
    if selector["kind"] == "struct_fields":
        return (
            function_name.startswith(f"{symbol}::")
            and candidate.get("genre") == "StructField"
        )
    if selector["kind"] == "remaining_path":
        return False
    fail(f"unhandled mutation selector: {selector!r}")


def select_candidates(candidates: list[dict], obligations: list[dict]) -> list[tuple[dict, dict]]:
    selected: list[tuple[dict, dict]] = []
    matched_obligations: set[str] = set()
    primary = [
        obligation
        for obligation in obligations
        if obligation["selector"]["kind"] != "remaining_path"
    ]
    remainders: dict[str, dict] = {}
    for obligation in obligations:
        if obligation["selector"]["kind"] != "remaining_path":
            continue
        path = obligation["owner"]["path"]
        if path in remainders:
            fail(
                f"duplicate remaining-path mutation obligations for {path}: "
                f"{remainders[path]['id']!r}, {obligation['id']!r}"
            )
        remainders[path] = obligation
    for candidate in candidates:
        matches = [
            obligation
            for obligation in primary
            if candidate["file"] == obligation["owner"]["path"]
            and obligation_matches(candidate, obligation)
        ]
        if len(matches) > 1:
            fail(
                f"ambiguous mutation row {candidate['name']!r}: "
                f"{[entry['id'] for entry in matches]}"
            )
        if not matches and candidate["file"] in remainders:
            matches = [remainders[candidate["file"]]]
        if matches:
            selected.append((candidate, matches[0]))
            matched_obligations.add(matches[0]["id"])
    zero_match = sorted(
        obligation["id"]
        for obligation in obligations
        if obligation["id"] not in matched_obligations
    )
    if zero_match:
        fail(f"zero-match mutation obligations: {zero_match}")
    selected_names = {row[0]["name"] for row in selected}
    unowned = sorted(
        candidate["name"]
        for candidate in candidates
        if candidate["name"] not in selected_names
    )
    if unowned:
        fail(f"complete mutation universe contains unowned rows: {unowned}")
    return sorted(selected, key=lambda entry: entry[0]["name"])


def run_selection_canaries() -> None:
    candidate = {
        "name": "canary.rs:1:1: replace Owner::method",
        "file": "canary.rs",
        "function": {"function_name": "Owner::method"},
    }
    owner = {"path": "canary.rs", "symbol": "struct Owner"}
    obligation = {
        "id": "V1-MUT-CANARY",
        "owner": owner,
        "selector": {"kind": "all_methods"},
    }
    if len(select_candidates([candidate], [obligation])) != 1:
        fail("mutation selector positive canary did not select one row")
    try:
        select_candidates([candidate], [obligation, {**obligation, "id": "V1-MUT-DUP"}])
    except SystemExit as error:
        if "ambiguous mutation row" not in str(error):
            raise
    else:
        fail("mutation selector ambiguity canary did not fail")
    try:
        select_candidates([], [obligation])
    except SystemExit as error:
        if "zero-match mutation obligations" not in str(error):
            raise
    else:
        fail("mutation selector zero-match canary did not fail")
    remainder = {
        "id": "V1-MUT-CANARY-REMAINDER",
        "owner": owner,
        "selector": {"kind": "remaining_path"},
    }
    if select_candidates([candidate], [remainder])[0][1]["id"] != remainder["id"]:
        fail("mutation selector remaining-path canary did not own the unmatched row")
    other = {
        **candidate,
        "name": "canary.rs:2:1: replace Owner::other",
        "function": {"function_name": "Owner::other"},
    }
    exact = {**obligation, "selector": {"kind": "method", "name": "method"}}
    selected = select_candidates([candidate, other], [exact, remainder])
    selected_by_name = {row[0]["name"]: row[1]["id"] for row in selected}
    if selected_by_name[candidate["name"]] != exact["id"]:
        fail("mutation selector primary owner did not precede the path remainder")


def config_text(
    selected: list[tuple[dict, dict]], candidates: list[dict]
) -> str:
    selected_names = {candidate["name"] for candidate, _ in selected}
    lines = [
        "# Generated by tx-pool/scripts/check_mutation_matrix.py.",
        "# Do not edit candidate expressions by hand.",
        "examine_re = [",
    ]
    lines.extend(
        f"  {json.dumps('^' + re.escape(candidate['name']) + '$')},"
        for candidate, _ in selected
    )
    lines.extend(["]", "exclude_re = ["])
    lines.extend(
        f"  {json.dumps('^' + re.escape(candidate['name']) + '$')},"
        for candidate in sorted(candidates, key=lambda entry: entry["name"])
        if candidate["name"] not in selected_names
    )
    lines.extend(["]", ""])
    return "\n".join(lines)


def verify_exact_config(
    config: Path,
    selected: list[tuple[dict, dict]],
    package: str,
    features: list[str],
    paths: list[str],
    permitted_replays: set[str] | None = None,
) -> list[dict]:
    relisted = list_candidates(package, features, paths, config)
    expected = [candidate["name"] for candidate, _ in selected]
    observed = sorted(candidate["name"] for candidate in relisted)
    missing = sorted(set(expected).difference(observed))
    replayed = sorted(set(observed).difference(expected))
    forbidden_replays = sorted(
        set(replayed).difference(permitted_replays or set())
    )
    if missing or forbidden_replays:
        fail(
            "exact cargo-mutants config did not close: "
            f"missing={missing}, forbidden_replays={forbidden_replays}"
        )
    return sorted(relisted, key=lambda candidate: candidate["name"])


def read_unit_universe(path: Path) -> list[str]:
    sections, errors = read_test_inventory(path)
    if errors:
        fail("; ".join(errors))
    tests = sections.get("unit", [])
    if not tests or tests != sorted(set(tests)):
        fail("unit test inventory must be non-empty, sorted and unique")
    return tests


def component_index(contract: dict) -> dict[str, dict]:
    return {
        component["id"]: component
        for component in contract["selected_topology"]["components"]
    }


def evidence_set(
    obligation: dict,
    contract: dict,
    registry: dict,
    unit_tests: set[str],
) -> dict:
    binding = contract["refinement_inventory"]["semantic_bindings"][
        obligation["semantic_binding"]
    ]
    component = component_index(contract).get(obligation.get("component_id"))
    owner_ref = obligation["owner_ref"]
    primary_behavior_ids: set[str] = set()
    if owner_ref["kind"] == "behavior_owner":
        primary_behavior_ids.add(owner_ref["behavior_id"])
    component_binding_behaviors: set[str] = set()
    if component is not None:
        component_binding_behaviors = set(component["behavior_ids"]).intersection(
            binding["behavior_ids"]
        )
    if not primary_behavior_ids:
        primary_behavior_ids.update(component_binding_behaviors)
    behavior_ids = primary_behavior_ids.union(component_binding_behaviors)

    unit_evidence = [
        entry
        for entry in registry["unit_evidence"]
        if entry["behavior_id"] in primary_behavior_ids
    ]
    falsifiers = {entry["test"] for entry in unit_evidence}
    invariants = {
        invariant
        for entry in unit_evidence
        for invariant in entry["invariants"]
    }
    if component is not None:
        falsifiers.update(component["falsifier_tests"])
        invariants.update(component["invariants"])
    missing_tests = sorted(falsifiers.difference(unit_tests))
    if missing_tests:
        fail(
            f"mutation obligation {obligation['id']} has falsifiers outside the "
            f"complete library universe: {missing_tests}"
        )
    unknown_invariants = invariants.difference(target_invariant_ids(contract))
    if unknown_invariants:
        fail(
            f"mutation obligation {obligation['id']} has unknown invariants: "
            f"{sorted(unknown_invariants)}"
        )
    if not falsifiers or not invariants:
        fail(f"mutation obligation {obligation['id']} lacks falsifiable evidence")
    return {
        "semantic_binding": obligation["semantic_binding"],
        "component_id": obligation.get("component_id"),
        "behavior_ids": sorted(behavior_ids),
        "invariants": sorted(invariants),
        "production_owner": obligation["owner"],
        "legal_input_falsifier_count": len(falsifiers),
        "legal_input_falsifier_sha256": digest(sorted(falsifiers)),
        "inclusion_reason": (
            f"{obligation['semantic_binding']} -> {obligation['id']} -> "
            f"{obligation['owner']['path']}::{obligation['owner']['symbol']}"
        ),
    }


def input_record(path: Path) -> dict:
    return {"path": repo_relative(path), "sha256": file_digest(path)}


def production_source_revision(paths: list[str]) -> str:
    revision = run(["git", "log", "-1", "--format=%H", "--", *paths]).strip()
    if not re.fullmatch(r"[0-9a-f]{40}", revision):
        fail(f"cannot derive production source revision for {paths}")
    return revision


def command_template(
    package: str, features: list[str], paths: list[str], target: str
) -> list[str]:
    argv = ["cargo", "mutants", "--config", "<CONFIG>", "-p", package]
    if features:
        argv.extend(["--features", ",".join(features)])
    argv.extend(
        [
            "--test-tool",
            "nextest",
            "--baseline",
            "run",
            "-j",
            "1",
            "--minimum-test-timeout",
            "120",
            "--timeout-multiplier",
            "3",
            "--build-timeout-multiplier",
            "3",
            "--no-shuffle",
        ]
    )
    for path in paths:
        argv.extend(["-f", path])
    argv.extend(["-o", "<OUTPUT>", "--", f"--{target}"])
    return argv


def build_lock(
    manifest_path: Path,
    manifest: dict,
    contract: dict,
    registry: dict,
    acceptance: dict,
    selected: list[tuple[dict, dict]],
    version: str,
    exact_config: str,
) -> dict:
    paths = sorted({obligation["owner"]["path"] for _, obligation in selected})
    inventory = inventory_path(manifest)
    unit_tests = read_unit_universe(inventory)
    universe_id = (
        f"{manifest['package']}:{','.join(manifest['features'])}:"
        f"{acceptance['test_target']}"
    )
    evidence_sets = {
        obligation["id"]: evidence_set(
            obligation, contract, registry, set(unit_tests)
        )
        for obligation in resolve_obligations(acceptance, contract, registry)
    }
    rows = []
    for candidate, obligation in selected:
        rows.append(
            {
                "candidate": candidate["name"],
                "file": candidate["file"],
                "function": candidate["function"]["function_name"],
                "genre": candidate.get("genre"),
                "replacement": candidate["replacement"],
                "obligation_id": obligation["id"],
            }
        )
    command = command_template(
        manifest["package"], manifest["features"], paths, acceptance["test_target"]
    )
    contract_path = REPO_ROOT / manifest["architecture_contract"]
    behavior_path = registry_path(manifest)
    return {
        "schema_version": 3,
        "generator": GENERATOR_PATH,
        "cargo_mutants_version": version,
        "inputs": {
            "tools": [
                input_record(REPO_ROOT / GENERATOR_PATH),
                input_record(REPO_ROOT / "tx-pool/scripts/check_security_manifest.py"),
            ],
            "manifest": input_record(manifest_path),
            "architecture_contract": input_record(contract_path),
            "behavior_registry": input_record(behavior_path),
            "test_inventory": input_record(inventory),
            "production_sources": [
                input_record(REPO_ROOT / path) for path in paths
            ],
            "production_source_revision": production_source_revision(paths),
        },
        "test_universe": {
            "id": universe_id,
            "package": manifest["package"],
            "features": manifest["features"],
            "target": acceptance["test_target"],
            "count": len(unit_tests),
            "sha256": digest(unit_tests),
            "nextest_argv": [
                "cargo",
                "nextest",
                "run",
                "-p",
                manifest["package"],
                "--features",
                ",".join(manifest["features"]),
                f"--{acceptance['test_target']}",
            ],
        },
        "candidate_universe": {
            "count": len(rows),
            "sha256": digest(rows),
            "excluded_count": 0,
        },
        "execution": {
            "candidate_count": len(rows),
            "candidate_sha256": digest(rows),
            "config_sha256": hashlib.sha256(exact_config.encode()).hexdigest(),
            "command_template": command,
            "command_sha256": digest(command),
        },
        "evidence_sets": evidence_sets,
        "rows": rows,
    }


def write_text(path: Path, value: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(value)


def reconcile_lock(path: Path, generated: dict, write_lock: bool) -> None:
    rendered = json.dumps(generated, indent=2, sort_keys=True) + "\n"
    if write_lock:
        write_text(path, rendered)
        return
    try:
        observed = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot load generated mutation lock {path}: {error}; use --write-lock")
    if observed != generated:
        fail(f"generated mutation lock is stale: {path}; review then use --write-lock")


def read_json_object(path: Path, label: str) -> dict:
    try:
        value = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot load {label} {path}: {error}")
    if not isinstance(value, dict):
        fail(f"{label} must be a JSON object: {path}")
    return value


def candidate_from_lock_row(row: dict) -> dict:
    required = {
        "candidate",
        "file",
        "function",
        "genre",
        "replacement",
        "obligation_id",
    }
    if set(row) != required:
        fail(f"invalid candidate row fields: {row!r}")
    return {
        "name": row["candidate"],
        "file": row["file"],
        "function": {"function_name": row["function"]},
        "genre": row["genre"],
        "replacement": row["replacement"],
    }


def validate_lock_without_discovery(
    manifest_path: Path,
    lock_path: Path,
    manifest: dict,
    contract: dict,
    registry: dict,
    acceptance: dict,
) -> tuple[dict, list[dict], list[tuple[dict, dict]], str]:
    observed = read_json_object(lock_path, "generated mutation lock")
    if observed.get("schema_version") != 3:
        fail(f"generated mutation lock has unsupported schema: {lock_path}")
    universe = observed.get("candidate_universe")
    rows = observed.get("rows")
    if not isinstance(universe, dict) or not isinstance(rows, list) or not rows:
        fail("generated mutation lock has no complete candidate universe")
    candidates = [candidate_from_lock_row(row) for row in rows]
    obligations = resolve_obligations(acceptance, contract, registry)
    selected = select_candidates(candidates, obligations)
    exact_config = config_text(selected, candidates)
    version = observed.get("cargo_mutants_version")
    if not isinstance(version, str):
        fail("generated mutation lock lacks the cargo-mutants version")
    regenerated = build_lock(
        manifest_path,
        manifest,
        contract,
        registry,
        acceptance,
        selected,
        version,
        exact_config,
    )
    if observed != regenerated:
        fail(
            f"generated mutation lock is stale or internally inconsistent: {lock_path}; "
            "review then run --rediscover --write-lock from a clean checkpoint"
        )
    return observed, candidates, selected, exact_config


def outcome_path(path: Path) -> Path:
    if path.name == "outcomes.json":
        return path
    direct = path / "outcomes.json"
    nested = path / "mutants.out" / "outcomes.json"
    if direct.is_file():
        return direct
    if nested.is_file():
        return nested
    fail(f"cannot find cargo-mutants outcomes.json under {path}")


def read_outcome_sets(
    paths: list[Path], lock: dict
) -> tuple[list[dict], dict[str, str]]:
    expected = {row["candidate"] for row in lock["rows"]}
    locked_version = lock["cargo_mutants_version"].removeprefix("cargo-mutants ")
    inputs: list[dict] = []
    observed: dict[str, str] = {}
    for requested in paths:
        path = outcome_path(requested)
        outcomes = read_json_object(path, "cargo-mutants outcomes")
        records = outcomes.get("outcomes")
        if not isinstance(records, list) or not records:
            fail(f"mutation outcomes contain no baseline or mutant records: {path}")
        baseline = records[0]
        if baseline.get("scenario") != "Baseline" or baseline.get("summary") != "Success":
            fail(f"mutation baseline did not pass under the locked Nextest universe: {path}")
        version = outcomes.get("cargo_mutants_version")
        if version != locked_version:
            fail(
                f"mutation outcome tool version {version!r} differs from locked "
                f"version {locked_version!r}: {path}"
            )
        local_count = 0
        local_names: set[str] = set()
        for record in records[1:]:
            scenario = record.get("scenario")
            mutant = scenario.get("Mutant") if isinstance(scenario, dict) else None
            name = mutant.get("name") if isinstance(mutant, dict) else None
            summary = record.get("summary")
            if not isinstance(name, str) or not isinstance(summary, str):
                fail(f"invalid mutant outcome record: {record!r}")
            if summary not in MUTANT_OUTCOME_SUMMARIES:
                fail(f"unknown mutant outcome summary {summary!r}: {name}")
            if name not in expected:
                fail(f"unexpected mutant outcome outside the locked universe: {name}")
            if name in local_names:
                fail(f"duplicate mutant outcome inside one result set: {name}")
            if name in observed and observed[name] != summary:
                fail(
                    f"replayed mutant outcome changed from {observed[name]} "
                    f"to {summary}: {name}"
                )
            observed[name] = summary
            local_names.add(name)
            local_count += 1
        declared_total = outcomes.get("total_mutants")
        if declared_total != local_count:
            fail(
                f"mutation outcome count mismatch: declared={declared_total!r}, "
                f"observed={local_count}: {path}"
            )
        inputs.append(
            {
                "sha256": file_digest(path),
                "cargo_mutants_version": version,
                "start_time": outcomes.get("start_time"),
                "end_time": outcomes.get("end_time"),
                "candidate_count": local_count,
            }
        )
    return inputs, observed


def outcome_counts(observed: dict[str, str]) -> dict[str, int]:
    counts: dict[str, int] = {}
    for summary in observed.values():
        counts[summary] = counts.get(summary, 0) + 1
    return dict(sorted(counts.items()))


def result_disposition(
    candidate: str,
    summary: str,
    equivalence: dict[str, str],
) -> dict[str, str]:
    if summary == "CaughtMutant":
        return {"kind": "caught"}
    if summary == "Unviable":
        return {"kind": "compile_unviable"}
    if summary == "MissedMutant" and candidate in equivalence:
        return {"kind": "equivalent", "proof_id": equivalence[candidate]}
    return {"kind": "unaccepted"}


def disposition_counts(dispositions: dict[str, dict[str, str]]) -> dict[str, int]:
    counts: dict[str, int] = {}
    for disposition in dispositions.values():
        kind = disposition["kind"]
        counts[kind] = counts.get(kind, 0) + 1
    return dict(sorted(counts.items()))


def run_result_disposition_canaries() -> None:
    proof = {"candidate": "V1-EQ-CANARY"}
    expected = {
        "CaughtMutant": {"kind": "caught"},
        "Unviable": {"kind": "compile_unviable"},
        "MissedMutant": {"kind": "equivalent", "proof_id": "V1-EQ-CANARY"},
        "Timeout": {"kind": "unaccepted"},
    }
    for summary, disposition in expected.items():
        if result_disposition("candidate", summary, proof) != disposition:
            fail(f"mutation result disposition canary failed for {summary}")
    if result_disposition("unproved", "MissedMutant", proof) != {
        "kind": "unaccepted"
    }:
        fail("an unproved missed mutant received an accepted disposition")


def build_result_lock(
    mutation_lock_path: Path,
    mutation_lock: dict,
    inputs: list[dict],
    observed: dict[str, str],
    equivalence: dict[str, str],
) -> dict:
    expected = {row["candidate"] for row in mutation_lock["rows"]}
    unstarted = sorted(expected.difference(observed))
    if unstarted:
        fail(f"mutation result projection is incomplete; unstarted={unstarted}")
    dispositions = {
        name: result_disposition(name, observed[name], equivalence)
        for name in observed
    }
    accepted = all(
        disposition["kind"] != "unaccepted"
        for disposition in dispositions.values()
    )
    execution_count = sum(entry["candidate_count"] for entry in inputs)
    return {
        "schema_version": 3,
        "mutation_lock": input_record(mutation_lock_path),
        "candidate_count": len(expected),
        "execution_count": execution_count,
        "replayed_count": execution_count - len(expected),
        "outcome_inputs": inputs,
        "counts": outcome_counts(observed),
        "disposition_counts": disposition_counts(dispositions),
        "accepted": accepted,
        "rows": [
            {
                "candidate": name,
                "summary": observed[name],
                "disposition": dispositions[name],
            }
            for name in sorted(observed)
        ],
    }


def validate_result_lock(
    path: Path,
    mutation_lock_path: Path,
    mutation_lock: dict,
    equivalence: dict[str, str],
    require_accepted: bool,
) -> dict:
    result = read_json_object(path, "mutation result lock")
    if result.get("schema_version") != 3:
        fail(f"mutation result lock has unsupported schema: {path}")
    if result.get("mutation_lock") != input_record(mutation_lock_path):
        fail("mutation result lock is not bound to the current candidate lock")
    rows = result.get("rows")
    if not isinstance(rows, list):
        fail("mutation result lock rows must be a list")
    observed: dict[str, str] = {}
    dispositions: dict[str, dict[str, str]] = {}
    for row in rows:
        if not isinstance(row, dict) or set(row) != {
            "candidate",
            "summary",
            "disposition",
        }:
            fail(f"invalid mutation result row: {row!r}")
        name = row.get("candidate")
        summary = row.get("summary")
        if (
            not isinstance(name, str)
            or summary not in MUTANT_OUTCOME_SUMMARIES
            or name in observed
        ):
            fail(f"invalid or duplicate mutation result row: {row!r}")
        observed[name] = summary
        expected_disposition = result_disposition(name, summary, equivalence)
        if row.get("disposition") != expected_disposition:
            fail(f"mutation result disposition is stale: {name}")
        dispositions[name] = expected_disposition
    expected = {row["candidate"] for row in mutation_lock["rows"]}
    if set(observed) != expected or list(observed) != sorted(observed):
        fail("mutation result lock does not close the sorted candidate universe")
    counts = outcome_counts(observed)
    accepted = all(
        disposition["kind"] != "unaccepted"
        for disposition in dispositions.values()
    )
    if result.get("candidate_count") != len(expected):
        fail("mutation result lock candidate count is stale")
    if (
        result.get("counts") != counts
        or result.get("disposition_counts") != disposition_counts(dispositions)
        or result.get("accepted") is not accepted
    ):
        fail("mutation result lock summary is stale")
    inputs = result.get("outcome_inputs")
    input_fields = {
        "sha256",
        "cargo_mutants_version",
        "start_time",
        "end_time",
        "candidate_count",
    }
    if not isinstance(inputs, list) or not inputs:
        fail("mutation result lock outcome-input partition is invalid")
    input_count = 0
    input_hashes: set[str] = set()
    for entry in inputs:
        if not isinstance(entry, dict) or set(entry) != input_fields:
            fail(f"invalid mutation result input: {entry!r}")
        count = entry.get("candidate_count")
        sha256 = entry.get("sha256")
        if not isinstance(count, int) or count <= 0:
            fail(f"invalid mutation result input count: {entry!r}")
        if not isinstance(sha256, str) or not re.fullmatch(r"[0-9a-f]{64}", sha256):
            fail(f"invalid mutation result input digest: {entry!r}")
        if sha256 in input_hashes:
            fail(f"duplicate mutation result input digest: {sha256}")
        if entry.get("cargo_mutants_version") != mutation_lock[
            "cargo_mutants_version"
        ].removeprefix("cargo-mutants "):
            fail(f"mutation result input tool version is stale: {entry!r}")
        if not isinstance(entry.get("start_time"), str) or not isinstance(
            entry.get("end_time"), str
        ):
            fail(f"mutation result input timestamps are invalid: {entry!r}")
        input_hashes.add(sha256)
        input_count += count
    if input_count < len(expected):
        fail("mutation result lock outcome-input partition is invalid")
    if result.get("execution_count") != input_count:
        fail("mutation result lock execution count is stale")
    if result.get("replayed_count") != input_count - len(expected):
        fail("mutation result lock replay count is stale")
    if require_accepted and not accepted:
        unacceptable = sorted(
            name
            for name, disposition in dispositions.items()
            if disposition["kind"] == "unaccepted"
        )
        fail(f"mutation outcomes lack an accepted disposition: {unacceptable}")
    return result


def main() -> int:
    args = parse_args()
    if args.write_lock and not args.rediscover:
        fail("--write-lock requires explicit --rediscover")
    if args.resume_outcomes and args.write_config is None:
        fail("--resume-outcomes requires --write-config")
    if args.write_result_lock and not args.verify_outcomes:
        fail("--write-result-lock requires one or more --verify-outcomes inputs")
    if args.write_lock:
        require_clean_worktree({repo_relative(args.lock)})
    run_selection_canaries()
    run_equivalence_canaries()
    run_result_disposition_canaries()
    manifest, contract, registry, acceptance = load_inputs(args.manifest)
    if args.rediscover:
        obligations = resolve_obligations(acceptance, contract, registry)
        paths = sorted({obligation["owner"]["path"] for obligation in obligations})
        candidates = list_candidates(
            manifest["package"], manifest["features"], paths
        )
        selected = select_candidates(candidates, obligations)
        exact_config = config_text(selected, candidates)
        lock = build_lock(
            args.manifest,
            manifest,
            contract,
            registry,
            acceptance,
            selected,
            cargo_mutants_version(),
            exact_config,
        )
        reconcile_lock(args.lock, lock, args.write_lock)
    else:
        lock, candidates, selected, exact_config = validate_lock_without_discovery(
            args.manifest,
            args.lock,
            manifest,
            contract,
            registry,
            acceptance,
        )
    equivalence = equivalence_proof_index(
        contract,
        registry,
        set(read_unit_universe(inventory_path(manifest))),
        lock["rows"],
    )

    if args.write_config is not None:
        run_selected = selected
        completed: dict[str, str] = {}
        if args.resume_outcomes:
            _, completed = read_outcome_sets(args.resume_outcomes, lock)
            run_selected = [
                row for row in selected if row[0]["name"] not in completed
            ]
            if not run_selected:
                fail("resume outcome sets already close every locked candidate")
        run_config = config_text(run_selected, candidates)
        write_text(args.write_config, run_config)
        config_path = args.write_config
        relisted_for_run = verify_exact_config(
            config_path,
            run_selected,
            manifest["package"],
            manifest["features"],
            sorted({candidate["file"] for candidate in candidates}),
            set(completed),
        )

    result = None
    if args.verify_outcomes:
        inputs, observed = read_outcome_sets(args.verify_outcomes, lock)
        result = build_result_lock(args.lock, lock, inputs, observed, equivalence)
        if args.write_result_lock:
            write_text(
                args.result_lock,
                json.dumps(result, indent=2, sort_keys=True) + "\n",
            )
        else:
            checked = read_json_object(args.result_lock, "mutation result lock")
            if checked != result:
                fail(
                    f"mutation result lock is stale: {args.result_lock}; "
                    "review then use --write-result-lock"
                )
        if args.require_accepted and not result["accepted"]:
            unacceptable = [
                row["candidate"]
                for row in result["rows"]
                if row["disposition"]["kind"] == "unaccepted"
            ]
            fail(f"mutation outcomes lack an accepted disposition: {unacceptable}")
    elif args.result_lock.is_file():
        result = validate_result_lock(
            args.result_lock,
            args.lock,
            lock,
            equivalence,
            args.require_accepted,
        )
    elif args.require_accepted:
        fail(f"mutation result lock does not exist: {args.result_lock}")

    if args.print_json:
        print(json.dumps(lock, indent=2, sort_keys=True))
    else:
        execution = lock["execution"]
        print(
            f"validated {execution['candidate_count']} complete mutation rows "
            f"({execution['candidate_sha256']}) against "
            f"{lock['test_universe']['count']} library tests"
        )
        print(f"command: {shlex.join(execution['command_template'])}")
        if args.write_config is not None:
            print(
                f"config: {args.write_config} selects {len(run_selected)} "
                f"unstarted rows and relists {len(relisted_for_run)} executions"
            )
        if result is not None:
            print(
                f"outcomes: {json.dumps(result['counts'], sort_keys=True)}; "
                f"dispositions: {json.dumps(result['disposition_counts'], sort_keys=True)}; "
                f"accepted={str(result['accepted']).lower()}"
            )
    return 0


if __name__ == "__main__":
    sys.exit(main())
