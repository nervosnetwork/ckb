#!/usr/bin/env python3
"""Capture and deterministically analyze a tx-pool Samply profile.

The measured window is emitted by the production benchmark fixture immediately
before submission and after the stable pending callback.  The analyzer crops
the whole-process profile to that window, so fixture construction, warm-up and
teardown cannot be mistaken for target-work attribution.
"""

from __future__ import annotations

import argparse
import bisect
import datetime
import gzip
import hashlib
import json
import os
import platform
import re
import shlex
import subprocess
import sys
from collections import Counter
from pathlib import Path
from typing import Any


WORKSPACE_ROOT = Path(__file__).resolve().parents[2]
BENCHMARK_SOURCE = WORKSPACE_ROOT / "tx-pool" / "src" / "benchmark.rs"
ONE_SHOT_SOURCE = WORKSPACE_ROOT / "tx-pool" / "benches" / "profile_one_shot.rs"
SCRIPT_SOURCE = Path(__file__).resolve()
REMAPPED_SOURCE_ROOT = "/ckb-txpool-profile-source"
MARKER_PREFIX = "TX_POOL_PROFILE_WINDOW "
OBSERVATION_PREFIX = "TX_POOL_PROFILE_OBSERVATION "
PIPELINE_FEATURES = ("internal", "profiling")
ONE_SHOT_FEATURES = ("profiling",)
PROFILE_SCHEMA_VERSION = 1
OBSERVATION_SCHEMA_VERSION = 2
MANIFEST_SCHEMA_VERSION = 4
FINAL_BUILD_PROFILE = "prod"
SUMMARY_SCHEMA_VERSION = 3


class ProfileError(RuntimeError):
    """A reproducibility or profile-format contract was violated."""


def positive_integer(value: str) -> int:
    parsed = int(value)
    if parsed <= 0:
        raise argparse.ArgumentTypeError("must be greater than zero")
    return parsed


def nonnegative_integer(value: str) -> int:
    parsed = int(value)
    if parsed < 0:
        raise argparse.ArgumentTypeError("cannot be negative")
    return parsed


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="action", required=True)

    capture = subparsers.add_parser(
        "capture", help="capture one fixed tx-pool profile and analyze it"
    )
    capture.add_argument(
        "--output-prefix",
        type=Path,
        required=True,
        help="artifact prefix outside the source tree, for example /tmp/txpool-cold",
    )
    capture.add_argument(
        "--binary",
        type=Path,
        help="reuse an existing internal+profiling pipeline benchmark binary",
    )
    capture.add_argument(
        "--binary-profile",
        help="required prod-profile attestation when --binary is supplied",
    )
    capture.add_argument(
        "--target-dir",
        type=Path,
        default=WORKSPACE_ROOT / "target" / "tx-pool-profile",
        help="Cargo target directory used only when --binary is omitted",
    )
    capture.add_argument("--rate", type=positive_integer, default=1000)
    capture.add_argument(
        "--tx-type",
        choices=(
            "always_success",
            "secp256k1",
            "dependent_always_success",
            "dependent_secp",
        ),
        required=True,
    )
    capture.add_argument("--pool-state", choices=("cold", "warm"), required=True)
    capture.add_argument(
        "--dependency-order",
        choices=("parent_first", "child_first"),
        default="parent_first",
    )
    capture.add_argument("--peers", type=positive_integer, required=True)
    capture.add_argument("--workers", type=positive_integer, required=True)
    capture.add_argument("--size", type=positive_integer, required=True)
    capture.add_argument("--warm-pool-size", type=nonnegative_integer, default=0)
    capture.add_argument(
        "--force", action="store_true", help="replace an existing artifact set"
    )

    one_shot = subparsers.add_parser(
        "capture-one-shot",
        help="capture one production-shaped one-shot workload and analyze it",
    )
    one_shot.add_argument(
        "--output-prefix",
        type=Path,
        required=True,
        help="artifact prefix outside the source tree, for example /tmp/txpool-fanout",
    )
    one_shot.add_argument(
        "--binary",
        type=Path,
        help="reuse an existing profiling-enabled profile_one_shot binary",
    )
    one_shot.add_argument(
        "--binary-profile",
        help="required prod-profile attestation when --binary is supplied",
    )
    one_shot.add_argument(
        "--target-dir",
        type=Path,
        default=WORKSPACE_ROOT / "target" / "tx-pool-profile-one-shot",
        help="Cargo target directory used only when --binary is omitted",
    )
    one_shot.add_argument("--rate", type=positive_integer, default=1000)
    one_shot.add_argument("--scenario", required=True)
    one_shot.add_argument("--target", type=positive_integer, required=True)
    one_shot.add_argument("--warm", type=nonnegative_integer, required=True)
    one_shot.add_argument("--workers", type=positive_integer, required=True)
    one_shot.add_argument("--peers", type=positive_integer, required=True)
    one_shot.add_argument(
        "--force", action="store_true", help="replace an existing artifact set"
    )

    analyze = subparsers.add_parser(
        "analyze", help="verify a manifest and regenerate its deterministic summary"
    )
    analyze.add_argument("--manifest", type=Path, required=True)
    args = parser.parse_args()
    if args.action in {"capture", "capture-one-shot"}:
        if args.binary is None and args.binary_profile is not None:
            parser.error("--binary-profile requires --binary")
        if args.binary is not None:
            try:
                require_final_build_profile(args.binary_profile)
            except ValueError as error:
                parser.error(str(error))
    return args


def require_final_build_profile(profile: str | None) -> str:
    if profile != FINAL_BUILD_PROFILE:
        raise ValueError(
            "reused profile binaries require an explicit prod build-profile attestation"
        )
    return profile


def run_text(command: list[str], *, env: dict[str, str] | None = None) -> str:
    try:
        completed = subprocess.run(
            command,
            cwd=WORKSPACE_ROOT,
            env=env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=True,
        )
    except OSError as error:
        raise ProfileError(f"cannot execute {shlex.join(command)}: {error}") from error
    except subprocess.CalledProcessError as error:
        detail = (error.stderr or error.stdout or "no process output").strip()
        raise ProfileError(
            f"command failed ({error.returncode}): {shlex.join(command)}\n{detail}"
        ) from error
    output = completed.stdout.strip()
    if not output:
        raise ProfileError(f"command produced no identity output: {shlex.join(command)}")
    return output


def sha256_bytes(content: bytes) -> str:
    return hashlib.sha256(content).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    try:
        with path.open("rb") as source:
            for chunk in iter(lambda: source.read(1024 * 1024), b""):
                digest.update(chunk)
    except OSError as error:
        raise ProfileError(f"cannot hash {path}: {error}") from error
    return digest.hexdigest()


