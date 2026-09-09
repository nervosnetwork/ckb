#!/usr/bin/env python3
"""Capture and deterministically analyze one tx-pool Samply profile."""

from __future__ import annotations

import argparse
import bisect
import gzip
import hashlib
import json
import math
import os
import platform
import re
import subprocess
import sys
from collections import Counter
from pathlib import Path
from typing import Any

from measurement_process import run_process


WORKSPACE_ROOT = Path(__file__).resolve().parents[2]
ONE_SHOT_SOURCE = WORKSPACE_ROOT / "tx-pool" / "benches" / "profile_one_shot.rs"
SPAN_SOURCE = ONE_SHOT_SOURCE.with_name("profile_spans") / "mod.rs"
SCRIPT_SOURCE = Path(__file__).resolve()
PROCESS_SOURCE = SCRIPT_SOURCE.with_name("measurement_process.py")
REMAPPED_SOURCE_ROOT = "/ckb-txpool-profile-source"
MARKER_PREFIX = "TX_POOL_PROFILE_WINDOW "
OBSERVATION_PREFIX = "TX_POOL_PROFILE_OBSERVATION "
ONE_SHOT_FEATURES = ("profiling",)
PROFILE_SCHEMA_VERSION = 2
OBSERVATION_SCHEMA_VERSION = 2
MANIFEST_SCHEMA_VERSION = 8
SUMMARY_SCHEMA_VERSION = 7
FINAL_BUILD_PROFILE = "prod"
ARTIFACT_SUFFIXES = {
    "profile": ".json.gz",
    "symbols": ".json.syms.json",
    "stdout": ".stdout.log",
    "stderr": ".stderr.log",
    "spans": ".spans.json",
    "span_stdout": ".span.stdout.log",
    "span_stderr": ".span.stderr.log",
}
SCENARIO_FIELDS = ("scenario", "target", "warm", "workers", "peers")
OBSERVATION_INTEGER_FIELDS = (
    "elapsed_nanos",
    "accepted",
    "callback_duplicates",
    "p99_latency_nanos",
    "target_cpu_nanos",
    "target_user_cpu_nanos",
    "target_system_cpu_nanos",
    "allocation_calls",
    "allocated_bytes",
    "reorg_latency_nanos",
    "reorg_overlap_callbacks",
    "relay_ok",
    "relay_duplicate_ok",
    "relay_rejects",
    "relay_unknown_parents",
    "relay_generation_resets",
    "shutdown_latency_nanos",
)
OBSERVATION_FIELDS = {
    "schema_version",
    *SCENARIO_FIELDS,
    *OBSERVATION_INTEGER_FIELDS,
    "throughput_tps",
    "relay_unknown_parent_observations",
}


class ProfileError(RuntimeError):
    """A capture identity or profile artifact is invalid."""


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
    actions = parser.add_subparsers(dest="action", required=True)
    capture = actions.add_parser("capture", help="capture and analyze one workload")
    capture.add_argument("--output-prefix", type=Path, required=True)
    capture.add_argument("--binary", type=Path)
    capture.add_argument("--binary-profile")
    capture.add_argument(
        "--target-dir",
        type=Path,
        default=WORKSPACE_ROOT / "target" / "tx-pool-profile-one-shot",
    )
    capture.add_argument("--rate", type=positive_integer, default=1000)
    capture.add_argument("--scenario", required=True)
    capture.add_argument("--target", type=positive_integer, required=True)
    capture.add_argument("--warm", type=nonnegative_integer, required=True)
    capture.add_argument("--workers", type=positive_integer, required=True)
    capture.add_argument("--peers", type=positive_integer, required=True)
    capture.add_argument("--force", action="store_true")
    capture.add_argument("--timeout-seconds", type=positive_integer, default=180)
    analyze = actions.add_parser("analyze", help="verify and reanalyze a bundle")
    analyze.add_argument("--manifest", type=Path, required=True)
    args = parser.parse_args()
    if args.action == "capture":
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
        raise ValueError("reused binaries require an explicit prod profile attestation")
    return profile


