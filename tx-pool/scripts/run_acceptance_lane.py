#!/usr/bin/env python3
"""Execute one fixed tx-pool Acceptance lane and emit a bound result record.

This runner owns execution mechanics only.  The architecture contract owns the
phase DAG and the security manifest checker owns admission of a result into the
convergence state.
"""

from __future__ import annotations

import argparse
from contextlib import ExitStack, contextmanager
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
from typing import BinaryIO


REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_MANIFEST = REPO_ROOT / "tx-pool" / "security-regression-manifest.json"
ENVIRONMENT_KEYS = (
    "CARGO_BUILD_JOBS",
    "CARGO_ENCODED_RUSTFLAGS",
    "CARGO_INCREMENTAL",
    "CARGO_TARGET_DIR",
    "CI",
    "RUSTFLAGS",
    "RUSTUP_TOOLCHAIN",
)
RESULT_FIELDS = {
    "schema_version",
    "phase",
    "acceptance_universe",
    "subject",
    "runner_sha256",
    "command_plan_sha256",
    "environment",
    "execution",
    "prerequisite_evidence_sha256",
    "commands",
    "artifacts",
    "observations",
    "outcome",
    "evidence_sha256",
}
EXECUTION_FIELDS = {
    "kind",
    "tree",
    "clean_status_sha256",
    "submodule_source_set_sha256",
    "tool_source_set_sha256",
}
SUBMODULE_SOURCE_FIELDS = {"path", "commit", "tree", "tree_listing_sha256"}
COMMAND_RESULT_FIELDS = {
    "argv",
    "cwd",
    "exit_code",
    "output_sha256",
    "output_bytes",
}
ARTIFACT_FIELDS = {"path", "sha256"}
ACCEPTANCE_PHASES = (
    "complete_correctness",
    "deterministic_smoke",
    "complete_mutation",
    "empirical_performance_acceptance",
    "portability_and_final_review",
)
MUTATION_ARTIFACTS = (
    "tx-pool/mutation-acceptance-lock.json",
    "tx-pool/mutation-result-lock.json",
)
MUTATION_DIAGNOSTIC_ARTIFACT = (
    "tx-pool/optimization-evidence/acceptance/complete_mutation-diagnostic.txt"
)
PERFORMANCE_ARTIFACT = "tx-pool/optimization-evidence/performance-acceptance.json"
RELEASE_PROGRESS = "tx-pool/.release-progress"
SECURITY_MANIFEST = "tx-pool/security-regression-manifest.json"


def canonical_json_sha256(value: object) -> str:
    payload = json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True)
    return hashlib.sha256(payload.encode()).hexdigest()


def file_sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _repository_file(relative: str) -> Path:
    path = Path(relative)
    if path.is_absolute() or ".." in path.parts:
        raise ValueError(f"Acceptance input path escapes the repository: {relative}")
    resolved = (REPO_ROOT / path).resolve()
    try:
        resolved.relative_to(REPO_ROOT.resolve())
    except ValueError as error:
        raise ValueError(
            f"Acceptance input path escapes the repository: {relative}"
        ) from error
    return resolved


def _joined_acceptance_inputs(
    manifest_path: Path, manifest: dict[str, object]
) -> dict[str, bytes]:
    """Load the exact pre-final U and predecessor evidence for cold replay."""

    status = manifest.get("convergence_status")
    if not isinstance(status, dict):
        raise ValueError("cold Acceptance replay has no convergence status")
    universe_ref = status.get("acceptance_universe")
    if not isinstance(universe_ref, dict) or not isinstance(
        universe_ref.get("path"), str
    ):
        raise ValueError("cold Acceptance replay has no frozen universe")
    payloads = {
        SECURITY_MANIFEST: manifest_path.read_bytes(),
        RELEASE_PROGRESS: _repository_file(RELEASE_PROGRESS).read_bytes(),
    }
    universe_path = universe_ref["path"]
    universe_source = _repository_file(universe_path)
    universe_payload = universe_source.read_bytes()
    if file_sha256(universe_source) != universe_ref.get("sha256"):
        raise ValueError("cold Acceptance replay universe hash differs")
    payloads[universe_path] = universe_payload
    evidence = status.get("acceptance_evidence")
    if not isinstance(evidence, dict):
        raise ValueError("cold Acceptance replay evidence is invalid")
    completed_acceptance = set(status.get("completed_phases", [])).intersection(
        ACCEPTANCE_PHASES
    )
    if set(evidence) != completed_acceptance:
        raise ValueError(
            "cold Acceptance replay evidence does not match completed phases"
        )
    for phase, reference in sorted(evidence.items()):
        if not isinstance(reference, dict) or not isinstance(reference.get("path"), str):
            raise ValueError(f"cold Acceptance reference {phase} is invalid")
        path_value = reference["path"]
        result_payload = _repository_file(path_value).read_bytes()
        if hashlib.sha256(result_payload).hexdigest() != reference.get("sha256"):
            raise ValueError(f"cold Acceptance result {phase} hash differs")
        result = json.loads(result_payload)
        if not isinstance(result, dict):
            raise ValueError(f"cold Acceptance result {phase} is not an object")
        if result.get("evidence_sha256") != reference.get("evidence_sha256"):
            raise ValueError(f"cold Acceptance result {phase} evidence differs")
        payloads[path_value] = result_payload
        artifacts = result.get("artifacts")
        if not isinstance(artifacts, list):
            raise ValueError(f"cold Acceptance result {phase} artifacts are invalid")
        for artifact in artifacts:
            if not isinstance(artifact, dict) or not isinstance(artifact.get("path"), str):
                raise ValueError(f"cold Acceptance result {phase} artifact is invalid")
            artifact_path = artifact["path"]
            artifact_payload = _repository_file(artifact_path).read_bytes()
            if hashlib.sha256(artifact_payload).hexdigest() != artifact.get("sha256"):
                raise ValueError(f"cold Acceptance artifact {artifact_path} hash differs")
            payloads[artifact_path] = artifact_payload
    return payloads