def files_sha256(paths: list[Path]) -> str:
    digest = hashlib.sha256()
    for path in paths:
        relative = path.relative_to(WORKSPACE_ROOT).as_posix()
        digest.update(relative.encode())
        digest.update(b"\0")
        digest.update(path.read_bytes())
        digest.update(b"\0")
    return digest.hexdigest()


def input_file(path: Path) -> dict[str, Any]:
    resolved = path.resolve(strict=True)
    return {
        "path_at_capture": str(resolved),
        "size_bytes": resolved.stat().st_size,
        "sha256": sha256_file(resolved),
    }


def artifact(path: Path, bundle_dir: Path) -> dict[str, Any]:
    resolved = path.resolve(strict=True)
    try:
        relative = resolved.relative_to(bundle_dir.resolve(strict=True))
    except ValueError as error:
        raise ProfileError(f"profile artifact is outside its bundle: {resolved}") from error
    return {
        "path": relative.as_posix(),
        "size_bytes": resolved.stat().st_size,
        "sha256": sha256_file(resolved),
    }


def verify_artifact(record: dict[str, Any], label: str, bundle_dir: Path) -> Path:
    required = {"path", "size_bytes", "sha256"}
    if set(record) != required:
        raise ProfileError(f"{label} artifact has an unsupported schema")
    relative = Path(record["path"])
    if relative.is_absolute() or ".." in relative.parts:
        raise ProfileError(f"{label} artifact path is not bundle-relative")
    path = (bundle_dir / relative).resolve()
    try:
        path.relative_to(bundle_dir.resolve())
    except ValueError as error:
        raise ProfileError(f"{label} artifact escapes its bundle") from error
    if not path.is_file():
        raise ProfileError(f"{label} artifact is missing: {path}")
    if path.stat().st_size != record["size_bytes"]:
        raise ProfileError(f"{label} artifact size changed: {path}")
    if sha256_file(path) != record["sha256"]:
        raise ProfileError(f"{label} artifact hash changed: {path}")
    return path


def read_json(path: Path) -> dict[str, Any]:
    try:
        if path.suffix == ".gz":
            with gzip.open(path, "rt", encoding="utf-8") as source:
                value = json.load(source)
        else:
            with path.open(encoding="utf-8") as source:
                value = json.load(source)
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise ProfileError(f"cannot read JSON artifact {path}: {error}") from error
    if not isinstance(value, dict):
        raise ProfileError(f"JSON artifact is not an object: {path}")
    return value


def write_json(path: Path, value: dict[str, Any]) -> None:
    try:
        path.write_text(
            json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False) + "\n"
        )
    except OSError as error:
        raise ProfileError(f"cannot write {path}: {error}") from error


def output_paths(prefix: Path) -> dict[str, Path]:
    absolute = prefix.expanduser().resolve()
    try:
        absolute.relative_to(WORKSPACE_ROOT)
    except ValueError:
        pass
    else:
        raise ProfileError("profile artifacts must be stored outside the source tree")
    return {
        "profile": Path(f"{absolute}.json.gz"),
        "symbols": Path(f"{absolute}.json.syms.json"),
        "stdout": Path(f"{absolute}.stdout.log"),
        "stderr": Path(f"{absolute}.stderr.log"),
        "spans": Path(f"{absolute}.spans.json"),
        "span_stdout": Path(f"{absolute}.span.stdout.log"),
        "span_stderr": Path(f"{absolute}.span.stderr.log"),
        "manifest": Path(f"{absolute}.manifest.json"),
        "summary": Path(f"{absolute}.summary.json"),
    }


def prepare_outputs(paths: dict[str, Path], force: bool) -> None:
    existing = [path for path in paths.values() if path.exists()]
    if existing and not force:
        joined = ", ".join(str(path) for path in existing)
        raise ProfileError(f"refusing to overwrite existing artifacts: {joined}")
    for path in existing:
        if not path.is_file():
            raise ProfileError(f"refusing to replace a non-file artifact: {path}")
        path.unlink()
    paths["profile"].parent.mkdir(parents=True, exist_ok=True)


def build_environment(target_dir: Path) -> dict[str, str]:
    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = str(target_dir.expanduser().resolve())
    env["CARGO_INCREMENTAL"] = "0"
    remap = f"--remap-path-prefix={WORKSPACE_ROOT}={REMAPPED_SOURCE_ROOT}"
    encoded = env.get("CARGO_ENCODED_RUSTFLAGS")
    if encoded:
        env["CARGO_ENCODED_RUSTFLAGS"] = f"{encoded}\x1f{remap}"
    else:
        env["RUSTFLAGS"] = f"{env.get('RUSTFLAGS', '')} {remap}".strip()
    return env


def build_binary(
    target_dir: Path, bench_name: str, features: tuple[str, ...]
) -> tuple[Path, list[str], dict[str, str]]:
    command = [
        "cargo",
        "bench",
        "-p",
        "ckb-tx-pool",
        "--features",
        ",".join(features),
        "--bench",
        bench_name,
        "--no-run",
        "--locked",
        "--profile",
        FINAL_BUILD_PROFILE,
        "--message-format",
        "json",
    ]
    env = build_environment(target_dir)
    try:
        completed = subprocess.run(
            command,
            cwd=WORKSPACE_ROOT,
            env=env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=True,
        )
    except OSError as error:
        raise ProfileError(f"cannot execute Cargo: {error}") from error
    except subprocess.CalledProcessError as error:
        raise ProfileError(
            f"profile binary build failed ({error.returncode}):\n{error.stderr.strip()}"
        ) from error

    executables: list[Path] = []
    for line in completed.stdout.splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        target = message.get("target", {})
        executable = message.get("executable")
        if (
            message.get("reason") == "compiler-artifact"
            and target.get("name") == bench_name
            and "bench" in target.get("kind", [])
            and executable
        ):
            executables.append(Path(executable).resolve())
    unique = sorted(set(executables))
    if len(unique) != 1 or not unique[0].is_file():
        raise ProfileError(
            f"Cargo reported {len(unique)} {bench_name} benchmark executables; expected one"
        )
    return unique[0], command, env


def sanitized_runtime_environment() -> dict[str, str]:
    env = os.environ.copy()
    for name in (
        "QUICK_BENCH",
        "FULL_BENCH",
        "TX_POOL_BENCH_PREFLIGHT",
        "TX_POOL_PROFILE_TX_TYPE",
        "TX_POOL_PROFILE_POOL_STATE",
        "TX_POOL_PROFILE_DEPENDENCY_ORDER",
        "TX_POOL_PROFILE_PEERS",
        "TX_POOL_PROFILE_WORKERS",
        "TX_POOL_PROFILE_SIZE",
        "TX_POOL_PROFILE_WARM_POOL_SIZE",
        "TX_POOL_PROFILE_TRACE_PATH",
    ):
        env.pop(name, None)
    return env