def run(
    command: list[str],
    *,
    env: dict[str, str] | None = None,
    label: str = "command",
    timeout: float = 120,
    log_paths: tuple[Path, Path] | None = None,
) -> subprocess.CompletedProcess[str]:
    try:
        return run_process(
            command,
            timeout=timeout,
            cwd=WORKSPACE_ROOT,
            env=env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=True,
        )
    except OSError as error:
        raise ProfileError(f"cannot execute {label}: {error}") from error
    except subprocess.TimeoutExpired as error:
        if log_paths is not None:
            captured = subprocess.CompletedProcess(command, 124, error.stdout or "", error.stderr or "")
            save_output(captured, *log_paths)
        detail = error.stderr or error.stdout or "no process output"
        if isinstance(detail, bytes):
            detail = detail.decode(errors="replace")
        raise ProfileError(f"{label} exceeded {timeout:g} seconds:\n{detail}") from error
    except subprocess.CalledProcessError as error:
        if log_paths is not None:
            save_output(subprocess.CompletedProcess(command, error.returncode, error.stdout or "", error.stderr or ""), *log_paths)
        detail = (error.stderr or error.stdout or "no process output").strip()
        raise ProfileError(f"{label} failed ({error.returncode}):\n{detail}") from error
    except (KeyboardInterrupt, SystemExit) as error:
        if log_paths is not None:
            save_output(subprocess.CompletedProcess(command, 130,
                        getattr(error, "output", None) or "",
                        getattr(error, "stderr", None) or ""), *log_paths)
        raise


def command_output(command: list[str]) -> str:
    output = run(command, label=command[0]).stdout.strip()
    if not output:
        raise ProfileError(f"{command[0]} produced no identity output")
    return output


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
        digest.update(path.relative_to(WORKSPACE_ROOT).as_posix().encode())
        digest.update(b"\0")
        digest.update(path.read_bytes())
        digest.update(b"\0")
    return digest.hexdigest()


def read_json(path: Path) -> dict[str, Any]:
    try:
        source = (
            gzip.open(path, "rt", encoding="utf-8")
            if path.suffix == ".gz"
            else path.open(encoding="utf-8")
        )
        with source:
            value = json.load(source)
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise ProfileError(f"cannot read JSON artifact {path}: {error}") from error
    if not isinstance(value, dict):
        raise ProfileError(f"JSON artifact is not an object: {path}")
    return value


def write_json(path: Path, value: dict[str, Any]) -> None:
    try:
        path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")
    except OSError as error:
        raise ProfileError(f"cannot write {path}: {error}") from error


def output_paths(prefix: Path) -> dict[str, Path]:
    absolute = prefix.expanduser().resolve()
    if absolute == WORKSPACE_ROOT or WORKSPACE_ROOT in absolute.parents:
        raise ProfileError("profile artifacts must be stored outside the source tree")
    paths = {
        name: Path(f"{absolute}{suffix}")
        for name, suffix in ARTIFACT_SUFFIXES.items()
    }
    paths["manifest"] = Path(f"{absolute}.manifest.json")
    paths["summary"] = Path(f"{absolute}.summary.json")
    return paths


def prepare_outputs(paths: dict[str, Path], force: bool) -> None:
    existing = [path for path in paths.values() if path.exists()]
    if existing and not force:
        raise ProfileError(f"refusing to overwrite existing artifacts: {existing}")
    for path in existing:
        if not path.is_file():
            raise ProfileError(f"refusing to replace non-file artifact: {path}")
        path.unlink()
    paths["profile"].parent.mkdir(parents=True, exist_ok=True)


def artifact(path: Path, bundle_dir: Path) -> dict[str, Any]:
    resolved = path.resolve(strict=True)
    try:
        relative = resolved.relative_to(bundle_dir.resolve(strict=True))
    except ValueError as error:
        raise ProfileError(f"artifact is outside its bundle: {resolved}") from error
    return {
        "path": relative.as_posix(),
        "size_bytes": resolved.stat().st_size,
        "sha256": sha256_file(resolved),
    }


def verify_artifacts(manifest: dict[str, Any], bundle_dir: Path) -> dict[str, Path]:
    records = manifest.get("artifacts")
    if not isinstance(records, dict) or set(records) != set(ARTIFACT_SUFFIXES):
        raise ProfileError("manifest artifact table is unsupported")
    paths: dict[str, Path] = {}
    root = bundle_dir.resolve()
    for label, record in records.items():
        if not isinstance(record, dict) or set(record) != {
            "path",
            "size_bytes",
            "sha256",
        }:
            raise ProfileError(f"{label} artifact identity is invalid")
        relative = Path(record["path"])
        if relative.is_absolute() or ".." in relative.parts:
            raise ProfileError(f"{label} artifact path is not bundle-relative")
        path = (root / relative).resolve()
        if root not in path.parents or not path.is_file():
            raise ProfileError(f"{label} artifact is missing or outside its bundle")
        if path.stat().st_size != record["size_bytes"]:
            raise ProfileError(f"{label} artifact size changed")
        if sha256_file(path) != record["sha256"]:
            raise ProfileError(f"{label} artifact hash changed")
        paths[label] = path
    return paths