def phase_command_plan(phase: str) -> list[dict[str, object]]:
    """Return the fixed execution plan for an implemented Acceptance lane."""

    plans: dict[str, list[list[str]]] = {
        "complete_correctness": [
            ["python3", "tx-pool/scripts/check_all.py"],
            ["cargo", "nextest", "run", "-p", "ckb-tx-pool", "--features", "internal"],
            [
                "cargo",
                "clippy",
                "-p",
                "ckb-tx-pool",
                "--all-targets",
                "--features",
                "internal",
                "--",
                "-D",
                "warnings",
            ],
            ["python3", "tx-pool/scripts/run_managed_integration.py"],
        ],
        "deterministic_smoke": [
            ["python3", "tx-pool/scripts/run_managed_integration.py", "--anchors"],
        ],
        "complete_mutation": [
            [
                "python3",
                "tx-pool/scripts/check_mutation_matrix.py",
                "--rediscover",
                "--write-lock",
                "--write-config",
                "<CONFIG>",
            ],
            [
                "cargo",
                "nextest",
                "run",
                "--no-run",
                "-p",
                "ckb-tx-pool",
                "--features",
                "internal",
                "--lib",
            ],
            [
                "cargo",
                "mutants",
                "--config",
                "<CONFIG>",
                "<LOCKED-MUTATION-SELECTION>",
                "-o",
                "<OUTCOMES>",
                "--",
                "--lib",
            ],
            [
                "python3",
                "tx-pool/scripts/check_mutation_matrix.py",
                "--verify-outcomes",
                "<OUTCOMES>",
                "--write-result-lock",
                "--require-accepted",
            ],
        ],
        "empirical_performance_acceptance": [
            ["python3", "tx-pool/scripts/cross_version_benchmark.py", "<FROZEN-X2-MATRIX>"],
        ],
        "portability_and_final_review": [
            ["python3", "tx-pool/scripts/check_all.py"],
            ["python3", "tx-pool/scripts/check_security_manifest.py"],
        ],
    }
    commands = plans.get(phase)
    if commands is None:
        raise ValueError(f"Acceptance lane {phase!r} has no executable plan yet")
    return [{"argv": command, "cwd": "."} for command in commands]


def phase_commands_passed(phase: str, commands: object) -> bool:
    """Return whether the policy-owned terminal command accepted the lane.

    cargo-mutants deliberately exits non-zero when it observes missed mutants.
    That raw observation is not the mutation lane's disposition authority: the
    frozen mutation-matrix verifier is.  Preserve the tool exit code in the
    record, but let the final verifier decide whether every missed row has a
    pre-existing equivalence proof.
    """

    if not isinstance(commands, list):
        return False
    try:
        expected_count = len(phase_command_plan(phase))
    except ValueError:
        return False
    if len(commands) != expected_count or not all(
        isinstance(command, dict) and isinstance(command.get("exit_code"), int)
        for command in commands
    ):
        return False
    if phase == "complete_mutation":
        return (
            commands[0]["exit_code"] == 0
            and commands[1]["exit_code"] == 0
            and commands[3]["exit_code"] == 0
        )
    return all(command["exit_code"] == 0 for command in commands)


def phase_failed_prefix_is_valid(phase: str, commands: object) -> bool:
    """Reject incomplete or post-failure command traces in a failed result."""

    if not isinstance(commands, list) or not commands or not all(
        isinstance(command, dict) and isinstance(command.get("exit_code"), int)
        for command in commands
    ):
        return False
    if phase == "complete_mutation":
        return (
            len(commands) == 1
            and commands[0]["exit_code"] != 0
        ) or (
            len(commands) == 2
            and commands[0]["exit_code"] == 0
            and commands[1]["exit_code"] != 0
        ) or (
            len(commands) == 4
            and commands[0]["exit_code"] == 0
            and commands[1]["exit_code"] == 0
            and commands[3]["exit_code"] != 0
        )
    return all(command["exit_code"] == 0 for command in commands[:-1]) and (
        commands[-1]["exit_code"] != 0
    )


def phase_failed_diagnostic_is_required(phase: str, commands: object) -> bool:
    """Return whether a terminal verifier failure must persist its diagnosis."""

    return (
        phase == "complete_mutation"
        and isinstance(commands, list)
        and len(commands) == 4
        and all(
            isinstance(command, dict) and isinstance(command.get("exit_code"), int)
            for command in commands
        )
        and commands[0].get("exit_code") == 0
        and commands[1].get("exit_code") == 0
        and commands[3].get("exit_code") != 0
    )


def control_flow_canary_errors() -> list[str]:
    """Exercise the mutation observer/adjudicator boundary without I/O."""

    errors: list[str] = []
    command = lambda exit_code: {"exit_code": exit_code}
    if not phase_commands_passed(
        "complete_mutation", [command(0), command(0), command(2), command(0)]
    ):
        errors.append("mutation runner canary rejected adjudicated missed mutants")
    if phase_commands_passed(
        "complete_mutation", [command(0), command(0), command(0), command(1)]
    ):
        errors.append("mutation runner canary bypassed the final adjudicator")
    if not phase_failed_prefix_is_valid(
        "complete_mutation", [command(0), command(0), command(2), command(1)]
    ):
        errors.append("mutation runner canary rejected an adjudicator failure")
    if not phase_failed_prefix_is_valid(
        "complete_mutation", [command(0), command(1)]
    ):
        errors.append("mutation runner canary rejected a prewarm failure")
    if phase_failed_prefix_is_valid(
        "complete_mutation", [command(0), command(0), command(2)]
    ):
        errors.append("mutation runner canary admitted an unadjudicated result")
    if not phase_failed_diagnostic_is_required(
        "complete_mutation", [command(0), command(0), command(2), command(1)]
    ):
        errors.append("mutation runner canary discarded a terminal failure diagnosis")
    if phase_failed_diagnostic_is_required(
        "complete_mutation", [command(0), command(0), command(2), command(0)]
    ):
        errors.append("mutation runner canary attached failure diagnostics to a passed lane")
    if phase_failed_diagnostic_is_required(
        "complete_mutation", [command(0), command(0), command(2)]
    ):
        errors.append("mutation runner canary diagnosed an unadjudicated prefix")
    if not phase_commands_passed(
        "complete_correctness", [command(0), command(0), command(0), command(0)]
    ):
        errors.append("ordinary Acceptance runner canary rejected a green lane")
    errors.extend(environment_canary_errors())
    return errors