def scenario_environment(args: argparse.Namespace) -> dict[str, str]:
    dependent = args.tx_type.startswith("dependent_")
    if args.dependency_order == "child_first" and not dependent:
        raise ProfileError("child_first order requires a dependent transaction type")
    env = sanitized_runtime_environment()
    env.update(
        {
            "TX_POOL_PROFILE_TX_TYPE": args.tx_type,
            "TX_POOL_PROFILE_POOL_STATE": args.pool_state,
            "TX_POOL_PROFILE_DEPENDENCY_ORDER": args.dependency_order,
            "TX_POOL_PROFILE_PEERS": str(args.peers),
            "TX_POOL_PROFILE_WORKERS": str(args.workers),
            "TX_POOL_PROFILE_SIZE": str(args.size),
            "TX_POOL_PROFILE_WARM_POOL_SIZE": str(args.warm_pool_size),
        }
    )
    return env


def parse_marker(stdout: str) -> dict[str, Any]:
    records = [
        line.removeprefix(MARKER_PREFIX)
        for line in stdout.splitlines()
        if line.startswith(MARKER_PREFIX)
    ]
    if len(records) != 1:
        raise ProfileError(f"expected exactly one profile window marker, found {len(records)}")
    try:
        marker = json.loads(records[0])
    except json.JSONDecodeError as error:
        raise ProfileError(f"profile window marker is invalid JSON: {error}") from error
    required = {
        "schema_version",
        "scenario",
        "start_unix_nanos",
        "end_unix_nanos",
        "elapsed_nanos",
    }
    if not isinstance(marker, dict) or set(marker) != required:
        raise ProfileError("profile window marker has an unsupported schema")
    start = marker["start_unix_nanos"]
    end = marker["end_unix_nanos"]
    elapsed = marker["elapsed_nanos"]
    if marker["schema_version"] != PROFILE_SCHEMA_VERSION:
        raise ProfileError("profile window schema version is unsupported")
    if not all(isinstance(value, int) for value in (start, end, elapsed)):
        raise ProfileError("profile window timestamps must be integers")
    if start >= end or end - start != elapsed:
        raise ProfileError("profile window timestamps are inconsistent")
    if not isinstance(marker["scenario"], str) or not marker["scenario"]:
        raise ProfileError("profile scenario name is empty")
    return marker


def parse_observation(stdout: str, expected: dict[str, Any]) -> dict[str, Any]:
    records = [
        line.removeprefix(OBSERVATION_PREFIX)
        for line in stdout.splitlines()
        if line.startswith(OBSERVATION_PREFIX)
    ]
    if len(records) != 1:
        raise ProfileError(
            f"expected exactly one profile observation, found {len(records)}"
        )
    try:
        observation = json.loads(records[0])
    except json.JSONDecodeError as error:
        raise ProfileError(f"profile observation is invalid JSON: {error}") from error
    required = {
        "schema_version",
        "scenario",
        "target",
        "warm",
        "workers",
        "peers",
        "elapsed_nanos",
        "throughput_tps",
        "accepted",
        "p99_latency_nanos",
        "target_cpu_nanos",
        "target_user_cpu_nanos",
        "target_system_cpu_nanos",
        "allocation_calls",
        "allocated_bytes",
        "reorg_latency_nanos",
        "reorg_overlap_callbacks",
        "shutdown_latency_nanos",
    }
    terminal_fields = {
        "callback_duplicates",
        "relay_ok",
        "relay_duplicate_ok",
        "relay_rejects",
        "relay_unknown_parents",
        "relay_generation_resets",
    }
    terminal_observation_fields = terminal_fields | {
        "relay_unknown_parent_observations"
    }
    if not isinstance(observation, dict):
        raise ProfileError("profile observation has an unsupported schema")
    schema_version = observation.get("schema_version")
    if schema_version == 1:
        expected_fields = required
    elif schema_version == OBSERVATION_SCHEMA_VERSION:
        expected_fields = required | terminal_observation_fields
    else:
        raise ProfileError("profile observation schema version is unsupported")
    if set(observation) != expected_fields:
        raise ProfileError("profile observation has an unsupported schema")
    identity = {
        name: observation[name]
        for name in ("scenario", "target", "warm", "workers", "peers")
    }
    if identity != expected:
        raise ProfileError(f"profile observation drifted: {identity} != {expected}")
    integer_metrics = (
        "elapsed_nanos",
        "accepted",
        "p99_latency_nanos",
        "target_cpu_nanos",
        "target_user_cpu_nanos",
        "target_system_cpu_nanos",
        "allocation_calls",
        "allocated_bytes",
        "reorg_latency_nanos",
        "reorg_overlap_callbacks",
        "shutdown_latency_nanos",
    )
    if schema_version == OBSERVATION_SCHEMA_VERSION:
        integer_metrics += tuple(sorted(terminal_fields))
    if any(
        not isinstance(observation[name], int)
        or isinstance(observation[name], bool)
        or observation[name] < 0
        for name in integer_metrics
    ):
        raise ProfileError("profile observation has an invalid integer metric")
    positive_metrics = (
        "elapsed_nanos",
        "p99_latency_nanos",
        "target_cpu_nanos",
        "reorg_latency_nanos",
        "shutdown_latency_nanos",
    )
    if any(observation[name] <= 0 for name in positive_metrics):
        raise ProfileError("profile observation has an empty target metric")
    if (
        observation["target_user_cpu_nanos"]
        + observation["target_system_cpu_nanos"]
        != observation["target_cpu_nanos"]
    ):
        raise ProfileError("profile observation CPU components do not sum to total")
    if observation["accepted"] != expected["target"] + expected["warm"]:
        raise ProfileError("profile observation did not accept the exact workload")
    throughput = observation["throughput_tps"]
    if (
        not isinstance(throughput, (int, float))
        or isinstance(throughput, bool)
        or throughput <= 0
    ):
        raise ProfileError("profile observation throughput is invalid")
    expected_overlap = expected["scenario"] == "reorg_in_flight"
    if (observation["reorg_overlap_callbacks"] > 0) != expected_overlap:
        raise ProfileError("profile observation reorg overlap differs from the scenario")
    if schema_version == OBSERVATION_SCHEMA_VERSION:
        exact_accepted = expected["target"] + expected["warm"]
        unknown_parent_observations = observation[
            "relay_unknown_parent_observations"
        ]
        if not isinstance(unknown_parent_observations, list):
            raise ProfileError("profile observation has invalid unknown-parent evidence")
        observed_unknown_parent_count = 0
        for item in unknown_parent_observations:
            if not isinstance(item, dict) or set(item) != {"peer", "parents", "count"}:
                raise ProfileError(
                    "profile observation has invalid unknown-parent evidence"
                )
            peer = item["peer"]
            parents = item["parents"]
            count = item["count"]
            if (
                not isinstance(peer, int)
                or isinstance(peer, bool)
                or peer < 0
                or not isinstance(count, int)
                or isinstance(count, bool)
                or count <= 0
                or not isinstance(parents, list)
                or not parents
                or any(not isinstance(parent, str) or not parent for parent in parents)
            ):
                raise ProfileError(
                    "profile observation has invalid unknown-parent evidence"
                )
            observed_unknown_parent_count += count
        if observed_unknown_parent_count != observation["relay_unknown_parents"]:
            raise ProfileError(
                "profile observation unknown-parent count does not match its evidence"
            )
        if observation["callback_duplicates"] != 0:
            raise ProfileError("profile observation contains duplicate callbacks")
        if observation["relay_ok"] != exact_accepted:
            raise ProfileError("profile observation did not relay the exact accepted workload")
        if observation["relay_duplicate_ok"] != 0:
            raise ProfileError("profile observation contains duplicate relay terminals")
        if (
            observation["relay_unknown_parents"] != 0
            and not expected["scenario"].endswith("_reverse")
        ):
            raise ProfileError("profile observation contains unknown-parent terminals")
        if observation["relay_generation_resets"] != 0:
            raise ProfileError("profile observation contains generation-reset terminals")
        expected_rejects = expected["warm"] if expected["scenario"] == "rbf_pairs" else 0
        if observation["relay_rejects"] != expected_rejects:
            raise ProfileError("profile observation contains an unexpected reject terminal set")
    return observation


