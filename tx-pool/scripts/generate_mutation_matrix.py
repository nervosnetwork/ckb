#!/usr/bin/env python3
"""Generate and verify the V1 semantic mutation-acceptance matrix."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import shlex
import subprocess
import sys
import tempfile

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
GENERATOR_PATH = "tx-pool/scripts/generate_mutation_matrix.py"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument("--lock", type=Path, default=DEFAULT_LOCK)
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
        "--verify-outcomes",
        type=Path,
        help="reconcile cargo-mutants outcomes with every locked row",
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


def require_clean_worktree() -> None:
    status = run(["git", "status", "--porcelain=v1", "--untracked-files=all"])
    if status.strip():
        fail(
            "--write-lock requires a clean recoverable Git checkpoint; "
            f"current changes:\n{status.rstrip()}"
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
        if not isinstance(name, str) or not isinstance(function, dict):
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
    fail(f"unhandled mutation selector: {selector!r}")


def select_candidates(candidates: list[dict], obligations: list[dict]) -> list[tuple[dict, dict]]:
    selected: list[tuple[dict, dict]] = []
    matched_obligations: set[str] = set()
    for candidate in candidates:
        matches = [
            obligation
            for obligation in obligations
            if candidate["file"] == obligation["owner"]["path"]
            and obligation_matches(candidate, obligation)
        ]
        if len(matches) > 1:
            fail(
                f"ambiguous mutation row {candidate['name']!r}: "
                f"{[entry['id'] for entry in matches]}"
            )
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


def config_text(
    selected: list[tuple[dict, dict]], candidates: list[dict]
) -> str:
    selected_names = {candidate["name"] for candidate, _ in selected}
    lines = [
        "# Generated by tx-pool/scripts/generate_mutation_matrix.py.",
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
) -> None:
    relisted = list_candidates(package, features, paths, config)
    expected = [candidate["name"] for candidate, _ in selected]
    observed = sorted(candidate["name"] for candidate in relisted)
    if observed != expected:
        missing = sorted(set(expected).difference(observed))
        unowned = sorted(set(observed).difference(expected))
        fail(
            "exact cargo-mutants config did not close: "
            f"missing={missing}, unowned={unowned}"
        )


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


def candidate_core(candidate: dict) -> dict:
    return {
        "name": candidate["name"],
        "file": candidate["file"],
        "function": candidate["function"]["function_name"],
        "function_span": candidate["function"].get("span"),
        "mutation_span": candidate.get("span"),
        "genre": candidate.get("genre"),
        "replacement": candidate.get("replacement"),
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
                "function": candidate["function"]["function_name"],
                "genre": candidate.get("genre"),
                "obligation_id": obligation["id"],
            }
        )
    command = command_template(
        manifest["package"], manifest["features"], paths, acceptance["test_target"]
    )
    contract_path = REPO_ROOT / manifest["architecture_contract"]
    behavior_path = registry_path(manifest)
    return {
        "schema_version": 1,
        "generator": GENERATOR_PATH,
        "cargo_mutants_version": version,
        "inputs": {
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
        "execution": {
            "candidate_count": len(rows),
            "candidate_sha256": digest([candidate_core(row) for row, _ in selected]),
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


def verify_outcomes(path: Path, lock: dict) -> dict[str, int]:
    try:
        outcomes = json.loads(outcome_path(path).read_text())
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot load mutation outcomes: {error}")
    records = outcomes.get("outcomes")
    if not isinstance(records, list) or not records:
        fail("mutation outcomes contain no baseline or mutant records")
    baseline = records[0]
    if baseline.get("scenario") != "Baseline" or baseline.get("summary") != "Success":
        fail("mutation baseline did not pass under the locked Nextest universe")
    observed: dict[str, str] = {}
    for record in records[1:]:
        scenario = record.get("scenario")
        mutant = scenario.get("Mutant") if isinstance(scenario, dict) else None
        name = mutant.get("name") if isinstance(mutant, dict) else None
        summary = record.get("summary")
        if not isinstance(name, str) or not isinstance(summary, str):
            fail(f"invalid mutant outcome record: {record!r}")
        if name in observed:
            fail(f"duplicate mutant outcome: {name}")
        observed[name] = summary
    expected = {row["candidate"] for row in lock["rows"]}
    unstarted = sorted(expected.difference(observed))
    unexpected = sorted(set(observed).difference(expected))
    if unstarted or unexpected:
        fail(f"mutation outcome closure failed: unstarted={unstarted}, unexpected={unexpected}")
    counts: dict[str, int] = {}
    for summary in observed.values():
        counts[summary] = counts.get(summary, 0) + 1
    unacceptable = sorted(
        name
        for name, summary in observed.items()
        if summary not in {"CaughtMutant", "Unviable"}
    )
    if unacceptable:
        fail(f"mutation survivors/timeouts require semantic adjudication: {unacceptable}")
    return counts


def main() -> int:
    args = parse_args()
    if args.write_lock:
        require_clean_worktree()
    run_selection_canaries()
    manifest, contract, registry, acceptance = load_inputs(args.manifest)
    obligations = resolve_obligations(acceptance, contract, registry)
    paths = sorted({obligation["owner"]["path"] for obligation in obligations})
    candidates = list_candidates(
        manifest["package"], manifest["features"], paths
    )
    selected = select_candidates(candidates, obligations)
    exact_config = config_text(selected, candidates)
    if args.write_config is not None:
        write_text(args.write_config, exact_config)
        config_path = args.write_config
        verify_exact_config(
            config_path,
            selected,
            manifest["package"],
            manifest["features"],
            paths,
        )
    else:
        with tempfile.TemporaryDirectory(prefix="ckb-mutation-matrix-") as directory:
            config_path = Path(directory) / "mutants.toml"
            write_text(config_path, exact_config)
            verify_exact_config(
                config_path,
                selected,
                manifest["package"],
                manifest["features"],
                paths,
            )
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
    outcome_counts = None
    if args.verify_outcomes is not None:
        outcome_counts = verify_outcomes(args.verify_outcomes, lock)
    if args.print_json:
        print(json.dumps(lock, indent=2, sort_keys=True))
    else:
        execution = lock["execution"]
        print(
            f"validated {execution['candidate_count']} exact mutation rows "
            f"({execution['candidate_sha256']}) against "
            f"{lock['test_universe']['count']} library tests"
        )
        print(f"command: {shlex.join(execution['command_template'])}")
        if outcome_counts is not None:
            print(f"outcomes: {json.dumps(outcome_counts, sort_keys=True)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
