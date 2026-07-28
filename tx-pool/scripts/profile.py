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
SCRIPT_SOURCE = Path(__file__).resolve()
REMAPPED_SOURCE_ROOT = "/ckb-txpool-profile-source"
MARKER_PREFIX = "TX_POOL_PROFILE_WINDOW "
PROFILE_FEATURES = ("internal", "profiling")
PROFILE_SCHEMA_VERSION = 1
MANIFEST_SCHEMA_VERSION = 2
SUMMARY_SCHEMA_VERSION = 1


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

    analyze = subparsers.add_parser(
        "analyze", help="verify a manifest and regenerate its deterministic summary"
    )
    analyze.add_argument("--manifest", type=Path, required=True)
    return parser.parse_args()


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


def artifact(path: Path) -> dict[str, Any]:
    resolved = path.resolve(strict=True)
    return {
        "path": str(resolved),
        "size_bytes": resolved.stat().st_size,
        "sha256": sha256_file(resolved),
    }


def verify_artifact(record: dict[str, Any], label: str) -> Path:
    required = {"path", "size_bytes", "sha256"}
    if set(record) != required:
        raise ProfileError(f"{label} artifact has an unsupported schema")
    path = Path(record["path"])
    if not path.is_absolute() or not path.is_file():
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
        "spans": Path(f"{absolute}.spans.log"),
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


def build_binary(target_dir: Path) -> tuple[Path, list[str], dict[str, str]]:
    command = [
        "cargo",
        "bench",
        "-p",
        "ckb-tx-pool",
        "--features",
        ",".join(PROFILE_FEATURES),
        "--bench",
        "pipeline",
        "--no-run",
        "--locked",
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
            and target.get("name") == "pipeline"
            and "bench" in target.get("kind", [])
            and executable
        ):
            executables.append(Path(executable).resolve())
    unique = sorted(set(executables))
    if len(unique) != 1 or not unique[0].is_file():
        raise ProfileError(
            f"Cargo reported {len(unique)} pipeline benchmark executables; expected one"
        )
    return unique[0], command, env


def scenario_environment(args: argparse.Namespace) -> dict[str, str]:
    dependent = args.tx_type.startswith("dependent_")
    if args.dependency_order == "child_first" and not dependent:
        raise ProfileError("child_first order requires a dependent transaction type")
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
    if args.binary is None:
        binary, build_command, build_env = build_binary(args.target_dir)
    else:
        binary = args.binary.expanduser().resolve(strict=True)
        if not binary.is_file() or not os.access(binary, os.X_OK):
            raise ProfileError(f"profile binary is not executable: {binary}")
        build_command = []
        build_env = os.environ.copy()

    env = scenario_environment(args)
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
        "--bench",
        "--noplot",
        "--discard-baseline",
        "--color",
        "never",
    ]
    started_utc = datetime.datetime.now(datetime.timezone.utc).isoformat()
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
    if not paths["profile"].is_file() or not paths["symbols"].is_file():
        raise ProfileError("Samply capture did not produce profile and symbol artifacts")

    # Span formatting is intentionally isolated from CPU sampling. Writing a
    # close record for every kernel acquisition is useful causal evidence, but
    # its subscriber locks and file I/O would otherwise manufacture the very
    # contention that Samply is meant to attribute.
    span_env = scenario_environment(args)
    span_env["TX_POOL_PROFILE_TRACE_PATH"] = str(paths["spans"])
    span_command = [
        str(binary),
        "--bench",
        "--noplot",
        "--discard-baseline",
        "--color",
        "never",
    ]
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
    if not paths["spans"].is_file():
        raise ProfileError("span capture did not produce its trace artifact")
    if span_marker["scenario"] != marker["scenario"]:
        raise ProfileError("CPU and span captures used different scenarios")

    manifest = {
        "schema_version": MANIFEST_SCHEMA_VERSION,
        "git": git_identity(),
        "features": list(PROFILE_FEATURES),
        "scenario": {
            "tx_type": args.tx_type,
            "pool_state": args.pool_state,
            "dependency_order": args.dependency_order,
            "peers": args.peers,
            "workers": args.workers,
            "size": args.size,
            "warm_pool_size": args.warm_pool_size,
        },
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
        },
        "span_capture": {
            "started_utc": span_started_utc,
            "ended_utc": span_ended_utc,
            "command": span_command,
            "window": span_marker,
            "isolation": "separate execution from CPU sampling",
        },
        "environment": environment_identity(paths["profile"].parent, build_env),
        "inputs": {
            "workspace_manifest_sha256": sha256_file(WORKSPACE_ROOT / "Cargo.toml"),
            "cargo_lock_sha256": sha256_file(WORKSPACE_ROOT / "Cargo.lock"),
            "tx_pool_manifest_sha256": sha256_file(
                WORKSPACE_ROOT / "tx-pool" / "Cargo.toml"
            ),
            "harness_sha256": files_sha256([BENCHMARK_SOURCE, SCRIPT_SOURCE]),
        },
        "artifacts": {
            "binary": artifact(binary),
            "profile": artifact(paths["profile"]),
            "symbols": artifact(paths["symbols"]),
            "stdout": artifact(paths["stdout"]),
            "stderr": artifact(paths["stderr"]),
            "spans": artifact(paths["spans"]),
            "span_stdout": artifact(paths["span_stdout"]),
            "span_stderr": artifact(paths["span_stderr"]),
        },
        "summary_path": str(paths["summary"]),
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