def filesystem_type(path: Path) -> str:
    if sys.platform == "darwin":
        mounts = run_text(["mount"])
        resolved = path.resolve()
        matches: list[tuple[int, str]] = []
        for line in mounts.splitlines():
            match = re.match(r"^.+ on (.+) \(([^,)]+)", line)
            if match is None:
                continue
            mount_point = Path(match.group(1).replace("\\040", " "))
            try:
                resolved.relative_to(mount_point)
            except ValueError:
                continue
            matches.append((len(mount_point.parts), match.group(2)))
        if not matches:
            raise ProfileError(f"cannot determine filesystem type for {resolved}")
        return max(matches)[1]
    return run_text(["stat", "-f", "-c", "%T", str(path)])


def cpu_model() -> str:
    if sys.platform == "darwin":
        return run_text(["sysctl", "-n", "machdep.cpu.brand_string"])
    cpuinfo = Path("/proc/cpuinfo")
    if cpuinfo.is_file():
        for line in cpuinfo.read_text().splitlines():
            if line.lower().startswith("model name") and ":" in line:
                model = line.split(":", 1)[1].strip()
                if model:
                    return model
    processor = platform.processor().strip()
    if not processor:
        raise ProfileError("cannot determine the CPU model")
    return processor


def git_identity() -> dict[str, Any]:
    revision = run_text(["git", "rev-parse", "HEAD"])
    tracked_diff = subprocess.run(
        ["git", "diff", "--binary", "HEAD"],
        cwd=WORKSPACE_ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=True,
    ).stdout
    try:
        untracked_process = subprocess.run(
            ["git", "ls-files", "--others", "--exclude-standard", "--directory"],
            cwd=WORKSPACE_ROOT,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=True,
        )
    except (OSError, subprocess.CalledProcessError) as error:
        raise ProfileError(f"cannot enumerate untracked files: {error}") from error
    untracked_output = untracked_process.stdout.strip()
    untracked = [line for line in untracked_output.splitlines() if line]
    untracked_digest = hashlib.sha256()
    for relative in sorted(untracked):
        path = WORKSPACE_ROOT / relative
        untracked_digest.update(relative.encode())
        untracked_digest.update(b"\0")
        if path.is_file():
            untracked_digest.update(path.read_bytes())
        elif path.is_dir():
            for child in sorted(candidate for candidate in path.rglob("*") if candidate.is_file()):
                untracked_digest.update(child.relative_to(WORKSPACE_ROOT).as_posix().encode())
                untracked_digest.update(b"\0")
                untracked_digest.update(child.read_bytes())
        else:
            raise ProfileError(f"untracked path changed during identity capture: {path}")
        untracked_digest.update(b"\0")
    combined = hashlib.sha256()
    combined.update(tracked_diff)
    combined.update(b"\0")
    combined.update(untracked_digest.digest())
    return {
        "revision": revision,
        "tracked_diff_sha256": sha256_bytes(tracked_diff),
        "untracked_paths": sorted(untracked),
        "untracked_content_sha256": untracked_digest.hexdigest(),
        "working_tree_sha256": combined.hexdigest(),
    }


def environment_identity(output_dir: Path, effective_env: dict[str, str]) -> dict[str, Any]:
    cpu_count = os.cpu_count()
    machine = platform.machine().strip()
    platform_name = platform.platform().strip()
    if cpu_count is None or cpu_count <= 0 or not machine or not platform_name:
        raise ProfileError("host identity is incomplete")
    identity: dict[str, Any] = {
        "cargo": run_text(["cargo", "--version", "--verbose"]),
        "rustc": run_text(["rustc", "--version", "--verbose"]),
        "samply": run_text(["samply", "--version"]),
        "cpu_count": cpu_count,
        "cpu_model": cpu_model(),
        "machine": machine,
        "platform": platform_name,
        "source_root": str(WORKSPACE_ROOT),
        "source_root_utf8_bytes": len(str(WORKSPACE_ROOT).encode()),
        "artifact_filesystem": filesystem_type(output_dir),
        "rustflags": os.environ.get("RUSTFLAGS", ""),
        "cargo_encoded_rustflags": os.environ.get("CARGO_ENCODED_RUSTFLAGS", ""),
        "effective_rustflags": effective_env.get("RUSTFLAGS", ""),
        "effective_cargo_encoded_rustflags": effective_env.get(
            "CARGO_ENCODED_RUSTFLAGS", ""
        ),
    }
    if sys.platform == "darwin":
        identity["battery"] = run_text(["pmset", "-g", "batt"])
        identity["thermal"] = run_text(["pmset", "-g", "therm"])
    return identity