def validate_result_shape(value: object) -> list[str]:
    """Validate the self-contained, policy-free result envelope."""

    errors: list[str] = []
    if not isinstance(value, dict) or set(value) != RESULT_FIELDS:
        return ["Acceptance result fields differ"]
    if value.get("schema_version") != 2:
        errors.append("Acceptance result schema version differs")
    phase = value.get("phase")
    if not isinstance(phase, str):
        errors.append("Acceptance result phase is invalid")
    else:
        try:
            expected_plan = phase_command_plan(phase)
        except ValueError as error:
            errors.append(str(error))
        else:
            if value.get("command_plan_sha256") != canonical_json_sha256(expected_plan):
                errors.append("Acceptance result command plan differs")
    universe = value.get("acceptance_universe")
    if not isinstance(universe, dict) or set(universe) != {"path", "sha256"}:
        errors.append("Acceptance result universe binding differs")
    subject = value.get("subject")
    if not isinstance(subject, dict) or set(subject) != {"checkpoint", "tree"}:
        errors.append("Acceptance result subject binding differs")
    for label, digest in (
        ("runner", value.get("runner_sha256")),
        ("command plan", value.get("command_plan_sha256")),
    ):
        if not isinstance(digest, str) or len(digest) != 64:
            errors.append(f"Acceptance result {label} SHA-256 is invalid")
    environment = value.get("environment")
    if not isinstance(environment, dict) or set(environment) != set(ENVIRONMENT_KEYS):
        errors.append("Acceptance result environment projection differs")
    elif any(item is not None and not isinstance(item, str) for item in environment.values()):
        errors.append("Acceptance result environment value is invalid")
    execution = value.get("execution")
    if not isinstance(execution, dict) or set(execution) != EXECUTION_FIELDS:
        errors.append("Acceptance result execution projection differs")
    else:
        if execution.get("kind") != "detached_exact_checkpoint":
            errors.append("Acceptance result execution kind differs")
        if execution.get("tree") != value.get("subject", {}).get("tree"):
            errors.append("Acceptance result execution tree differs")
        if execution.get("clean_status_sha256") != hashlib.sha256(b"").hexdigest():
            errors.append("Acceptance result checkout was not clean after execution")
        for field in ("tree", "clean_status_sha256", "tool_source_set_sha256"):
            digest = execution.get(field)
            expected_length = 40 if field == "tree" else 64
            if not isinstance(digest, str) or len(digest) != expected_length:
                errors.append(f"Acceptance result execution {field} is invalid")
    prerequisites = value.get("prerequisite_evidence_sha256")
    if not isinstance(prerequisites, dict) or any(
        not isinstance(key, str)
        or not isinstance(item, str)
        or len(item) != 64
        for key, item in prerequisites.items()
    ):
        errors.append("Acceptance result prerequisite projection is invalid")
    artifacts = value.get("artifacts")
    artifact_path_set: set[str] = set()
    if not isinstance(artifacts, list):
        errors.append("Acceptance result artifact projection is invalid")
    else:
        paths: list[str] = []
        for artifact in artifacts:
            if not isinstance(artifact, dict) or set(artifact) != ARTIFACT_FIELDS:
                errors.append("Acceptance result artifact fields differ")
                continue
            path = artifact.get("path")
            digest = artifact.get("sha256")
            if (
                not isinstance(path, str)
                or not path
                or path.startswith("/")
                or ".." in Path(path).parts
                or not isinstance(digest, str)
                or len(digest) != 64
            ):
                errors.append("Acceptance result artifact identity is invalid")
                continue
            paths.append(path)
        if paths != sorted(set(paths)):
            errors.append("Acceptance result artifacts are not unique and sorted")
        artifact_path_set = set(paths)
    if not isinstance(value.get("observations"), dict):
        errors.append("Acceptance result observations are invalid")
    commands = value.get("commands")
    if not isinstance(commands, list):
        errors.append("Acceptance result commands are invalid")
    else:
        expected_plan = []
        if isinstance(phase, str):
            try:
                expected_plan = phase_command_plan(phase)
            except ValueError:
                pass
        if not commands or len(commands) > len(expected_plan):
            errors.append("Acceptance result command prefix differs")
        for index, command in enumerate(commands):
            if not isinstance(command, dict) or set(command) != COMMAND_RESULT_FIELDS:
                errors.append(f"Acceptance result command {index} fields differ")
                continue
            if index < len(expected_plan) and {
                "argv": command.get("argv"),
                "cwd": command.get("cwd"),
            } != expected_plan[index]:
                errors.append(f"Acceptance result command {index} identity differs")
            if not isinstance(command.get("exit_code"), int):
                errors.append(f"Acceptance result command {index} exit code is invalid")
            if not isinstance(command.get("output_bytes"), int) or command["output_bytes"] < 0:
                errors.append(f"Acceptance result command {index} byte count is invalid")
            digest = command.get("output_sha256")
            if not isinstance(digest, str) or len(digest) != 64:
                errors.append(f"Acceptance result command {index} output hash is invalid")
    outcome = value.get("outcome")
    if outcome not in {"passed", "failed"}:
        errors.append("Acceptance result outcome is invalid")
    elif isinstance(commands, list):
        passed = phase_commands_passed(phase, commands) if isinstance(phase, str) else False
        if (outcome == "passed") is not passed:
            errors.append("Acceptance result outcome differs from command results")
        if outcome == "failed" and not (
            isinstance(phase, str) and phase_failed_prefix_is_valid(phase, commands)
        ):
            errors.append("failed Acceptance result is not the first failing prefix")
        if phase == "complete_mutation" and passed:
            if artifact_path_set != set(MUTATION_ARTIFACTS):
                errors.append("passed mutation result artifact universe differs")
        elif phase_failed_diagnostic_is_required(phase, commands):
            if MUTATION_DIAGNOSTIC_ARTIFACT not in artifact_path_set:
                errors.append("failed mutation result discarded its terminal diagnosis")
            observations = value.get("observations")
            if isinstance(observations, dict) and observations.get("accepted") is False:
                if not set(MUTATION_ARTIFACTS).issubset(artifact_path_set):
                    errors.append("failed mutation disposition discarded its row locks")
                unaccepted = observations.get("unaccepted_candidates")
                if not isinstance(unaccepted, list) or not unaccepted:
                    errors.append("failed mutation disposition has no exact blocker rows")
    evidence = value.get("evidence_sha256")
    expected_evidence = canonical_json_sha256(
        {key: item for key, item in value.items() if key != "evidence_sha256"}
    )
    if evidence != expected_evidence:
        errors.append("Acceptance result evidence SHA-256 differs")
    return errors


