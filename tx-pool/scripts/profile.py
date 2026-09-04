#!/usr/bin/env python3
"""Capture and deterministically analyze one tx-pool Samply profile."""

from __future__ import annotations

import argparse
import bisect
import gzip
import hashlib
import json
import os
import platform
import re
import subprocess
import sys
from collections import Counter
from pathlib import Path
from typing import Any


WORKSPACE_ROOT = Path(__file__).resolve().parents[2]
ONE_SHOT_SOURCE = WORKSPACE_ROOT / "tx-pool" / "benches" / "profile_one_shot.rs"
SCRIPT_SOURCE = Path(__file__).resolve()
REMAPPED_SOURCE_ROOT = "/ckb-txpool-profile-source"
MARKER_PREFIX = "TX_POOL_PROFILE_WINDOW "
OBSERVATION_PREFIX = "TX_POOL_PROFILE_OBSERVATION "
ONE_SHOT_FEATURES = ("profiling",)
PROFILE_SCHEMA_VERSION = 1
OBSERVATION_SCHEMA_VERSION = 2
MANIFEST_SCHEMA_VERSION = 5
SUMMARY_SCHEMA_VERSION = 4
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
) -> subprocess.CompletedProcess[str]:
    try:
        return subprocess.run(
            command,
            cwd=WORKSPACE_ROOT,
            env=env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=True,
        )
    except OSError as error:
        raise ProfileError(f"cannot execute {label}: {error}") from error
    except subprocess.CalledProcessError as error:
        detail = (error.stderr or error.stdout or "no process output").strip()
        raise ProfileError(f"{label} failed ({error.returncode}):\n{detail}") from error


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
    completed = run(command, env=env, label="profile binary build")
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
        or throughput <= 0
    ):
        raise ProfileError("profile observation throughput is invalid")
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
        observation["callback_duplicates"]
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
        stdout.write_text(completed.stdout)
        stderr.write_text(completed.stderr)
    except OSError as error:
        raise ProfileError(f"cannot save capture output: {error}") from error


def file_identity(path: Path) -> dict[str, Any]:
    path = path.resolve(strict=True)
    return {
        "path_at_capture": str(path),
        "size_bytes": path.stat().st_size,
        "sha256": sha256_file(path),
    }