def capture(args: argparse.Namespace) -> Path:
    paths = output_paths(args.output_prefix)
    prepare_outputs(paths, args.force)
    if args.action == "capture":
        harness = "pipeline"
        features = PIPELINE_FEATURES
        sources = [BENCHMARK_SOURCE, SCRIPT_SOURCE]
        scenario = {
            "tx_type": args.tx_type,
            "pool_state": args.pool_state,
            "dependency_order": args.dependency_order,
            "peers": args.peers,
            "workers": args.workers,
            "size": args.size,
            "warm_pool_size": args.warm_pool_size,
        }
        runtime_env = scenario_environment(args)
        runtime_args = [
            "--bench",
            "--noplot",
            "--discard-baseline",
            "--color",
            "never",
        ]
        expected_observation = None
    elif args.action == "capture-one-shot":
        harness = "profile_one_shot"
        features = ONE_SHOT_FEATURES
        sources = [ONE_SHOT_SOURCE, SCRIPT_SOURCE]
        scenario = {
            "scenario": args.scenario,
            "target": args.target,
            "warm": args.warm,
            "workers": args.workers,
            "peers": args.peers,
        }
        runtime_env = sanitized_runtime_environment()
        runtime_args = [
            args.scenario,
            str(args.target),
            str(args.warm),
            str(args.workers),
            str(args.peers),
        ]
        expected_observation = scenario
    else:
        raise ProfileError(f"unsupported capture action: {args.action}")
    if args.binary is None:
        binary, build_command, build_env = build_binary(
            args.target_dir, harness, features
        )
    else:
        binary = args.binary.expanduser().resolve(strict=True)
        if not binary.is_file() or not os.access(binary, os.X_OK):
            raise ProfileError(f"profile binary is not executable: {binary}")
        build_command = []
        build_env = os.environ.copy()

    command = [
        "samply",
        "record",
        "--rate",
        str(args.rate),
        "--save-only",
        "--unstable-presymbolicate",
        "--output",
        str(paths["profile"]),
        str(binary),
        *runtime_args,
    ]
    started_utc = datetime.datetime.now(datetime.timezone.utc).isoformat()
    try:
        completed = subprocess.run(
            command,
            cwd=WORKSPACE_ROOT,
            env=runtime_env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=True,
        )
    except OSError as error:
        raise ProfileError(f"cannot execute Samply: {error}") from error
    except subprocess.CalledProcessError as error:
        detail = (error.stderr or error.stdout or "no process output").strip()
        raise ProfileError(f"Samply capture failed ({error.returncode}):\n{detail}") from error
    ended_utc = datetime.datetime.now(datetime.timezone.utc).isoformat()
    try:
        paths["stdout"].write_text(completed.stdout)
        paths["stderr"].write_text(completed.stderr)
    except OSError as error:
        raise ProfileError(f"cannot save profiler output: {error}") from error
    marker = parse_marker(completed.stdout)
    observation = (
        parse_observation(completed.stdout, expected_observation)
        if expected_observation is not None
        else None
    )
    if not paths["profile"].is_file() or not paths["symbols"].is_file():
        raise ProfileError("Samply capture did not produce profile and symbol artifacts")

    # Span counting is intentionally isolated from CPU sampling. The benchmark
    # subscriber uses fixed relaxed-atomic counters while the target is active
    # and writes one JSON artifact afterward; it performs no per-span format,
    # file-lock or I/O work.
    span_env = runtime_env.copy()
    span_env["TX_POOL_PROFILE_TRACE_PATH"] = str(paths["spans"])
    span_command = [str(binary), *runtime_args]
    span_started_utc = datetime.datetime.now(datetime.timezone.utc).isoformat()
    try:
        span_completed = subprocess.run(
            span_command,
            cwd=WORKSPACE_ROOT,
            env=span_env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=True,
        )
    except OSError as error:
        raise ProfileError(f"cannot execute span capture: {error}") from error
    except subprocess.CalledProcessError as error:
        detail = (error.stderr or error.stdout or "no process output").strip()
        raise ProfileError(f"span capture failed ({error.returncode}):\n{detail}") from error
    span_ended_utc = datetime.datetime.now(datetime.timezone.utc).isoformat()
    try:
        paths["span_stdout"].write_text(span_completed.stdout)
        paths["span_stderr"].write_text(span_completed.stderr)
    except OSError as error:
        raise ProfileError(f"cannot save span capture output: {error}") from error
    span_marker = parse_marker(span_completed.stdout)
    if expected_observation is not None:
        parse_observation(span_completed.stdout, expected_observation)
    if not paths["spans"].is_file():
        raise ProfileError("span capture did not produce its trace artifact")
    if span_marker["scenario"] != marker["scenario"]:
        raise ProfileError("CPU and span captures used different scenarios")

    manifest = {
        "schema_version": MANIFEST_SCHEMA_VERSION,
        "git": git_identity(),
        "harness": harness,
        "features": list(features),
        "scenario": scenario,
        "observation": observation,
        "window": marker,
        "capture": {
            "sample_rate_hz": args.rate,
            "started_utc": started_utc,
            "ended_utc": ended_utc,
            "command": command,
            "build_command": build_command,
            "binary_provenance": (
                "built_by_profile_runner" if build_command else "reused_by_sha256"
            ),
            "build_profile": FINAL_BUILD_PROFILE,
        },
        "span_capture": {
            "started_utc": span_started_utc,
            "ended_utc": span_ended_utc,
            "command": span_command,
            "window": span_marker,
            "isolation": (
                "separate execution from CPU sampling; in-memory span counters "
                "with one post-window JSON write"
            ),
        },
        "environment": environment_identity(paths["profile"].parent, build_env),
        "inputs": {
            "workspace_manifest_sha256": sha256_file(WORKSPACE_ROOT / "Cargo.toml"),
            "cargo_lock_sha256": sha256_file(WORKSPACE_ROOT / "Cargo.lock"),
            "tx_pool_manifest_sha256": sha256_file(
                WORKSPACE_ROOT / "tx-pool" / "Cargo.toml"
            ),
            "harness_sources": [
                source.relative_to(WORKSPACE_ROOT).as_posix() for source in sources
            ],
            "harness_sha256": files_sha256(sources),
            "binary": input_file(binary),
        },
        "artifacts": {
            "profile": artifact(paths["profile"], paths["manifest"].parent),
            "symbols": artifact(paths["symbols"], paths["manifest"].parent),
            "stdout": artifact(paths["stdout"], paths["manifest"].parent),
            "stderr": artifact(paths["stderr"], paths["manifest"].parent),
            "spans": artifact(paths["spans"], paths["manifest"].parent),
            "span_stdout": artifact(
                paths["span_stdout"], paths["manifest"].parent
            ),
            "span_stderr": artifact(
                paths["span_stderr"], paths["manifest"].parent
            ),
        },
        "summary_path": paths["summary"].name,
    }
    write_json(paths["manifest"], manifest)
    analyze_manifest(paths["manifest"])
    return paths["manifest"]