def _run_git(argv: list[str], *, cwd: Path = REPO_ROOT) -> str:
    completed = subprocess.run(
        ["git", *argv],
        cwd=cwd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        check=False,
    )
    if completed.returncode != 0:
        raise ValueError(
            f"git {' '.join(argv)} failed ({completed.returncode}): "
            f"{completed.stderr.strip()}"
        )
    return completed.stdout


def _clean_status(checkout: Path) -> str:
    return _run_git(
        [
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--ignore-submodules=untracked",
        ],
        cwd=checkout,
    )


def _git_object_identity(repository: Path, commit: str) -> tuple[str, str]:
    """Return the exact tree and recursive listing identity for one commit."""

    tree = _run_git(["rev-parse", f"{commit}^{{tree}}"], cwd=repository).strip()
    listing = subprocess.run(
        ["git", "ls-tree", "-rz", "--full-tree", commit],
        cwd=repository,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if listing.returncode != 0:
        raise ValueError(
            f"cannot enumerate frozen submodule {repository}: "
            + listing.stderr.decode(errors="replace").strip()
        )
    return tree, hashlib.sha256(listing.stdout).hexdigest()


def _materialize_frozen_submodules(checkout: Path, sources: object) -> str:
    """Populate exact gitlinks from already frozen local object repositories."""

    if not isinstance(sources, list):
        raise ValueError("Acceptance universe submodule source closure is invalid")
    rows: list[dict[str, str]] = []
    paths: set[str] = set()
    for item in sources:
        if (
            not isinstance(item, dict)
            or set(item) != SUBMODULE_SOURCE_FIELDS
            or not all(
                isinstance(item.get(field), str) for field in SUBMODULE_SOURCE_FIELDS
            )
        ):
            raise ValueError("Acceptance universe submodule source row is invalid")
        path = item["path"]
        if (
            not path
            or Path(path).is_absolute()
            or ".." in Path(path).parts
            or re.fullmatch(r"[0-9a-f]{40}", item["commit"]) is None
            or re.fullmatch(r"[0-9a-f]{40}", item["tree"]) is None
            or re.fullmatch(r"[0-9a-f]{64}", item["tree_listing_sha256"]) is None
            or path in paths
        ):
            raise ValueError("Acceptance universe submodule source identity is invalid")
        paths.add(path)
        rows.append(item)
    if rows != sorted(rows, key=lambda row: (len(Path(row["path"]).parts), row["path"])):
        raise ValueError("Acceptance universe submodule source order differs")

    for row in rows:
        relative = row["path"]
        source = REPO_ROOT / relative
        destination = checkout / relative
        if not source.is_dir():
            raise ValueError(f"frozen submodule object source is unavailable: {relative}")
        observed_tree, observed_listing = _git_object_identity(source, row["commit"])
        if (
            observed_tree != row["tree"]
            or observed_listing != row["tree_listing_sha256"]
        ):
            raise ValueError(f"frozen submodule object source differs: {relative}")
        if destination.exists():
            if any(destination.iterdir()):
                raise ValueError(f"uninitialized submodule path is not empty: {relative}")
            destination.rmdir()
        destination.parent.mkdir(parents=True, exist_ok=True)
        _run_git(["clone", "--no-checkout", str(source), str(destination)], cwd=checkout)
        _run_git(["checkout", "--detach", row["commit"]], cwd=destination)
        materialized_tree, materialized_listing = _git_object_identity(
            destination, row["commit"]
        )
        if (
            materialized_tree != row["tree"]
            or materialized_listing != row["tree_listing_sha256"]
            or _run_git(["rev-parse", "HEAD"], cwd=destination).strip()
            != row["commit"]
        ):
            raise ValueError(f"materialized frozen submodule differs: {relative}")
    return canonical_json_sha256(rows)


@contextmanager
def _detached_checkpoint(
    checkpoint: str,
    expected_tree: str,
    submodule_sources: object,
    *,
    prefix: str = "ckb-txpool-acceptance-",
):
    """Materialize one exact, disposable Git subject for a lane."""

    with tempfile.TemporaryDirectory(prefix=prefix) as parent:
        checkout = Path(parent) / "checkout"
        _run_git(["worktree", "add", "--detach", str(checkout), checkpoint])
        try:
            observed_tree = _run_git(["rev-parse", "HEAD^{tree}"], cwd=checkout).strip()
            if observed_tree != expected_tree:
                raise ValueError(
                    "detached Acceptance checkout tree differs: "
                    f"expected={expected_tree}, observed={observed_tree}"
                )
            status = _clean_status(checkout)
            if status:
                raise ValueError(f"detached Acceptance checkout is initially dirty:\n{status}")
            submodule_source_set_sha256 = _materialize_frozen_submodules(
                checkout, submodule_sources
            )
            yield checkout, submodule_source_set_sha256
        finally:
            completed = subprocess.run(
                ["git", "worktree", "remove", "--force", str(checkout)],
                cwd=REPO_ROOT,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                check=False,
            )
            if completed.returncode != 0:
                raise ValueError(
                    "cannot retire detached Acceptance checkout: "
                    + completed.stderr.strip()
                )


def _stream_command(
    argv: list[str],
    output: BinaryIO,
    *,
    cwd: Path,
    environment: dict[str, str],
    capture_path: Path | None = None,
) -> tuple[int, str, int]:
    digest = hashlib.sha256()
    count = 0
    process = subprocess.Popen(
        argv,
        cwd=cwd,
        env=environment,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    assert process.stdout is not None
    capture = capture_path.open("wb") if capture_path is not None else None
    try:
        while True:
            chunk = process.stdout.read(64 * 1024)
            if not chunk:
                break
            digest.update(chunk)
            count += len(chunk)
            output.write(chunk)
            output.flush()
            if capture is not None:
                capture.write(chunk)
    finally:
        if capture is not None:
            capture.close()
    return process.wait(), digest.hexdigest(), count


def _effective_environment(checkpoint: str, phase: str) -> dict[str, str]:
    environment = dict(os.environ)
    if phase == "complete_mutation":
        # cargo-mutants owns one copied source tree per worker and expects each
        # tree to own its own target directory.  A process-wide target override
        # aliases otherwise isolated workers and lets one mutant reuse another
        # worker's Cargo fingerprints or binaries.  An explicit relative value
        # is resolved from each cargo subprocess's worker-local source root.
        environment["CARGO_TARGET_DIR"] = "target"
    else:
        environment.setdefault(
            "CARGO_TARGET_DIR",
            str(REPO_ROOT / "target" / "tx-pool-acceptance" / checkpoint),
        )
    environment.setdefault("CARGO_INCREMENTAL", "0")
    return environment


def environment_canary_errors() -> list[str]:
    """Reject a process-wide target alias in the parallel mutation lane."""

    previous = os.environ.get("CARGO_TARGET_DIR")
    os.environ["CARGO_TARGET_DIR"] = "/shared/mutation-target"
    try:
        mutation = _effective_environment("canary", "complete_mutation")
        ordinary = _effective_environment("canary", "complete_correctness")
    finally:
        if previous is None:
            os.environ.pop("CARGO_TARGET_DIR", None)
        else:
            os.environ["CARGO_TARGET_DIR"] = previous
    errors: list[str] = []
    if mutation.get("CARGO_TARGET_DIR") != "target":
        errors.append("parallel mutation workers retained one shared Cargo target")
    if ordinary.get("CARGO_TARGET_DIR") != "/shared/mutation-target":
        errors.append("ordinary Acceptance lost its caller-owned Cargo target")
    parallel = ["cargo", "mutants", "-j", "4"]
    try:
        _validate_mutation_worker_isolation(
            parallel, {"CARGO_TARGET_DIR": "/shared/mutation-target"}
        )
    except ValueError:
        pass
    else:
        errors.append("mutation worker-isolation guard admitted an absolute shared target")
    try:
        _validate_mutation_worker_isolation(parallel, {"CARGO_TARGET_DIR": "target"})
    except ValueError:
        errors.append("mutation worker-isolation guard rejected a relative worker target")
    return errors


def _validate_mutation_worker_isolation(
    argv: list[str], environment: dict[str, str]
) -> None:
    """Require each parallel cargo-mutants source tree to own its Cargo output."""

    try:
        jobs_index = argv.index("-j")
        jobs = int(argv[jobs_index + 1])
    except (ValueError, IndexError) as error:
        raise ValueError("mutation command has no valid bounded worker count") from error
    target = environment.get("CARGO_TARGET_DIR")
    if jobs > 1 and (not target or Path(target).is_absolute()):
        raise ValueError(
            "parallel mutation workers require one relative Cargo target per source tree"
        )


def _validate_tool_sources(checkout: Path, sources: dict[str, str]) -> None:
    observed: dict[str, str] = {}
    for relative, expected in sorted(sources.items()):
        path = checkout / relative
        try:
            digest = file_sha256(path)
        except OSError as error:
            raise ValueError(
                f"frozen Acceptance checkout lacks tool source {relative}: {error}"
            ) from error
        if digest != expected:
            raise ValueError(
                f"frozen Acceptance tool source differs for {relative}: "
                f"expected={expected}, observed={digest}"
            )
        observed[relative] = digest
    if observed != dict(sorted(sources.items())):
        raise ValueError("frozen Acceptance tool source set differs")


def _bundle_path(output_path: Path, relative: str) -> Path:
    return Path(str(output_path) + ".artifacts") / relative


def _copy_artifact(checkout: Path, output_path: Path, relative: str) -> dict[str, str]:
    source = checkout / relative
    target = _bundle_path(output_path, relative)
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes(source.read_bytes())
    return {"path": relative, "sha256": file_sha256(target)}


def _stage_artifact(source: Path, output_path: Path, relative: str) -> dict[str, str]:
    target = _bundle_path(output_path, relative)
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes(source.read_bytes())
    return {"path": relative, "sha256": file_sha256(target)}


def _performance_scenarios(checkout: Path) -> list[str]:
    contract = json.loads((checkout / "tx-pool/architecture-contract.json").read_text())
    matrix = contract["declared_workload_environment_matrix"]
    scenarios: list[str] = []
    for family in matrix["timed_families"]:
        for name in family["scenarios"]:
            for population in family["pool_populations"]:
                for peers in family["peers"]:
                    for workers in family["workers"]:
                        for target in family["target_transactions"]:
                            scenarios.append(
                                f"{name},{target},{population['warm_transactions']},{workers},{peers}"
                            )
    # Required hostile-shape timing is a confirmation observation, not an X2
    # selection axis. A deliberately delayed committed-effect callback keeps
    # publication in flight while the ordered reorg authority is exercised.
    scenarios.append("reorg_in_flight,2000,0,8,4")
    return scenarios


def run_lane(phase: str, manifest_path: Path, output_path: Path) -> dict[str, object]:
    manifest = json.loads(manifest_path.read_text())
    status = manifest.get("convergence_status", {})
    if status.get("state") != "acceptance":
        raise ValueError("Acceptance lane requires the Acceptance state")
    universe_ref = status.get("acceptance_universe")
    if not isinstance(universe_ref, dict) or set(universe_ref) != {"path", "sha256"}:
        raise ValueError("Acceptance lane requires one frozen universe")
    universe_path = REPO_ROOT / universe_ref["path"]
    if file_sha256(universe_path) != universe_ref["sha256"]:
        raise ValueError("Acceptance universe raw hash differs")
    universe = json.loads(universe_path.read_text())
    production = universe.get("categories", {}).get("production_sources", {})
    checkpoint = production.get("checkpoint")
    tree = production.get("tree")
    if not isinstance(checkpoint, str) or not isinstance(tree, str):
        raise ValueError("Acceptance universe has no frozen product subject")
    submodule_sources = production.get("recursive_submodules")
    plan = phase_command_plan(phase)
    completed = set(status.get("completed_phases", []))
    required = {
        "complete_correctness": {"evidence_universe_freeze"},
        "deterministic_smoke": {"evidence_universe_freeze"},
        "complete_mutation": {"evidence_universe_freeze"},
        "empirical_performance_acceptance": {
            "evidence_universe_freeze",
            "complete_correctness",
        },
        "portability_and_final_review": {
            "complete_correctness",
            "deterministic_smoke",
            "complete_mutation",
            "empirical_performance_acceptance",
        },
    }[phase]
    contract = json.loads((REPO_ROOT / "tx-pool/architecture-contract.json").read_text())
    phase_dependencies = {
        row["id"]: set(row["requires"])
        for row in contract["convergence_protocol"]["phase_dag"]
    }
    if required != phase_dependencies.get(phase):
        raise ValueError(f"Acceptance runner dependency projection differs for {phase}")
    missing = required - completed
    if missing:
        raise ValueError(f"Acceptance lane lacks predecessors {sorted(missing)}")
    if phase in completed:
        raise ValueError(f"Acceptance lane {phase} is already complete")
    prerequisite_phases = required.intersection(ACCEPTANCE_PHASES)
    evidence = status.get("acceptance_evidence")
    if not isinstance(evidence, dict) or set(evidence) != completed.intersection(
        ACCEPTANCE_PHASES
    ):
        raise ValueError(
            "Acceptance evidence does not match the completed phase projection"
        )
    for prerequisite in prerequisite_phases:
        reference = evidence.get(prerequisite)
        if not isinstance(reference, dict) or not isinstance(
            reference.get("evidence_sha256"), str
        ):
            raise ValueError(
                f"Acceptance lane lacks bound predecessor evidence {prerequisite}"
            )
    runner_path = Path(__file__).resolve()
    tool_sources = universe.get("categories", {}).get("tool_semantics", {}).get(
        "source_sha256", {}
    )
    runner_relative = str(runner_path.relative_to(REPO_ROOT))
    runner_sha256 = file_sha256(runner_path)
    if tool_sources.get(runner_relative) != runner_sha256:
        raise ValueError("Acceptance runner is not bound into the frozen universe")

    environment = _effective_environment(checkpoint, phase)
    command_results: list[dict[str, object]] = []
    artifact_results: list[dict[str, str]] = []
    observations: dict[str, object] = {}
    checkout_prefix = (
        "ckb-txpool-measure-a-"
        if phase == "empirical_performance_acceptance"
        else "ckb-txpool-acceptance-"
    )
    with _detached_checkpoint(
        checkpoint, tree, submodule_sources, prefix=checkout_prefix
    ) as (checkout, submodule_source_set_sha256):
        _validate_tool_sources(checkout, tool_sources)
        joined_originals: dict[Path, bytes | None] = {}
        joined_payloads: dict[Path, bytes] = {}
        joined_status = ""
        if phase == "portability_and_final_review":
            for relative, payload in sorted(
                _joined_acceptance_inputs(manifest_path, manifest).items()
            ):
                repository_path = _repository_file(relative)
                destination = checkout / repository_path.relative_to(REPO_ROOT.resolve())
                joined_originals[destination] = (
                    destination.read_bytes() if destination.is_file() else None
                )
                joined_payloads[destination] = payload
                destination.parent.mkdir(parents=True, exist_ok=True)
                destination.write_bytes(payload)
            joined_status = _clean_status(checkout)
        executable_plan = plan
        if phase == "complete_mutation":
            mutation_dir = checkout.parent / "mutation"
            mutation_dir.mkdir()
            config = mutation_dir / "mutants.toml"
            outcomes = mutation_dir / "outcomes"
            final_adjudicator_log = mutation_dir / "final-adjudicator.log"
            executable_plan = [
                {
                    "argv": [
                        "python3",
                        "tx-pool/scripts/check_mutation_matrix.py",
                        "--rediscover",
                        "--write-lock",
                        "--write-config",
                        str(config),
                    ],
                    "cwd": ".",
                }
            ]
        # Resources referenced by a command must outlive the command itself.
        # In particular, the paired performance worktree cannot be retired
        # after argv construction and before cross_version_benchmark opens it.
        with ExitStack() as command_resources:
            if phase == "empirical_performance_acceptance":
                if "complete_correctness" not in completed:
                    raise ValueError(
                        "performance Acceptance requires the completed correctness oracle"
                    )
                performance_directory = checkout.parent / "performance"
                performance_directory.mkdir()
                performance_output = performance_directory / "performance.json"
                comparison, comparison_submodule_source_set_sha256 = (
                    command_resources.enter_context(
                        _detached_checkpoint(
                            checkpoint,
                            tree,
                            submodule_sources,
                            prefix="ckb-txpool-measure-b-",
                        )
                    )
                )
                if comparison_submodule_source_set_sha256 != submodule_source_set_sha256:
                    raise ValueError("paired frozen submodule source closures differ")
                if len(str(checkout).encode()) != len(str(comparison).encode()):
                    raise ValueError("paired frozen source roots have unequal path lengths")
                target_a = performance_directory / "target-a"
                target_b = performance_directory / "target-b"
                argv = [
                    "python3",
                    "tx-pool/scripts/cross_version_benchmark.py",
                    "--baseline-root",
                    str(checkout),
                    "--candidate-root",
                    str(comparison),
                    "--baseline-target-dir",
                    str(target_a),
                    "--candidate-target-dir",
                    str(target_b),
                    "--baseline-build-features",
                    "internal,profiling",
                    "--candidate-build-features",
                    "internal,profiling",
                    "--output",
                    str(performance_output),
                    "--runs",
                    "6",
                    "--replicates-per-sample",
                    "1",
                    "--initial-cooldown-seconds",
                    "15",
                    "--cooldown-seconds",
                    "2",
                    "--max-paired-mad-percent",
                    "1.5",
                    "--timeout-seconds",
                    "300",
                ]
                for scenario in _performance_scenarios(checkout):
                    argv.extend(("--scenario", scenario))
                executable_plan = [{"argv": argv, "cwd": "."}]
            for command in executable_plan:
                argv = command["argv"]
                assert isinstance(argv, list) and all(
                    isinstance(item, str) for item in argv
                )
                print(f"\n[acceptance:{phase}] {' '.join(argv)}", flush=True)
                exit_code, output_sha256, output_bytes = _stream_command(
                    argv,
                    sys.stdout.buffer,
                    cwd=checkout,
                    environment=environment,
                )
                command_results.append(
                    {
                        "argv": plan[len(command_results)]["argv"],
                        "cwd": command["cwd"],
                        "exit_code": exit_code,
                        "output_sha256": output_sha256,
                        "output_bytes": output_bytes,
                    }
                )
                if exit_code != 0:
                    break
        if phase == "complete_mutation" and command_results[-1]["exit_code"] == 0:
            lock = json.loads((checkout / MUTATION_ARTIFACTS[0]).read_text())
            template = lock["execution"]["command_template"]
            mutation_command = [
                str(config) if item == "<CONFIG>" else str(outcomes) if item == "<OUTPUT>" else item
                for item in template
            ]
            _validate_mutation_worker_isolation(mutation_command, environment)
            for command_index, argv in enumerate((
                [
                    "cargo",
                    "nextest",
                    "run",
                    "--no-run",
                    "-p",
                    "ckb-tx-pool",
                    "--features",
                    "internal",
                    "--lib",
                ],
                mutation_command,
                [
                    "python3",
                    "tx-pool/scripts/check_mutation_matrix.py",
                    "--verify-outcomes",
                    str(outcomes),
                    "--write-result-lock",
                    "--require-accepted",
                ],
            )):
                print(f"\n[acceptance:{phase}] {' '.join(argv)}", flush=True)
                exit_code, output_sha256, output_bytes = _stream_command(
                    argv,
                    sys.stdout.buffer,
                    cwd=checkout,
                    environment=environment,
                    capture_path=(final_adjudicator_log if command_index == 2 else None),
                )
                command_results.append(
                    {
                        # The result binds the policy-owned abstract command plan;
                        # transient paths and the expanded lock-owned mutant
                        # selection are execution details verified by the
                        # generated lock/result artifacts.
                        "argv": plan[len(command_results)]["argv"],
                        "cwd": ".",
                        "exit_code": exit_code,
                        "output_sha256": output_sha256,
                        "output_bytes": output_bytes,
                    }
                )
                # cargo-mutants reports missed rows with a non-zero exit.  The
                # following frozen verifier owns their equivalence disposition,
                # so only a prewarm or final-verifier failure terminates this
                # lane.
                if exit_code != 0 and command_index != 1:
                    break
            if len(command_results) == 4:
                for relative in MUTATION_ARTIFACTS:
                    if (checkout / relative).is_file():
                        artifact_results.append(
                            _copy_artifact(checkout, output_path, relative)
                        )
                if phase_failed_diagnostic_is_required(phase, command_results):
                    artifact_results.append(
                        _stage_artifact(
                            final_adjudicator_log,
                            output_path,
                            MUTATION_DIAGNOSTIC_ARTIFACT,
                        )
                    )
            if (checkout / MUTATION_ARTIFACTS[1]).is_file():
                result_lock = json.loads((checkout / MUTATION_ARTIFACTS[1]).read_text())
                unaccepted_candidates = [
                    row["candidate"]
                    for row in result_lock["rows"]
                    if row["disposition"]["kind"] == "unaccepted"
                ]
                observations = {
                    "candidate_count": result_lock["candidate_count"],
                    "counts": result_lock["counts"],
                    "disposition_counts": result_lock["disposition_counts"],
                    "accepted": result_lock["accepted"],
                    "unaccepted_candidates": unaccepted_candidates,
                }
        if (
            phase == "empirical_performance_acceptance"
            and command_results[-1]["exit_code"] == 0
        ):
            performance = json.loads(performance_output.read_text())
            summary = performance.get("summary", {})
            scenario_summaries = {
                key: value for key, value in summary.items() if key != "aggregate"
            }
            expected_count = len(_performance_scenarios(checkout))
            if (
                len(scenario_summaries) != expected_count
                or any(value.get("status") != "comparable" for value in scenario_summaries.values())
                or performance.get("failures")
                or performance.get("runs") != 6
                or performance.get("max_paired_mad_percent") != 1.5
                or performance.get("baseline_binary", {}).get("sha256")
                != performance.get("candidate_binary", {}).get("sha256")
            ):
                raise ValueError("fixed-X2 performance artifact did not close its declared matrix")
            artifact_results.append(
                _stage_artifact(performance_output, output_path, PERFORMANCE_ARTIFACT)
            )
            observations = {
                "scenario_count": expected_count,
                "runs_per_scenario": 6,
                "binary_sha256": performance["candidate_binary"]["sha256"],
                "matrix_status": "comparable",
                "aggregate": summary["aggregate"],
                "required_observations": [
                    "negative_throughput",
                    "p99_end_to_end_latency",
                    "cpu_time_per_transaction",
                    "peak_rss_and_allocation_rate",
                    "authority_lock_wait_and_hold",
                    "shutdown_and_reorg_interference_latency",
                ],
                "reorg_in_flight_scenario": next(
                    value
                    for key, value in scenario_summaries.items()
                    if key.startswith("reorg_in_flight-")
                ),
            }
        final_status = _clean_status(checkout)
        if phase == "portability_and_final_review":
            changed_joined_inputs = [
                destination.relative_to(checkout).as_posix()
                for destination, expected in joined_payloads.items()
                if not destination.is_file() or destination.read_bytes() != expected
            ]
            if changed_joined_inputs:
                raise ValueError(
                    "portability commands changed joined Acceptance bytes: "
                    + repr(sorted(changed_joined_inputs))
                )
            if final_status != joined_status:
                raise ValueError(
                    "portability commands changed the joined Acceptance projection:\n"
                    + final_status
                )
            for destination, original in joined_originals.items():
                if original is None:
                    destination.unlink(missing_ok=True)
                else:
                    destination.write_bytes(original)
            final_status = _clean_status(checkout)
            if final_status:
                raise ValueError(
                    "portability checkout did not restore its exact product tree:\n"
                    + final_status
                )
        observed_paths = {
            line[3:].rsplit(" -> ", 1)[-1]
            for line in final_status.splitlines()
            if len(line) >= 4
        }
        allowed_paths = set(MUTATION_ARTIFACTS) if phase == "complete_mutation" else set()
        unexpected_paths = observed_paths - allowed_paths
        commands_passed = phase_commands_passed(phase, command_results)
        missing_paths = allowed_paths - observed_paths if commands_passed else set()
        if unexpected_paths or missing_paths:
            details = []
            if unexpected_paths:
                details.append(f"unexpected={sorted(unexpected_paths)}")
            if missing_paths:
                details.append(f"missing={sorted(missing_paths)}")
            raise ValueError(
                "Acceptance commands changed the exact checkout: "
                + "; ".join(details)
                + (f"\n{final_status}" if final_status else "")
            )
        final_status = ""

    record: dict[str, object] = {
        "schema_version": 2,
        "phase": phase,
        "acceptance_universe": universe_ref,
        "subject": {"checkpoint": checkpoint, "tree": tree},
        "runner_sha256": runner_sha256,
        "command_plan_sha256": canonical_json_sha256(plan),
        "environment": {key: environment.get(key) for key in ENVIRONMENT_KEYS},
        "execution": {
            "kind": "detached_exact_checkpoint",
            "tree": tree,
            "clean_status_sha256": hashlib.sha256(final_status.encode()).hexdigest(),
            "submodule_source_set_sha256": submodule_source_set_sha256,
            "tool_source_set_sha256": canonical_json_sha256(tool_sources),
        },
        "prerequisite_evidence_sha256": (
            {
                predecessor: status["acceptance_evidence"][predecessor][
                    "evidence_sha256"
                ]
                for predecessor in sorted(prerequisite_phases)
            }
        ),
        "commands": command_results,
        "artifacts": sorted(artifact_results, key=lambda item: item["path"]),
        "observations": observations,
        "outcome": "passed" if phase_commands_passed(phase, command_results) else "failed",
    }
    record["evidence_sha256"] = canonical_json_sha256(record)
    errors = validate_result_shape(record)
    for artifact in record["artifacts"]:
        path = _bundle_path(output_path, artifact["path"])
        try:
            observed_sha256 = file_sha256(path)
        except OSError as error:
            errors.append(f"cannot read generated Acceptance artifact: {error}")
        else:
            if observed_sha256 != artifact["sha256"]:
                errors.append(
                    f"generated Acceptance artifact hash differs: {artifact['path']}"
                )
    if errors:
        raise ValueError("generated invalid Acceptance result: " + "; ".join(errors))
    return record


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--phase", required=True, choices=ACCEPTANCE_PHASES)
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument("--output", type=Path, required=True)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        record = run_lane(args.phase, args.manifest, args.output)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(record, indent=2, ensure_ascii=True) + "\n")
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    print(
        f"wrote {record['outcome']} Acceptance result for {args.phase}: {args.output}"
    )
    return 0 if record["outcome"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