def ranked(counter: Counter[str], total: int, limit: int = 30) -> list[dict[str, Any]]:
    return [
        {
            "symbol": symbol,
            "samples": count,
            "percent_of_window_samples": round(100.0 * count / total, 4) if total else 0.0,
        }
        for symbol, count in sorted(counter.items(), key=lambda item: (-item[1], item[0]))[:limit]
    ]


def analyze_profile(manifest: dict[str, Any]) -> dict[str, Any]:
    artifacts = manifest.get("artifacts")
    if not isinstance(artifacts, dict):
        raise ProfileError("profile manifest has no artifact table")
    paths = {
        label: verify_artifact(record, label)
        for label, record in artifacts.items()
    }
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

    leaf: Counter[str] = Counter()
    inclusive: Counter[str] = Counter()
    thread_summaries: list[dict[str, Any]] = []
    total_samples = 0
    total_cpu_micros = 0
    for thread in threads:
        if not isinstance(thread, dict):
            raise ProfileError("Samply thread entry is not an object")
        samples = thread.get("samples")
        if not isinstance(samples, dict):
            raise ProfileError("Samply thread has no samples table")
        length = validate_parallel_table(
            samples, ("timeDeltas", "stack", "threadCPUDelta"), "samples"
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
            delta = samples["timeDeltas"][index]
            if not isinstance(delta, (int, float)) or delta < 0:
                raise ProfileError("Samply sample time delta is invalid")
            elapsed_ms += delta
            inside = window_start_ms <= elapsed_ms <= window_end_ms
            if inside:
                selected += 1
                stack_index = samples["stack"][index]
                if stack_index is not None:
                    frames = stack_frames(thread, stack_index)
                    names = [resolver.frame_name(thread, frame) for frame in frames]
                    if names:
                        leaf[names[0]] += 1
                        for name in set(names):
                            inclusive[name] += 1
                cpu_delta = samples["threadCPUDelta"][index]
                if not isinstance(cpu_delta, (int, float)) or cpu_delta < 0:
                    raise ProfileError("Samply thread CPU delta is invalid")
                # A CPU delta describes the interval since the previous sample.
                # Exclude an interval that crosses the benchmark window boundary.
                if previous_ms is not None and previous_ms >= window_start_ms:
                    selected_cpu += cpu_delta
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
    thread_summaries.sort(key=lambda item: (-item["samples"], str(item["tid"])))
    top_leaf = ranked(leaf, total_samples)
    return {
        "schema_version": SUMMARY_SCHEMA_VERSION,
        "scenario": manifest["scenario"],
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
            "cpu_boundary_policy": "exclude intervals whose previous sample precedes the window",
        },
        "threads": thread_summaries,
        "top_leaf_symbols": top_leaf,
        "top_inclusive_symbols": ranked(inclusive, total_samples),
    }


def analyze_manifest(manifest_path: Path) -> Path:
    absolute = manifest_path.expanduser().resolve(strict=True)
    manifest = read_json(absolute)
    if manifest.get("schema_version") != MANIFEST_SCHEMA_VERSION:
        raise ProfileError("profile manifest schema version is unsupported")
    summary_path = Path(manifest.get("summary_path", ""))
    if not summary_path.is_absolute():
        raise ProfileError("profile manifest summary path is not absolute")
    summary = analyze_profile(manifest)
    write_json(summary_path, summary)
    return summary_path


def main() -> int:
    args = parse_args()
    try:
        if args.action == "capture":
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