class SymbolResolver:
    def __init__(self, profile: dict[str, Any], sidecar: dict[str, Any]) -> None:
        self.profile_libs = profile.get("libs")
        self.string_table = sidecar.get("string_table")
        datasets = sidecar.get("data")
        if not isinstance(self.profile_libs, list):
            raise ProfileError("Samply profile has no library table")
        if not isinstance(self.string_table, list) or not isinstance(datasets, list):
            raise ProfileError("Samply symbol sidecar has an unsupported schema")
        self.datasets: dict[str, tuple[dict[int, int], list[int], list[dict[str, Any]]]] = {}
        for data in datasets:
            code_id = data.get("code_id")
            symbols = data.get("symbol_table")
            known = data.get("known_addresses")
            if not isinstance(code_id, str) or not isinstance(symbols, list) or not isinstance(known, list):
                raise ProfileError("Samply symbol dataset has an unsupported schema")
            exact = {pair[0]: pair[1] for pair in known if isinstance(pair, list) and len(pair) == 2}
            starts = [symbol["rva"] for symbol in symbols]
            if starts != sorted(starts):
                raise ProfileError(f"symbol table for {code_id} is not sorted")
            self.datasets[code_id] = (exact, starts, symbols)

    def frame_name(self, thread: dict[str, Any], frame_index: int) -> str:
        frame_table = thread["frameTable"]
        func_table = thread["funcTable"]
        resources = thread["resourceTable"]
        strings = thread["stringArray"]
        func_index = frame_table["func"][frame_index]
        name = strings[func_table["name"][func_index]]
        if not re.fullmatch(r"0x[0-9a-fA-F]+", name):
            return name
        resource_index = func_table["resource"][func_index]
        lib_index = resources["lib"][resource_index]
        library = self.profile_libs[lib_index]
        code_id = library.get("codeId")
        address = frame_table["address"][frame_index]
        if not isinstance(code_id, str) or not isinstance(address, int):
            return f"{library.get('name', 'unmapped')}::{name}"
        dataset = self.datasets.get(code_id)
        if dataset is None:
            return f"{library.get('name', 'unmapped')}::{name}"
        exact, starts, symbols = dataset
        symbol_index = exact.get(address)
        if symbol_index is None:
            position = bisect.bisect_right(starts, address) - 1
            if position >= 0:
                candidate = symbols[position]
                if address < candidate["rva"] + candidate["size"]:
                    symbol_index = position
        if symbol_index is None or not 0 <= symbol_index < len(symbols):
            return f"{library.get('name', 'unmapped')}::{name}"
        string_index = symbols[symbol_index]["symbol"]
        if not isinstance(string_index, int) or not 0 <= string_index < len(self.string_table):
            raise ProfileError(f"invalid symbol string index for {code_id}")
        return self.string_table[string_index]


def validate_parallel_table(table: dict[str, Any], required: tuple[str, ...], label: str) -> int:
    length = table.get("length")
    if not isinstance(length, int) or length < 0:
        raise ProfileError(f"{label} has no valid length")
    for field in required:
        values = table.get(field)
        if not isinstance(values, list) or len(values) != length:
            raise ProfileError(f"{label}.{field} does not match its declared length")
    return length


def stack_frames(thread: dict[str, Any], stack_index: int) -> list[int]:
    table = thread["stackTable"]
    frames: list[int] = []
    visited: set[int] = set()
    current: int | None = stack_index
    while current is not None:
        if current in visited or not 0 <= current < table["length"]:
            raise ProfileError("Samply stack table contains a cycle or invalid prefix")
        visited.add(current)
        frames.append(table["frame"][current])
        current = table["prefix"][current]
    return frames


def ranked_samples(
    counter: Counter[str], total: int, limit: int = 100
) -> list[dict[str, Any]]:
    return [
        {
            "symbol": symbol,
            "samples": count,
            "percent_of_window_samples": round(100.0 * count / total, 4) if total else 0.0,
        }
        for symbol, count in sorted(counter.items(), key=lambda item: (-item[1], item[0]))[:limit]
    ]


def ranked_cpu(
    counter: Counter[str], total_cpu_micros: float, limit: int = 100
) -> list[dict[str, Any]]:
    return [
        {
            "symbol": symbol,
            "thread_cpu_delta_micros": round(cpu_micros, 3),
            "percent_of_complete_interval_thread_cpu_delta": (
                round(100.0 * cpu_micros / total_cpu_micros, 4)
                if total_cpu_micros
                else 0.0
            ),
        }
        for symbol, cpu_micros in sorted(
            counter.items(), key=lambda item: (-item[1], item[0])
        )[:limit]
    ]


def analyze_span_counters(manifest: dict[str, Any], path: Path) -> dict[str, Any]:
    span_capture = manifest.get("span_capture")
    if not isinstance(span_capture, dict) or not isinstance(span_capture.get("window"), dict):
        raise ProfileError("profile manifest has no span-capture window")
    window = span_capture["window"]
    start = window.get("start_unix_nanos")
    end = window.get("end_unix_nanos")
    if not isinstance(start, int) or not isinstance(end, int) or start >= end:
        raise ProfileError("span-capture window is invalid")
    counters = read_json(path)
    if set(counters) != {"schema_version", "measurement", "window", "spans"}:
        raise ProfileError("span counter artifact has an unsupported shape")
    schema = counters["schema_version"]
    measurement = counters["measurement"]
    supported = {
        (1, "span_starts_during_target_work"),
        (2, "span_lifetimes_started_during_target_work"),
    }
    if (schema, measurement) not in supported:
        raise ProfileError("span counter artifact has an unsupported measurement")
    if counters["window"] != window:
        raise ProfileError("span counter artifact and manifest windows differ")
    spans = counters["spans"]
    if not isinstance(spans, list) or not spans:
        raise ProfileError("span counter artifact has no registered coordinates")
    names: list[str] = []
    selected = 0
    selected_elapsed_nanos = 0
    for span in spans:
        expected_fields = (
            {"name", "start_count"}
            if schema == 1
            else {"name", "start_count", "elapsed_nanos"}
        )
        if not isinstance(span, dict) or set(span) != expected_fields:
            raise ProfileError("span counter entry has an unsupported shape")
        name = span["name"]
        count = span["start_count"]
        if not isinstance(name, str) or not name.startswith("tx_pool."):
            raise ProfileError("span counter entry has an invalid name")
        if not isinstance(count, int) or isinstance(count, bool) or count < 0:
            raise ProfileError(f"span counter {name} has an invalid count")
        names.append(name)
        selected += count
        if schema == 2:
            elapsed_nanos = span["elapsed_nanos"]
            if (
                not isinstance(elapsed_nanos, int)
                or isinstance(elapsed_nanos, bool)
                or elapsed_nanos < 0
            ):
                raise ProfileError(f"span counter {name} has an invalid lifetime")
            selected_elapsed_nanos += elapsed_nanos
    if names != sorted(set(names)):
        raise ProfileError("span counter names must be unique and sorted")
    if selected == 0:
        raise ProfileError("span counter artifact contains no target-window work")
    batch_counts = {
        span["name"]: span["start_count"]
        for span in spans
        if isinstance(span, dict)
    }
    if manifest.get("harness") in {"pipeline", "profile_one_shot"} and batch_counts.get(
        "tx_pool.ingress.remote_batch", 0
    ) <= 0:
        raise ProfileError(
            "profile target did not traverse the production remote-batch ingress"
        )
    return {
        "window": window,
        "schema_version": schema,
        "measurement": measurement,
        "selected_span_starts": selected,
        "selected_span_elapsed_nanos": (
            selected_elapsed_nanos if schema == 2 else None
        ),
        "spans": spans,
        "measurement_caveat": (
            "start counts are schedule-dependent control-flow observations from a separate "
            "low-overhead execution; CPU samples and controlled A/B own timing conclusions"
        ),
    }