def capture(args: argparse.Namespace) -> Path:
    paths = output_paths(args.output_prefix)
    prepare_outputs(paths, args.force)
    sources = [ONE_SHOT_SOURCE, SCRIPT_SOURCE]
    scenario = {name: getattr(args, name) for name in SCENARIO_FIELDS}
    runtime_args = [str(scenario[name]) for name in SCENARIO_FIELDS]
    runtime_env = os.environ.copy()
    runtime_env.pop("TX_POOL_PROFILE_TRACE_PATH", None)
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
    completed = run(command, env=runtime_env, label="Samply capture")
    save_output(completed, paths["stdout"], paths["stderr"])
    window = parse_marker(completed.stdout)
    observation = parse_observation(completed.stdout, scenario)
    if not paths["profile"].is_file() or not paths["symbols"].is_file():
        raise ProfileError("Samply did not produce profile and symbol artifacts")

    span_env = runtime_env.copy()
    span_env["TX_POOL_PROFILE_TRACE_PATH"] = str(paths["spans"])
    span_command = [str(binary), *runtime_args]
    span_completed = run(span_command, env=span_env, label="span capture")
    save_output(span_completed, paths["span_stdout"], paths["span_stderr"])
    span_window = parse_marker(span_completed.stdout)
    parse_observation(span_completed.stdout, scenario)
    if not paths["spans"].is_file():
        raise ProfileError("span capture did not produce its artifact")
    if span_window["scenario"] != window["scenario"]:
        raise ProfileError("CPU and span captures used different scenarios")

    manifest = {
        "schema_version": MANIFEST_SCHEMA_VERSION,
        "git": git_identity(),
        "harness": "profile_one_shot",
        "features": list(ONE_SHOT_FEATURES),
        "scenario": scenario,
        "observation": observation,
        "window": window,
        "capture": {
            "sample_rate_hz": args.rate,
            "command": command,
            "build_command": build_command,
            "build_profile": FINAL_BUILD_PROFILE,
        },
        "span_capture": {"command": span_command, "window": span_window},
        "environment": environment_identity(build_env),
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
            "binary": file_identity(binary),
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
        {"symbol": symbol, "thread_cpu_delta_micros": round(cpu, 3)}
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
    if not isinstance(start_time, (int, float)) or not isinstance(
        interval, (int, float)
    ):
        raise ProfileError("Samply timing metadata is invalid")
    if meta.get("sampleUnits", {}).get("threadCPUDelta") != "µs":
        raise ProfileError("Samply CPU deltas are not microseconds")
    start = window["start_unix_nanos"] / 1_000_000 - start_time
    end = window["end_unix_nanos"] / 1_000_000 - start_time
    if start < 0 or start >= end:
        raise ProfileError("target window falls before the Samply profile")

    leaf: Counter[str] = Counter()
    inclusive: Counter[str] = Counter()
    samples_in_window = 0
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
                cpu = samples["threadCPUDelta"][index]
                if (
                    not isinstance(cpu, (int, float))
                    or isinstance(cpu, bool)
                    or cpu < 0
                ):
                    raise ProfileError("Samply thread CPU delta is invalid")
                if previous is not None and previous >= start:
                    complete_cpu += cpu
                    stack = samples["stack"][index]
                    names = (
                        [] if stack is None else stack_names(thread, stack, resolver)
                    )
                    if names:
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
        },
        "leaf_hotspots": ranked(leaf),
        "inclusive_hotspots": ranked(inclusive),
    }


def analyze_spans(manifest: dict[str, Any], path: Path) -> dict[str, Any]:
    counters = read_json(path)
    expected_window = manifest.get("span_capture", {}).get("window")
    if set(counters) != {"schema_version", "measurement", "window", "spans"} or (
        counters["schema_version"],
        counters["measurement"],
    ) != (2, "span_lifetimes_started_during_target_work"):
        raise ProfileError("span artifact schema is unsupported")
    if counters["window"] != expected_window:
        raise ProfileError("span artifact and manifest windows differ")
    spans = counters["spans"]
    if not isinstance(spans, list) or not spans:
        raise ProfileError("span artifact is empty")
    names = []
    for span in spans:
        if not isinstance(span, dict) or set(span) != {
            "name",
            "start_count",
            "elapsed_nanos",
        }:
            raise ProfileError("span entry schema is unsupported")
        if (
            not isinstance(span["name"], str)
            or not span["name"].startswith("tx_pool.")
            or type(span["start_count"]) is not int
            or span["start_count"] < 0
            or type(span["elapsed_nanos"]) is not int
            or span["elapsed_nanos"] < 0
        ):
            raise ProfileError("span entry is invalid")
        names.append(span["name"])
    if names != sorted(set(names)):
        raise ProfileError("span names must be unique and sorted")
    by_name = {span["name"]: span["start_count"] for span in spans}
    if by_name.get("tx_pool.ingress.remote_batch", 0) <= 0:
        raise ProfileError("profile did not traverse production remote-batch ingress")
    starts = sum(span["start_count"] for span in spans)
    if starts == 0:
        raise ProfileError("span artifact contains no target-window work")
    return {
        "window": counters["window"],
        "measurement": counters["measurement"],
        "total_starts": starts,
        "total_elapsed_nanos": sum(span["elapsed_nanos"] for span in spans),
        "spans": spans,
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
    if observation != manifest.get("observation"):
        raise ProfileError("profile observation differs from its manifest")
    parse_observation(span_stdout, expected)
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
    sources = [ONE_SHOT_SOURCE, SCRIPT_SOURCE]
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