def build_environment(target_dir: Path) -> dict[str, str]:
    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = str(target_dir.expanduser().resolve())
    env["CARGO_INCREMENTAL"] = "0"
    remap = f"--remap-path-prefix={WORKSPACE_ROOT}={REMAPPED_SOURCE_ROOT}"
    if env.get("CARGO_ENCODED_RUSTFLAGS"):
        env["CARGO_ENCODED_RUSTFLAGS"] += f"\x1f{remap}"
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
    completed = run(command, env=env, label="profile binary build", timeout=3600)
    executables = []
    for line in completed.stdout.splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        target = message.get("target", {})
        if (
            message.get("reason") == "compiler-artifact"
            and target.get("name") == bench_name
            and "bench" in target.get("kind", [])
            and message.get("executable")
        ):
            executables.append(Path(message["executable"]).resolve())
    unique = sorted(set(executables))
    if len(unique) != 1 or not unique[0].is_file():
        raise ProfileError(f"Cargo reported {len(unique)} {bench_name} executables")
    return unique[0], command, env


def tagged_json(stdout: str, prefix: str, label: str) -> dict[str, Any]:
    records = [
        line.removeprefix(prefix)
        for line in stdout.splitlines()
        if line.startswith(prefix)
    ]
    if len(records) != 1:
        raise ProfileError(f"expected exactly one {label}, found {len(records)}")
    try:
        value = json.loads(records[0])
    except json.JSONDecodeError as error:
        raise ProfileError(f"{label} is invalid JSON: {error}") from error
    if not isinstance(value, dict):
        raise ProfileError(f"{label} is not an object")
    return value


def parse_marker(stdout: str) -> dict[str, Any]:
    marker = tagged_json(stdout, MARKER_PREFIX, "profile window")
    if set(marker) != {
        "schema_version",
        "scenario",
        "start_unix_nanos",
        "end_unix_nanos",
        "elapsed_nanos",
    } or marker["schema_version"] != PROFILE_SCHEMA_VERSION:
        raise ProfileError("profile window schema is unsupported")
    start, end, elapsed = (
        marker["start_unix_nanos"],
        marker["end_unix_nanos"],
        marker["elapsed_nanos"],
    )
    if (
        any(type(value) is not int for value in (start, end, elapsed))
        or start >= end
        or end - start != elapsed
    ):
        raise ProfileError("profile window timestamps are inconsistent")
    if not isinstance(marker["scenario"], str) or not marker["scenario"]:
        raise ProfileError("profile scenario name is empty")
    return marker


def parse_observation(stdout: str, expected: dict[str, Any]) -> dict[str, Any]:
    observation = tagged_json(stdout, OBSERVATION_PREFIX, "profile observation")
    if (
        set(observation) != OBSERVATION_FIELDS
        or observation["schema_version"] != OBSERVATION_SCHEMA_VERSION
    ):
        raise ProfileError("profile observation schema is unsupported")
    identity = {name: observation[name] for name in SCENARIO_FIELDS}
    if identity != expected:
        raise ProfileError(f"profile observation drifted: {identity} != {expected}")
    if any(
        type(observation[name]) is not int or observation[name] < 0
        for name in OBSERVATION_INTEGER_FIELDS
    ):
        raise ProfileError("profile observation has an invalid integer metric")
    throughput = observation["throughput_tps"]
    if (
        not isinstance(throughput, (int, float))
        or isinstance(throughput, bool)
        or not math.isfinite(throughput)
        or throughput <= 0
    ):
        raise ProfileError("profile observation throughput is invalid")
    elapsed = observation["elapsed_nanos"]
    if elapsed <= 0 or not math.isclose(throughput, expected["target"] * 1e9 / elapsed, rel_tol=1e-9):
        raise ProfileError("profile throughput differs from target count and elapsed time")
    if (
        observation["target_user_cpu_nanos"]
        + observation["target_system_cpu_nanos"]
        != observation["target_cpu_nanos"]
    ):
        raise ProfileError("profile observation CPU components do not sum to total")
    accepted = expected["target"] + expected["warm"]
    if observation["accepted"] != accepted or observation["relay_ok"] != accepted:
        raise ProfileError("profile observation did not complete the exact workload")
    if (
        # Reorg can legitimately reaccept the same owner; relay Ok remains
        # unique. Match the one-shot executor and paired benchmark contract.
        (observation["callback_duplicates"] and expected["scenario"] != "reorg_in_flight")
        or observation["relay_duplicate_ok"]
        or observation["relay_generation_resets"]
    ):
        raise ProfileError("profile observation contains duplicate or reset terminals")
    expected_rejects = expected["warm"] if expected["scenario"] == "rbf_pairs" else 0
    if observation["relay_rejects"] != expected_rejects:
        raise ProfileError("profile observation contains an unexpected reject terminal set")
    if (observation["reorg_overlap_callbacks"] > 0) != (
        expected["scenario"] == "reorg_in_flight"
    ):
        raise ProfileError("profile observation reorg overlap differs from its scenario")
    unknown = observation["relay_unknown_parent_observations"]
    if not isinstance(unknown, list):
        raise ProfileError("profile observation unknown-parent evidence is invalid")
    for row in unknown:
        if (
            not isinstance(row, dict)
            or set(row) != {"peer", "parents", "count"}
            or type(row["peer"]) is not int
            or row["peer"] < 0
            or type(row["count"]) is not int
            or row["count"] <= 0
            or not isinstance(row["parents"], list)
            or not row["parents"]
            or any(not isinstance(parent, str) or not parent for parent in row["parents"])
        ):
            raise ProfileError("profile observation unknown-parent evidence is invalid")
    if sum(row["count"] for row in unknown) != observation["relay_unknown_parents"]:
        raise ProfileError("profile observation unknown-parent count does not match evidence")
    if observation["relay_unknown_parents"] and not expected["scenario"].endswith(
        "_reverse"
    ):
        raise ProfileError("profile observation contains unknown-parent terminals")
    return observation