def analyze_profile(manifest: dict[str, Any], bundle_dir: Path) -> dict[str, Any]:
    artifacts = manifest.get("artifacts")
    if not isinstance(artifacts, dict):
        raise ProfileError("profile manifest has no artifact table")
    paths = {
        label: verify_artifact(record, label, bundle_dir)
        for label, record in artifacts.items()
    }
    try:
        stdout = paths["stdout"].read_text()
        span_stdout = paths["span_stdout"].read_text()
    except (OSError, UnicodeError) as error:
        raise ProfileError(f"cannot read profile process output: {error}") from error
    if parse_marker(stdout) != manifest.get("window"):
        raise ProfileError("profile stdout window differs from its manifest")
    span_capture = manifest.get("span_capture")
    if not isinstance(span_capture, dict) or parse_marker(span_stdout) != span_capture.get(
        "window"
    ):
        raise ProfileError("span stdout window differs from its manifest")
    if manifest.get("harness") == "profile_one_shot":
        expected = manifest.get("scenario")
        if not isinstance(expected, dict):
            raise ProfileError("one-shot profile manifest has no scenario identity")
        observation = parse_observation(stdout, expected)
        if observation != manifest.get("observation"):
            raise ProfileError("profile observation differs from its manifest")
        parse_observation(span_stdout, expected)
    elif manifest.get("observation") is not None:
        raise ProfileError("pipeline profile manifest contains a one-shot observation")
    profile = read_json(paths["profile"])
    sidecar = read_json(paths["symbols"])
    resolver = SymbolResolver(profile, sidecar)
    meta = profile.get("meta")
    threads = profile.get("threads")
    if not isinstance(meta, dict) or not isinstance(threads, list):
        raise ProfileError("Samply profile has no meta/thread data")
    start_time_ms = meta.get("startTime")
    interval_ms = meta.get("interval")
    sample_units = meta.get("sampleUnits")
    if not isinstance(start_time_ms, (int, float)) or not isinstance(interval_ms, (int, float)):
        raise ProfileError("Samply profile has invalid timing metadata")
    if not isinstance(sample_units, dict) or sample_units.get("threadCPUDelta") != "µs":
        raise ProfileError("Samply thread CPU delta unit is not microseconds")
    window = manifest.get("window")
    if not isinstance(window, dict):
        raise ProfileError("profile manifest has no target window")
    window_start_ms = window["start_unix_nanos"] / 1_000_000 - start_time_ms
    window_end_ms = window["end_unix_nanos"] / 1_000_000 - start_time_ms
    if window_start_ms < 0 or window_start_ms >= window_end_ms:
        raise ProfileError("target window falls before the Samply profile")

    sample_leaf: Counter[str] = Counter()
    sample_inclusive: Counter[str] = Counter()
    cpu_leaf: Counter[str] = Counter()
    cpu_inclusive: Counter[str] = Counter()
    thread_summaries: list[dict[str, Any]] = []
    total_samples = 0
    total_cpu_micros = 0
    attributed_cpu_micros = 0
    for thread in threads:
        if not isinstance(thread, dict):
            raise ProfileError("Samply thread entry is not an object")
        samples = thread.get("samples")
        if not isinstance(samples, dict):
            raise ProfileError("Samply thread has no samples table")
        if "time" in samples:
            time_field = "time"
            absolute_time = True
        elif "timeDeltas" in samples:
            time_field = "timeDeltas"
            absolute_time = False
        else:
            raise ProfileError("Samply samples have no supported time coordinate")
        length = validate_parallel_table(
            samples, (time_field, "stack", "threadCPUDelta"), "samples"
        )
        validate_parallel_table(thread["stackTable"], ("frame", "prefix"), "stackTable")
        validate_parallel_table(thread["frameTable"], ("func", "address"), "frameTable")
        validate_parallel_table(thread["funcTable"], ("name", "resource"), "funcTable")
        validate_parallel_table(thread["resourceTable"], ("lib",), "resourceTable")
        elapsed_ms = 0.0
        selected = 0
        selected_cpu = 0
        previous_ms: float | None = None
        for index in range(length):
            coordinate = samples[time_field][index]
            if not isinstance(coordinate, (int, float)) or coordinate < 0:
                raise ProfileError("Samply sample time coordinate is invalid")
            if absolute_time:
                if previous_ms is not None and coordinate < previous_ms:
                    raise ProfileError("Samply absolute sample times are not monotonic")
                elapsed_ms = coordinate
            else:
                elapsed_ms += coordinate
            inside = window_start_ms <= elapsed_ms <= window_end_ms
            if inside:
                selected += 1
                stack_index = samples["stack"][index]
                names: list[str] = []
                if stack_index is not None:
                    frames = stack_frames(thread, stack_index)
                    names = [resolver.frame_name(thread, frame) for frame in frames]
                    if names:
                        sample_leaf[names[0]] += 1
                        for name in set(names):
                            sample_inclusive[name] += 1
                cpu_delta = samples["threadCPUDelta"][index]
                if not isinstance(cpu_delta, (int, float)) or cpu_delta < 0:
                    raise ProfileError("Samply thread CPU delta is invalid")
                # A CPU delta describes the interval since the previous sample.
                # Exclude an interval that crosses the benchmark window boundary.
                if previous_ms is not None and previous_ms >= window_start_ms:
                    selected_cpu += cpu_delta
                    if names:
                        attributed_cpu_micros += cpu_delta
                        cpu_leaf[names[0]] += cpu_delta
                        for name in set(names):
                            cpu_inclusive[name] += cpu_delta
            previous_ms = elapsed_ms
        if selected:
            thread_summaries.append(
                {
                    "name": thread.get("name", ""),
                    "pid": thread.get("pid"),
                    "tid": thread.get("tid"),
                    "samples": selected,
                    "cpu_micros_complete_intervals": round(selected_cpu, 3),
                }
            )
            total_samples += selected
            total_cpu_micros += selected_cpu
    if total_samples == 0:
        raise ProfileError("Samply profile contains no samples in the target window")
    if total_cpu_micros <= 0:
        raise ProfileError("Samply profile contains no CPU time in complete target intervals")
    thread_summaries.sort(key=lambda item: (-item["samples"], str(item["tid"])))
    unattributed_cpu_micros = total_cpu_micros - attributed_cpu_micros
    observation = manifest.get("observation")
    cpu_clock_crosscheck = None
    if isinstance(observation, dict):
        process_cpu_micros = observation["target_cpu_nanos"] / 1_000
        cpu_clock_crosscheck = {
            "process_rusage_cpu_micros": round(process_cpu_micros, 3),
            "samply_complete_interval_cpu_micros": round(total_cpu_micros, 3),
            "difference_micros": round(total_cpu_micros - process_cpu_micros, 3),
            "samply_percent_of_process_cpu": round(
                100.0 * total_cpu_micros / process_cpu_micros, 4
            ),
            "interpretation": (
                "aggregate coverage cross-check only; thread CPU delta is associated with "
                "the interval-ending sampled stack and is not exact async attribution"
            ),
        }
    return {
        "schema_version": SUMMARY_SCHEMA_VERSION,
        "harness": manifest["harness"],
        "scenario": manifest["scenario"],
        "observation": manifest.get("observation"),
        "window": {
            "scenario_name": window["scenario"],
            "start_unix_nanos": window["start_unix_nanos"],
            "end_unix_nanos": window["end_unix_nanos"],
            "wall_elapsed_nanos": window["elapsed_nanos"],
            "profile_start_offset_ms": round(window_start_ms, 6),
            "profile_end_offset_ms": round(window_end_ms, 6),
        },
        "sampling": {
            "requested_rate_hz": manifest["capture"]["sample_rate_hz"],
            "profile_interval_ms": interval_ms,
            "window_samples": total_samples,
            "cpu_micros_complete_intervals": round(total_cpu_micros, 3),
            "attributed_cpu_micros": round(attributed_cpu_micros, 3),
            "unattributed_cpu_micros": round(unattributed_cpu_micros, 3),
            "cpu_attribution_percent": round(
                100.0 * attributed_cpu_micros / total_cpu_micros, 4
            ),
            "cpu_boundary_policy": "exclude intervals whose previous sample precedes the window",
            "cpu_stack_policy": (
                "attribute each complete interval's threadCPUDelta to the stack at its ending "
                "sample; sample-count rankings separately describe sampled residency"
            ),
            "cpu_clock_crosscheck": cpu_clock_crosscheck,
        },
        "threads": thread_summaries,
        "top_leaf_symbols_by_thread_cpu_delta": ranked_cpu(
            cpu_leaf, total_cpu_micros
        ),
        "top_inclusive_symbols_by_thread_cpu_delta": ranked_cpu(
            cpu_inclusive, total_cpu_micros
        ),
        "top_leaf_symbols_by_window_samples": ranked_samples(
            sample_leaf, total_samples
        ),
        "top_inclusive_symbols_by_window_samples": ranked_samples(
            sample_inclusive, total_samples
        ),
        "span_capture": analyze_span_counters(manifest, paths["spans"]),
    }


def analyze_manifest(manifest_path: Path) -> Path:
    absolute = manifest_path.expanduser().resolve(strict=True)
    manifest = read_json(absolute)
    if manifest.get("schema_version") != MANIFEST_SCHEMA_VERSION:
        raise ProfileError("profile manifest schema version is unsupported")
    inputs = manifest.get("inputs")
    if not isinstance(inputs, dict):
        raise ProfileError("profile manifest has no input identity")
    binary = inputs.get("binary")
    if not isinstance(binary, dict) or set(binary) != {
        "path_at_capture",
        "size_bytes",
        "sha256",
    }:
        raise ProfileError("profile manifest has no exact binary identity")
    if (
        not isinstance(binary["path_at_capture"], str)
        or not binary["path_at_capture"]
        or not isinstance(binary["size_bytes"], int)
        or isinstance(binary["size_bytes"], bool)
        or binary["size_bytes"] <= 0
        or not isinstance(binary["sha256"], str)
        or re.fullmatch(r"[0-9a-f]{64}", binary["sha256"]) is None
    ):
        raise ProfileError("profile manifest binary identity is invalid")
    harness = manifest.get("harness")
    expected_sources = {
        "pipeline": [BENCHMARK_SOURCE, SCRIPT_SOURCE],
        "profile_one_shot": [ONE_SHOT_SOURCE, SCRIPT_SOURCE],
    }.get(harness)
    if expected_sources is None:
        raise ProfileError("profile manifest names an unsupported harness")
    recorded_sources = inputs.get("harness_sources")
    current_sources = [
        source.relative_to(WORKSPACE_ROOT).as_posix() for source in expected_sources
    ]
    if recorded_sources != current_sources:
        raise ProfileError("profile manifest harness source set is unsupported")
    current_harness = files_sha256(expected_sources)
    if inputs.get("harness_sha256") != current_harness:
        raise ProfileError(
            "profile manifest belongs to a different benchmark/analyzer source; "
            "check out its recorded revision before re-analysis"
        )
    summary_relative = Path(manifest.get("summary_path", ""))
    if summary_relative.is_absolute() or ".." in summary_relative.parts:
        raise ProfileError("profile manifest summary path is not bundle-relative")
    summary_path = (absolute.parent / summary_relative).resolve()
    try:
        summary_path.relative_to(absolute.parent.resolve())
    except ValueError as error:
        raise ProfileError("profile summary escapes its bundle") from error
    summary = analyze_profile(manifest, absolute.parent)
    write_json(summary_path, summary)
    return summary_path


def main() -> int:
    args = parse_args()
    try:
        if args.action in {"capture", "capture-one-shot"}:
            manifest = capture(args)
            print(f"profile manifest: {manifest}")
            print(f"profile summary: {read_json(manifest)['summary_path']}")
        else:
            summary = analyze_manifest(args.manifest)
            print(f"profile summary: {summary}")
    except (ProfileError, OSError, subprocess.SubprocessError, ValueError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