def validate_window_observation(window: dict[str, Any], observation: dict[str, Any]) -> None:
    elapsed = observation["elapsed_nanos"]
    tolerance = max(1_000_000, elapsed // 10_000)
    if window["scenario"] != observation["scenario"] or abs(window["elapsed_nanos"] - elapsed) > tolerance:
        raise ProfileError("profile wall window differs from monotonic observation")


def git_identity() -> dict[str, str]:
    tracked = run(["git", "diff", "--binary", "HEAD"], label="git diff").stdout.encode()
    status = run(
        ["git", "status", "--porcelain=v1", "--untracked-files=all"],
        label="git status",
    ).stdout.encode()
    return {
        "revision": command_output(["git", "rev-parse", "HEAD"]),
        "tracked_diff_sha256": hashlib.sha256(tracked).hexdigest(),
        "status_sha256": hashlib.sha256(status).hexdigest(),
    }


def environment_identity(build_env: dict[str, str]) -> dict[str, Any]:
    return {
        "cargo": command_output(["cargo", "--version", "--verbose"]),
        "rustc": command_output(["rustc", "--version", "--verbose"]),
        "samply": command_output(["samply", "--version"]),
        "platform": platform.platform(),
        "machine": platform.machine(),
        "cpu_count": os.cpu_count(),
        "cpu_model": platform.processor(),
        "rustflags": build_env.get("RUSTFLAGS", ""),
        "cargo_encoded_rustflags": build_env.get("CARGO_ENCODED_RUSTFLAGS", ""),
    }


def save_output(
    completed: subprocess.CompletedProcess[str], stdout: Path, stderr: Path
) -> None:
    try:
        for path, value in ((stdout, completed.stdout), (stderr, completed.stderr)):
            path.write_text(value.decode(errors="replace") if isinstance(value, bytes) else value)
    except OSError as error:
        raise ProfileError(f"cannot save capture output: {error}") from error


def file_identity(path: Path) -> dict[str, Any]:
    path = path.resolve(strict=True)
    return {
        "path_at_capture": str(path),
        "size_bytes": path.stat().st_size,
        "sha256": sha256_file(path),
    }


def capture_source_identity(sources: list[Path]) -> dict[str, Any]:
    return {
        "git": git_identity(),
        "inputs": {
            "workspace_manifest_sha256": sha256_file(WORKSPACE_ROOT / "Cargo.toml"),
            "cargo_lock_sha256": sha256_file(WORKSPACE_ROOT / "Cargo.lock"),
            "tx_pool_manifest_sha256": sha256_file(WORKSPACE_ROOT / "tx-pool" / "Cargo.toml"),
            "harness_sources": [source.relative_to(WORKSPACE_ROOT).as_posix() for source in sources],
            "harness_sha256": files_sha256(sources),
        },
    }


def capture(args: argparse.Namespace) -> Path:
    paths = output_paths(args.output_prefix)
    prepare_outputs(paths, args.force)
    sources = [ONE_SHOT_SOURCE, SPAN_SOURCE, SCRIPT_SOURCE, PROCESS_SOURCE]
    frozen_source = capture_source_identity(sources)
    scenario = {name: getattr(args, name) for name in SCENARIO_FIELDS}
    runtime_args = [str(scenario[name]) for name in SCENARIO_FIELDS]
    runtime_env = os.environ.copy()
    runtime_env.pop("TX_POOL_PROFILE_TRACE_PATH", None)
    runtime_env["TX_POOL_BENCH_COMPARISON_CONTRACT"] = "protocol"
    if args.binary is None:
        binary, build_command, build_env = build_binary(
            args.target_dir, "profile_one_shot", ONE_SHOT_FEATURES
        )
    else:
        require_final_build_profile(args.binary_profile)
        binary = args.binary.expanduser().resolve(strict=True)
        if not binary.is_file() or not os.access(binary, os.X_OK):
            raise ProfileError(f"profile binary is not executable: {binary}")
        build_command, build_env = [], os.environ.copy()

    if capture_source_identity(sources) != frozen_source:
        raise ProfileError("profile source changed during build")
    frozen_binary = file_identity(binary)

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
    completed = run(command, env=runtime_env, label="Samply capture", timeout=args.timeout_seconds,
                    log_paths=(paths["stdout"], paths["stderr"]))
    save_output(completed, paths["stdout"], paths["stderr"])
    window = parse_marker(completed.stdout)
    observation = parse_observation(completed.stdout, scenario)
    validate_window_observation(window, observation)
    if not paths["profile"].is_file() or not paths["symbols"].is_file():
        raise ProfileError("Samply did not produce profile and symbol artifacts")

    span_env = runtime_env.copy()
    span_env["TX_POOL_PROFILE_TRACE_PATH"] = str(paths["spans"])
    span_command = [str(binary), *runtime_args]
    span_completed = run(span_command, env=span_env, label="span capture", timeout=args.timeout_seconds,
                         log_paths=(paths["span_stdout"], paths["span_stderr"]))
    save_output(span_completed, paths["span_stdout"], paths["span_stderr"])
    span_window = parse_marker(span_completed.stdout)
    span_observation = parse_observation(span_completed.stdout, scenario)
    validate_window_observation(span_window, span_observation)
    if not paths["spans"].is_file():
        raise ProfileError("span capture did not produce its artifact")
    if span_window["scenario"] != window["scenario"]:
        raise ProfileError("CPU and span captures used different scenarios")
    if capture_source_identity(sources) != frozen_source or file_identity(binary) != frozen_binary:
        raise ProfileError("profile source or binary changed during capture")

    manifest = {
        "schema_version": MANIFEST_SCHEMA_VERSION,
        "git": frozen_source["git"],
        "harness": "profile_one_shot",
        "features": list(ONE_SHOT_FEATURES),
        "scenario": scenario,
        "observation": observation,
        "window": window,
        "capture": {
            "sample_rate_hz": args.rate,
            "timeout_seconds": args.timeout_seconds,
            "command": command,
            "build_command": build_command,
            "build_profile": FINAL_BUILD_PROFILE,
        },
        "span_capture": {"command": span_command, "window": span_window},
        "environment": environment_identity(build_env),
        "inputs": {
            **frozen_source["inputs"],
            "binary": frozen_binary,
        },
        "artifacts": {
            label: artifact(paths[label], paths["manifest"].parent)
            for label in ARTIFACT_SUFFIXES
        },
        "summary_path": paths["summary"].name,
    }
    write_json(paths["manifest"], manifest)
    analyze_manifest(paths["manifest"])
    return paths["manifest"]


class SymbolResolver:
    def __init__(self, profile: dict[str, Any], sidecar: dict[str, Any]) -> None:
        self.libraries = profile.get("libs")
        self.strings = sidecar.get("string_table")
        datasets = sidecar.get("data")
        if (
            not isinstance(self.libraries, list)
            or not isinstance(self.strings, list)
            or not isinstance(datasets, list)
        ):
            raise ProfileError("Samply symbol data is invalid")
        self.datasets: dict[
            str, tuple[dict[int, int], list[int], list[dict[str, Any]]]
        ] = {}
        for dataset in datasets:
            if not isinstance(dataset, dict):
                raise ProfileError("Samply symbol dataset is invalid")
            code_id = dataset.get("code_id")
            symbols = dataset.get("symbol_table")
            known = dataset.get("known_addresses")
            if (
                not isinstance(code_id, str)
                or not isinstance(symbols, list)
                or not isinstance(known, list)
            ):
                raise ProfileError("Samply symbol dataset is invalid")
            starts = [symbol["rva"] for symbol in symbols]
            if starts != sorted(starts):
                raise ProfileError(f"symbol table for {code_id} is not sorted")
            self.datasets[code_id] = (
                {pair[0]: pair[1] for pair in known},
                starts,
                symbols,
            )

    def frame_name(self, thread: dict[str, Any], frame_index: int) -> str:
        frame_table = thread["frameTable"]
        func_index = frame_table["func"][frame_index]
        function = thread["funcTable"]
        name = thread["stringArray"][function["name"][func_index]]
        if not re.fullmatch(r"0x[0-9a-fA-F]+", name):
            return name
        resource = function["resource"][func_index]
        library = self.libraries[thread["resourceTable"]["lib"][resource]]
        address = frame_table["address"][frame_index]
        dataset = (
            self.datasets.get(library.get("codeId"))
            if isinstance(address, int)
            else None
        )
        if dataset is None:
            return f"{library.get('name', 'unmapped')}::{name}"
        exact, starts, symbols = dataset
        index = exact.get(address)
        if index is None:
            position = bisect.bisect_right(starts, address) - 1
            if (
                position >= 0
                and address < symbols[position]["rva"] + symbols[position]["size"]
            ):
                index = position
        if index is None:
            return f"{library.get('name', 'unmapped')}::{name}"
        return self.strings[symbols[index]["symbol"]]


def table_length(table: dict[str, Any], fields: tuple[str, ...], label: str) -> int:
    length = table.get("length")
    if type(length) is not int or length < 0:
        raise ProfileError(f"{label} length is invalid")
    if any(
        not isinstance(table.get(field), list) or len(table[field]) != length
        for field in fields
    ):
        raise ProfileError(f"{label} columns do not match its length")
    return length


def stack_names(
    thread: dict[str, Any], stack_index: int, resolver: SymbolResolver
) -> list[str]:
    table = thread["stackTable"]
    frames = []
    visited = set()
    current: int | None = stack_index
    while current is not None:
        if current in visited or not 0 <= current < table["length"]:
            raise ProfileError("Samply stack table contains an invalid prefix")
        visited.add(current)
        frames.append(resolver.frame_name(thread, table["frame"][current]))
        current = table["prefix"][current]
    return frames


def ranked(counter: Counter[str]) -> list[dict[str, Any]]:
    return [
        {"symbol": symbol, "estimated_cpu_weight_micros": round(cpu, 3)}
        for symbol, cpu in sorted(counter.items(), key=lambda item: (-item[1], item[0]))[
            :100
        ]
    ]


def analyze_samples(
    profile: dict[str, Any], resolver: SymbolResolver, window: dict[str, Any]
) -> dict[str, Any]:
    meta, threads = profile.get("meta"), profile.get("threads")
    if not isinstance(meta, dict) or not isinstance(threads, list):
        raise ProfileError("Samply profile has no timing/thread data")
    start_time = meta.get("startTime")
    interval = meta.get("interval")
    if (any(type(value) not in (int, float) or not math.isfinite(value)
            for value in (start_time, interval)) or interval <= 0):
        raise ProfileError("Samply timing metadata is invalid")
    cpu_unit = meta.get("sampleUnits", {}).get("threadCPUDelta")
    cpu_scale = {"µs": 1.0, "ns": 0.001}.get(cpu_unit)
    if cpu_scale is None:
        raise ProfileError("Samply CPU deltas have an unsupported unit")
    start = window["start_unix_nanos"] / 1_000_000 - start_time
    end = window["end_unix_nanos"] / 1_000_000 - start_time
    if start < 0 or start >= end:
        raise ProfileError("target window falls before the Samply profile")

    leaf: Counter[str] = Counter()
    inclusive: Counter[str] = Counter()
    samples_in_window = 0
    complete_intervals = 0
    missing_cpu_intervals = 0
    positive_cpu_intervals = 0
    attributed_cpu = 0.0
    complete_cpu = 0.0
    for thread in threads:
        samples = thread.get("samples") if isinstance(thread, dict) else None
        if not isinstance(samples, dict):
            raise ProfileError("Samply thread has no samples")
        time_field = (
            "time"
            if "time" in samples
            else "timeDeltas"
            if "timeDeltas" in samples
            else None
        )
        if time_field is None:
            raise ProfileError("Samply samples have no time coordinate")
        length = table_length(
            samples, (time_field, "stack", "threadCPUDelta"), "samples"
        )
        table_length(thread["stackTable"], ("frame", "prefix"), "stackTable")
        table_length(thread["frameTable"], ("func", "address"), "frameTable")
        table_length(thread["funcTable"], ("name", "resource"), "funcTable")
        table_length(thread["resourceTable"], ("lib",), "resourceTable")
        elapsed = 0.0
        previous: float | None = None
        for index in range(length):
            coordinate = samples[time_field][index]
            if (
                not isinstance(coordinate, (int, float))
                or isinstance(coordinate, bool)
                or not math.isfinite(coordinate)
                or coordinate < 0
            ):
                raise ProfileError("Samply sample time is invalid")
            if time_field == "time":
                if previous is not None and coordinate < previous:
                    raise ProfileError("Samply absolute sample times are not monotonic")
                elapsed = coordinate
            else:
                elapsed += coordinate
            if start <= elapsed <= end:
                samples_in_window += 1
                if previous is not None and previous >= start:
                    complete_intervals += 1
                    cpu = samples["threadCPUDelta"][index]
                    if cpu is None:
                        # A failed backend CPU read is missing data, not zero.
                        missing_cpu_intervals += 1
                    else:
                        if (type(cpu) not in (int, float) or not math.isfinite(cpu) or cpu < 0):
                            raise ProfileError("Samply thread CPU delta is invalid")
                        cpu *= cpu_scale
                        positive_cpu_intervals += cpu > 0
                        complete_cpu += cpu
                        stack = samples["stack"][index]
                        names = [] if stack is None else stack_names(thread, stack, resolver)
                        if names:
                            attributed_cpu += cpu
                            leaf[names[0]] += cpu
                            for name in set(names):
                                inclusive[name] += cpu
            previous = elapsed
    if not samples_in_window or complete_cpu <= 0:
        raise ProfileError("Samply profile has no complete target-window CPU samples")
    return {
        "window": window,
        "sampling": {
            "profile_interval_ms": interval,
            "window_samples": samples_in_window,
            "complete_interval_cpu_micros": round(complete_cpu, 3),
            "complete_intervals": complete_intervals,
            "missing_cpu_intervals": missing_cpu_intervals,
            "observed_cpu_interval_fraction": (complete_intervals - missing_cpu_intervals) / complete_intervals,
            "source_cpu_unit": cpu_unit,
            "positive_cpu_intervals": positive_cpu_intervals,
            "unattributed_cpu_weight_micros": round(complete_cpu - attributed_cpu, 3),
            "attribution": "previous_complete_interval_cpu_weight_assigned_to_current_sample_stack",
            "interpretation": "Statistical hotspot weights, not causal function CPU. Missing CPU intervals are excluded, not zero. Waiting stacks do not establish CPU spent waiting; inclusive weights overlap.",
        },
        "leaf_hotspots": ranked(leaf),
        "inclusive_hotspots": ranked(inclusive),
    }


def analyze_spans(manifest: dict[str, Any], path: Path) -> dict[str, Any]:
    counters = read_json(path)
    expected_window = manifest.get("span_capture", {}).get("window")
    identity = (counters.get("schema_version"), counters.get("instrumentation"), counters.get("measurement"))
    legacy = identity == (3, "authority_v1", "span_lifetimes_started_during_target_work")
    entered = identity == (4, "authority_v2", "entered_scope_wall_time_within_capture_window")
    fields = {"schema_version", "instrumentation", "measurement", "window", "spans"}
    if entered:
        fields.add("capture_window")
    if not (legacy or entered) or set(counters) != fields:
        raise ProfileError("span artifact schema is unsupported")
    if entered:
        capture = counters["capture_window"]
        if not isinstance(capture, dict) or set(capture) != {"start_unix_nanos", "end_unix_nanos", "elapsed_nanos"}:
            raise ProfileError("span capture window schema is unsupported")
        start, end, elapsed = (capture[name] for name in ("start_unix_nanos", "end_unix_nanos", "elapsed_nanos"))
        if any(type(value) is not int for value in (start, end, elapsed)) or start < 0 or start >= end or end - start != elapsed:
            raise ProfileError("span capture window timestamps are inconsistent")
    if counters["window"] != expected_window:
        raise ProfileError("span artifact and manifest windows differ")
    spans = counters["spans"]
    if not isinstance(spans, list) or not spans:
        raise ProfileError("span artifact is empty")
    names = []
    for span in spans:
        fields = {"name", "start_count", "elapsed_nanos"}
        if entered:
            fields.update({"enter_count", "active_at_start", "active_at_end"})
        if not isinstance(span, dict) or set(span) != fields:
            raise ProfileError("span entry schema is unsupported")
        if (not isinstance(span["name"], str) or not span["name"].startswith("tx_pool.")
                or any(type(span[field]) is not int or span[field] < 0 for field in fields - {"name"})):
            raise ProfileError("span entry is invalid")
        if entered and (span["active_at_end"] > span["active_at_start"] + span["enter_count"]
                or (span["active_at_start"] + span["enter_count"] == 0 and span["elapsed_nanos"] != 0)):
            raise ProfileError("span entry depth or duration is inconsistent")
        names.append(span["name"])
    if names != sorted(set(names)):
        raise ProfileError("span names must be unique and sorted")
    observed = lambda span: span["enter_count"] + span["active_at_start"] if entered else span["start_count"]
    by_name = {span["name"]: observed(span) for span in spans}
    if by_name.get("tx_pool.ingress.remote_batch", 0) <= 0:
        raise ProfileError("profile did not traverse production remote-batch ingress")
    required = ("tx_pool.authority.apply", "tx_pool.effects.publish",
                "tx_pool.membership.admission", "tx_pool.stage.resolve", "tx_pool.stage.verify")
    missing = [name for name in required if by_name.get(name, 0) <= 0]
    if missing:
        raise ProfileError(f"profile lacks current-authority coverage: {', '.join(missing)}")
    starts = sum(span["start_count"] for span in spans)
    if starts == 0:
        raise ProfileError("span artifact contains no target-window work")
    return {
        "window": counters["window"],
        "measurement": counters["measurement"],
        "instrumentation": counters["instrumentation"],
        "total_starts": starts,
        "total_elapsed_nanos": sum(span["elapsed_nanos"] for span in spans),
        "spans": spans,
        "unobserved_span_names": [span["name"] for span in spans if observed(span) == 0],
        **({"capture_window": counters["capture_window"]} if entered else {}),
        "interpretation": (
            "Inclusive entered-scope/poll wall time inside the separately reported capture window, using a Unix-anchored monotonic clock. "
            "This window differs slightly from the benchmark marker because of recorder setup/finalization. "
            "Blocking and descheduling inside entries count; suspension and child work while the parent is inactive do not. "
            "Stage.verify is the driver, not separately spawned VM execution. Durations overlap and are not additive CPU. "
            "Start counts describe creations; preexisting spans may contribute entered time with zero starts."
            if entered else
            "Historical overlapping span lifetimes from a separate run, including retained references, not additive CPU. Zero starts means unobserved instrumentation, not absence of production work."
        ),
    }


def analyze_profile(manifest: dict[str, Any], bundle_dir: Path) -> dict[str, Any]:
    paths = verify_artifacts(manifest, bundle_dir)
    try:
        stdout = paths["stdout"].read_text()
        span_stdout = paths["span_stdout"].read_text()
    except (OSError, UnicodeError) as error:
        raise ProfileError(f"cannot read capture output: {error}") from error
    window = parse_marker(stdout)
    span_window = parse_marker(span_stdout)
    if window != manifest.get("window") or span_window != manifest.get(
        "span_capture", {}
    ).get("window"):
        raise ProfileError("capture window differs from its manifest")
    expected = manifest.get("scenario")
    if not isinstance(expected, dict) or set(expected) != set(SCENARIO_FIELDS):
        raise ProfileError("manifest scenario identity is invalid")
    observation = parse_observation(stdout, expected)
    validate_window_observation(window, observation)
    if observation != manifest.get("observation"):
        raise ProfileError("profile observation differs from its manifest")
    validate_window_observation(span_window, parse_observation(span_stdout, expected))
    profile = read_json(paths["profile"])
    samples = analyze_samples(
        profile,
        SymbolResolver(profile, read_json(paths["symbols"])),
        window,
    )
    return {
        "schema_version": SUMMARY_SCHEMA_VERSION,
        "scenario": expected,
        "observation": observation,
        **samples,
        "sampling": {
            "requested_rate_hz": manifest["capture"]["sample_rate_hz"],
            **samples["sampling"],
        },
        "span_capture": analyze_spans(manifest, paths["spans"]),
    }


def analyze_manifest(manifest_path: Path) -> Path:
    absolute = manifest_path.expanduser().resolve(strict=True)
    manifest = read_json(absolute)
    if manifest.get("schema_version") != MANIFEST_SCHEMA_VERSION:
        raise ProfileError("manifest schema is unsupported")
    if manifest.get("harness") != "profile_one_shot" or manifest.get(
        "features"
    ) != list(ONE_SHOT_FEATURES):
        raise ProfileError("manifest harness identity is unsupported")
    sources = [ONE_SHOT_SOURCE, SPAN_SOURCE, SCRIPT_SOURCE, PROCESS_SOURCE]
    inputs = manifest.get("inputs")
    if (
        not isinstance(inputs, dict)
        or inputs.get("harness_sources")
        != [source.relative_to(WORKSPACE_ROOT).as_posix() for source in sources]
        or inputs.get("harness_sha256") != files_sha256(sources)
    ):
        raise ProfileError("manifest belongs to a different harness/analyzer source")
    binary = inputs.get("binary")
    if (
        not isinstance(binary, dict)
        or set(binary) != {"path_at_capture", "size_bytes", "sha256"}
        or type(binary["size_bytes"]) is not int
        or binary["size_bytes"] <= 0
        or not isinstance(binary["sha256"], str)
        or re.fullmatch(r"[0-9a-f]{64}", binary["sha256"]) is None
    ):
        raise ProfileError("manifest binary identity is invalid")
    relative = Path(manifest.get("summary_path", ""))
    if relative.is_absolute() or ".." in relative.parts:
        raise ProfileError("summary path is not bundle-relative")
    summary_path = (absolute.parent / relative).resolve()
    if absolute.parent.resolve() not in summary_path.parents:
        raise ProfileError("summary path escapes its bundle")
    write_json(summary_path, analyze_profile(manifest, absolute.parent))
    return summary_path


def main() -> int:
    args = parse_args()
    try:
        result = (
            capture(args)
            if args.action == "capture"
            else analyze_manifest(args.manifest)
        )
        kind = "manifest" if args.action == "capture" else "summary"
        print(f"profile {kind}: {result}")
    except (ProfileError, OSError, subprocess.SubprocessError, ValueError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
