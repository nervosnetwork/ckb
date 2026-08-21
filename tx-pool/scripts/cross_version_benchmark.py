#!/usr/bin/env python3
"""Run reproducible paired fixed-binary tx-pool cross-version benchmarks."""

from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import math
import os
import platform
import re
import resource
import shlex
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

RESULT = re.compile(
    r"^BENCH_RESULT scenario=(?P<scenario>\S+) target=(?P<target>\d+) "
    r"warm=(?P<warm>\d+) workers=(?P<workers>\d+) peers=(?P<peers>\d+) "
    r"elapsed_ns=(?P<elapsed_ns>\d+) throughput_tps=(?P<throughput>[0-9.]+) "
    r"accepted=(?P<accepted>\d+) p99_latency_ns=(?P<p99_latency_ns>\d+) "
    r"target_cpu_ns=(?P<target_cpu_ns>\d+) "
    r"allocation_calls=(?P<allocation_calls>\d+) "
    r"allocated_bytes=(?P<allocated_bytes>\d+) "
    r"reorg_latency_ns=(?P<reorg_latency_ns>\d+) "
    r"reorg_overlap_callbacks=(?P<reorg_overlap_callbacks>\d+) "
    r"shutdown_latency_ns=(?P<shutdown_latency_ns>\d+)$",
    re.MULTILINE,
)
WINDOW = re.compile(
    r"^PROFILE_WINDOW start_unix_ns=(?P<start>\d+) end_unix_ns=(?P<end>\d+)$",
    re.MULTILINE,
)
RESOURCE_RESULT = re.compile(
    r"^RESOURCE_RESULT user_cpu_ns=(?P<user_cpu_ns>\d+) "
    r"system_cpu_ns=(?P<system_cpu_ns>\d+) max_rss_bytes=(?P<max_rss_bytes>\d+) "
    r"voluntary_context_switches=(?P<voluntary_context_switches>\d+) "
    r"involuntary_context_switches=(?P<involuntary_context_switches>\d+)$",
    re.MULTILINE,
)
MIN_CLOCK_TOLERANCE_NS = 1_000_000
CLOCK_TOLERANCE_DIVISOR = 10_000
MAX_SCENARIO_TRANSACTIONS = 32_768


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def command_output(command: list[str], root: Path | None = None) -> str:
    try:
        return subprocess.check_output(
            command,
            cwd=root,
            text=True,
            stderr=subprocess.STDOUT,
            timeout=10,
        ).strip()
    except (OSError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
        return f"unavailable: {type(error).__name__}"


def measure_child() -> int:
    """Isolate per-attempt descendant CPU/RSS accounting in one short process."""

    command = sys.argv[2:]
    if not command:
        raise RuntimeError("resource measurement wrapper has no child command")
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    completed = subprocess.run(
        command,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
    )
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    sys.stdout.buffer.write(completed.stdout)
    rss_scale = 1 if sys.platform == "darwin" else 1024
    print(
        "RESOURCE_RESULT "
        f"user_cpu_ns={round((after.ru_utime - before.ru_utime) * 1e9)} "
        f"system_cpu_ns={round((after.ru_stime - before.ru_stime) * 1e9)} "
        f"max_rss_bytes={round(after.ru_maxrss * rss_scale)} "
        f"voluntary_context_switches={after.ru_nvcsw - before.ru_nvcsw} "
        f"involuntary_context_switches={after.ru_nivcsw - before.ru_nivcsw}"
    )
    return completed.returncode


def git_record(root: Path) -> dict[str, str]:
    record = {
        "root": str(root.resolve()),
        "commit": command_output(["git", "rev-parse", "HEAD"], root),
        "status": command_output(["git", "status", "--short"], root),
        "cargo_lock_sha256": sha256(root / "Cargo.lock"),
        "cargo_manifest_sha256": sha256(root / "Cargo.toml"),
        "tx_pool_manifest_sha256": sha256(root / "tx-pool" / "Cargo.toml"),
    }
    if record["commit"].startswith("unavailable:"):
        raise RuntimeError(f"cannot identify Git revision for {root}")
    if record["status"].startswith("unavailable:"):
        raise RuntimeError(f"cannot inspect Git status for {root}")
    if record["status"]:
        raise RuntimeError(f"measurement worktree is dirty: {root}")
    return record


def binary_record(path: Path) -> dict[str, object]:
    resolved = path.resolve()
    if not resolved.is_file():
        raise RuntimeError(f"fixed binary does not exist: {resolved}")
    return {
        "path": str(resolved),
        "sha256": sha256(resolved),
        "size": resolved.stat().st_size,
    }


def build_binary(
    root: Path, target_dir: Path, features: str
) -> tuple[dict[str, object], dict[str, object]]:
    root = root.resolve()
    target_dir = target_dir.resolve()
    command = [
        "cargo",
        "bench",
        "-p",
        "ckb-tx-pool",
        "--bench",
        "profile_one_shot",
        "--no-run",
        "--locked",
        "--message-format=json",
    ]
    if features:
        command.extend(("--features", features))
    environment = os.environ.copy()
    environment["CARGO_TARGET_DIR"] = str(target_dir)
    environment["CARGO_INCREMENTAL"] = "0"
    remap = f"--remap-path-prefix={root}=/ckb-txpool-cross-source"
    encoded = environment.get("CARGO_ENCODED_RUSTFLAGS")
    if encoded is not None:
        inherited_flags = encoded.split("\x1f") if encoded else []
    else:
        inherited_flags = shlex.split(environment.get("RUSTFLAGS", ""))
    effective_flags = [*inherited_flags, remap]
    environment.pop("RUSTFLAGS", None)
    environment["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(effective_flags)
    completed = subprocess.run(
        command,
        cwd=root,
        env=environment,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if completed.returncode != 0:
        raise RuntimeError(
            f"cross-version benchmark build failed ({completed.returncode}):\n"
            f"stdout tail:\n{completed.stdout[-4_000:].strip()}\n"
            f"stderr tail:\n{completed.stderr[-4_000:].strip()}"
        )
    executables: set[Path] = set()
    for line in completed.stdout.splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        target = message.get("target", {})
        executable = message.get("executable")
        if (
            message.get("reason") == "compiler-artifact"
            and target.get("name") == "profile_one_shot"
            and "bench" in target.get("kind", [])
            and executable
        ):
            executables.add(Path(executable).resolve())
    if len(executables) != 1:
        raise RuntimeError(
            "Cargo did not report exactly one profile_one_shot executable"
        )
    binary = binary_record(executables.pop())
    return binary, {
        "command": command,
        "target_dir": str(target_dir),
        "features": features,
        "cargo_incremental": environment["CARGO_INCREMENTAL"],
        "inherited_rustflags": inherited_flags,
        "logical_rustflags": [
            *inherited_flags,
            "--remap-path-prefix=<SOURCE_ROOT>=/ckb-txpool-cross-source",
        ],
    }


def prepare_binary(
    root: Path, supplied: Path | None, target_dir: Path | None, features: str
) -> tuple[dict[str, object], dict[str, object]]:
    if supplied is not None:
        return binary_record(supplied), {"provenance": "supplied_by_sha256"}
    selected_target = target_dir or root / "target" / "tx-pool-cross"
    binary, build = build_binary(root, selected_target, features)
    build["provenance"] = "built_once_by_runner"
    return binary, build


def parse_scenario(value: str) -> dict[str, object]:
    fields = value.split(",")
    if len(fields) != 5:
        raise ValueError(f"invalid scenario: {value}")
    name = fields[0]
    values = [int(field) for field in fields[1:]]
    target, warm, workers, peers = values
    if (
        not name
        or target <= 0
        or warm < 0
        or workers <= 0
        or peers <= 0
        or target + warm > MAX_SCENARIO_TRANSACTIONS
    ):
        raise ValueError(f"invalid scenario: {value}")
    return dict(zip(("name", "target", "warm", "workers", "peers"), [name, *values]))


def relative_mad(values: list[float]) -> float:
    median = statistics.median(values)
    if median == 0:
        return 0.0
    return statistics.median(abs(value - median) for value in values) / median * 100.0


def spread(values: list[float]) -> float:
    median = statistics.median(values)
    if median == 0:
        return 0.0
    return (max(values) - min(values)) / median * 100.0


def cool(seconds: float) -> None:
    if seconds:
        time.sleep(seconds)


def write_checkpoint(path: Path, record: dict[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(record, indent=2, sort_keys=True) + "\n")
    temporary.replace(path)


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--baseline-binary",
        type=Path,
        help="reuse this immutable binary instead of building the baseline once",
    )
    parser.add_argument(
        "--candidate-binary",
        type=Path,
        help="reuse this immutable binary instead of building the candidate once",
    )
    parser.add_argument("--baseline-root", type=Path, required=True)
    parser.add_argument("--candidate-root", type=Path, required=True)
    parser.add_argument(
        "--baseline-target-dir",
        type=Path,
        help="isolated Cargo target used only when building the baseline",
    )
    parser.add_argument(
        "--candidate-target-dir",
        type=Path,
        help="isolated Cargo target used only when building the candidate",
    )
    parser.add_argument("--baseline-build-features", default="")
    parser.add_argument("--candidate-build-features", default="")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--runs", type=int, default=10)
    parser.add_argument(
        "--replicates-per-sample",
        type=int,
        default=1,
        help=(
            "repeat each side inside one recorded sample; values above one "
            "must be even so every sample contains balanced AB/BA order"
        ),
    )
    parser.add_argument("--initial-cooldown-seconds", type=float, default=15.0)
    parser.add_argument("--cooldown-seconds", type=float, default=10.0)
    parser.add_argument("--max-paired-mad-percent", type=float, default=1.5)
    parser.add_argument("--timeout-seconds", type=float, default=180.0)
    parser.add_argument(
        "--allow-noncomparable",
        action="store_true",
        help="record intentional capability failures without a nonzero exit",
    )
    parser.add_argument(
        "--scenario",
        action="append",
        required=True,
        metavar="NAME,TARGET,WARM,WORKERS,PEERS",
    )
    args = parser.parse_args()
    if args.runs < 6 or args.runs % 2:
        parser.error("--runs must be an even value of at least 6")
    if (
        args.replicates_per_sample <= 0
        or args.replicates_per_sample > 8
        or (
            args.replicates_per_sample != 1
            and args.replicates_per_sample % 2
        )
    ):
        parser.error(
            "--replicates-per-sample must be 1 or an even value from 2 to 8"
        )
    if (
        args.initial_cooldown_seconds < 0
        or args.cooldown_seconds < 0
        or args.timeout_seconds <= 0
        or args.max_paired_mad_percent <= 0
    ):
        parser.error("cooldowns must be non-negative and limits must be positive")
    if args.output.exists():
        parser.error("--output already exists; preserve it and choose a new artifact")
    if args.baseline_binary is not None and args.baseline_target_dir is not None:
        parser.error("--baseline-target-dir cannot accompany --baseline-binary")
    if args.candidate_binary is not None and args.candidate_target_dir is not None:
        parser.error("--candidate-target-dir cannot accompany --candidate-binary")
    return args


def scenario_key(scenario: dict[str, object]) -> str:
    return (
        f"{scenario['name']}-t{scenario['target']}-w{scenario['warm']}-"
        f"v{scenario['workers']}-p{scenario['peers']}"
    )


def failure_attempt(
    *,
    side: str,
    phase: str,
    command: list[str],
    started: int,
    ended: int,
    category: str,
    detail: str,
    output: str,
) -> dict[str, object]:
    return {
        "outcome": "failure",
        "side": side,
        "phase": phase,
        "command": command,
        "process_started_unix_ns": started,
        "process_ended_unix_ns": ended,
        "category": category,
        "detail": detail,
        "output": output,
    }


def timeout_output(error: subprocess.TimeoutExpired) -> str:
    output = error.stdout or ""
    if isinstance(output, bytes):
        return output.decode(errors="replace")
    return output


def run_attempt(
    binary: dict[str, object],
    root: Path,
    scenario: dict[str, object],
    side: str,
    phase: str,
    timeout: float,
) -> dict[str, object]:
    path = Path(str(binary["path"]))
    if sha256(path) != binary["sha256"]:
        raise RuntimeError(f"{side} binary changed before {phase}")
    benchmark_command = [
        str(path),
        str(scenario["name"]),
        str(scenario["target"]),
        str(scenario["warm"]),
        str(scenario["workers"]),
        str(scenario["peers"]),
    ]
    trace_directory = tempfile.TemporaryDirectory(prefix="ckb-txpool-span-")
    span_path = Path(trace_directory.name) / "spans.json"
    command = [
        sys.executable,
        str(Path(__file__).resolve()),
        "__measure_child__",
        *benchmark_command,
    ]
    child_environment = os.environ.copy()
    child_environment["TX_POOL_PROFILE_TRACE_PATH"] = str(span_path)
    started = time.time_ns()
    monotonic_started = time.monotonic_ns()
    try:
        completed = subprocess.run(
            command,
            cwd=root,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            timeout=timeout,
            env=child_environment,
        )
    except subprocess.TimeoutExpired as error:
        ended = time.time_ns()
        return failure_attempt(
            side=side,
            phase=phase,
            command=command,
            started=started,
            ended=ended,
            category="runner_timeout",
            detail=f"process exceeded {timeout:.3f} seconds",
            output=timeout_output(error),
        )
    except OSError as error:
        ended = time.time_ns()
        return failure_attempt(
            side=side,
            phase=phase,
            command=command,
            started=started,
            ended=ended,
            category="spawn_failure",
            detail=str(error),
            output="",
        )
    monotonic_ended = time.monotonic_ns()
    ended = time.time_ns()
    if completed.returncode != 0:
        trace_directory.cleanup()
        return failure_attempt(
            side=side,
            phase=phase,
            command=command,
            started=started,
            ended=ended,
            category="nonzero_exit",
            detail=f"process exited with status {completed.returncode}",
            output=completed.stdout,
        )
    results = list(RESULT.finditer(completed.stdout))
    windows = list(WINDOW.finditer(completed.stdout))
    resources = list(RESOURCE_RESULT.finditer(completed.stdout))
    try:
        spans = json.loads(span_path.read_text())
    except (OSError, json.JSONDecodeError):
        spans = None
    trace_directory.cleanup()
    if (
        len(results) != 1
        or len(windows) != 1
        or len(resources) != 1
        or not isinstance(spans, dict)
    ):
        return failure_attempt(
            side=side,
            phase=phase,
            command=command,
            started=started,
            ended=ended,
            category="invalid_evidence",
            detail=(
                f"observed {len(results)} results, {len(windows)} windows and "
                f"{len(resources)} resource records and "
                f"{'one' if isinstance(spans, dict) else 'no'} span record"
            ),
            output=completed.stdout,
        )
    parsed = results[0].groupdict()
    observed = {
        "name": parsed["scenario"],
        "target": int(parsed["target"]),
        "warm": int(parsed["warm"]),
        "workers": int(parsed["workers"]),
        "peers": int(parsed["peers"]),
    }
    expected_accepted = int(scenario["target"]) + int(scenario["warm"])
    accepted = int(parsed["accepted"])
    p99_latency_ns = int(parsed["p99_latency_ns"])
    target_cpu_ns = int(parsed["target_cpu_ns"])
    allocation_calls = int(parsed["allocation_calls"])
    allocated_bytes = int(parsed["allocated_bytes"])
    reorg_latency_ns = int(parsed["reorg_latency_ns"])
    reorg_overlap_callbacks = int(parsed["reorg_overlap_callbacks"])
    shutdown_latency_ns = int(parsed["shutdown_latency_ns"])
    resource_record = resources[0].groupdict()
    peak_rss_bytes = int(resource_record["max_rss_bytes"])
    span_rows = spans.get("spans") if isinstance(spans, dict) else None
    authority_span_elapsed_nanos = {
        row.get("name"): row.get("elapsed_nanos")
        for row in span_rows or []
        if isinstance(row, dict)
    }
    authority_span_start_counts = {
        row.get("name"): row.get("start_count")
        for row in span_rows or []
        if isinstance(row, dict)
    }
    required_lock_spans = {
        "tx_pool.authority.read_wait",
        "tx_pool.authority.read_hold",
        "tx_pool.authority.write_wait",
        "tx_pool.authority.write_hold",
    }
    window = windows[0].groupdict()
    elapsed_ns = int(parsed["elapsed_ns"])
    wall_window_ns = int(window["end"]) - int(window["start"])
    clock_delta_ns = wall_window_ns - elapsed_ns
    clock_tolerance_ns = max(
        MIN_CLOCK_TOLERANCE_NS, elapsed_ns // CLOCK_TOLERANCE_DIVISOR
    )
    profile_window_status = (
        "aligned"
        if abs(clock_delta_ns) <= clock_tolerance_ns
        else "scheduler_widened"
    )
    evidence_error = None
    if observed != scenario:
        evidence_error = f"scenario drift: {observed} != {scenario}"
    elif accepted != expected_accepted:
        evidence_error = f"accepted {accepted}, expected {expected_accepted}"
    elif wall_window_ns <= 0:
        evidence_error = "target wall-clock window is not monotonic"
    elif p99_latency_ns <= 0:
        evidence_error = "target p99 latency is not positive"
    elif target_cpu_ns <= 0:
        evidence_error = "target-window process CPU time is not positive"
    elif allocation_calls <= 0 or allocated_bytes <= 0:
        evidence_error = "target allocation observation is not positive"
    elif peak_rss_bytes <= 0:
        evidence_error = "process peak RSS is not positive"
    elif reorg_latency_ns <= 0 or shutdown_latency_ns <= 0:
        evidence_error = "reorg/shutdown latency observation is not positive"
    elif (observed["name"] == "reorg_in_flight") != (reorg_overlap_callbacks > 0):
        evidence_error = "reorg/callback overlap observation differs from the scenario"
    elif spans.get("schema_version") != 2 or spans.get(
        "measurement"
    ) != "span_lifetimes_started_during_target_work":
        evidence_error = "authority span observation uses an unsupported schema"
    elif not required_lock_spans.issubset(authority_span_elapsed_nanos):
        evidence_error = "authority lock span observation is incomplete"
    elif any(
        not isinstance(authority_span_start_counts.get(name), int)
        or isinstance(authority_span_start_counts.get(name), bool)
        or authority_span_start_counts[name] <= 0
        or not isinstance(authority_span_elapsed_nanos.get(name), int)
        or isinstance(authority_span_elapsed_nanos.get(name), bool)
        or authority_span_elapsed_nanos[name] <= 0
        for name in required_lock_spans
    ):
        evidence_error = "authority lock span observation is empty or invalid"
    elif clock_delta_ns < -clock_tolerance_ns:
        evidence_error = (
            f"target wall-clock window is shorter by {-clock_delta_ns}ns, exceeding "
            f"{clock_tolerance_ns}ns tolerance"
        )
    if evidence_error is not None:
        return failure_attempt(
            side=side,
            phase=phase,
            command=command,
            started=started,
            ended=ended,
            category="invalid_evidence",
            detail=evidence_error,
            output=completed.stdout,
        )
    child_user_cpu_ns = int(resource_record["user_cpu_ns"])
    child_system_cpu_ns = int(resource_record["system_cpu_ns"])
    child_cpu_ns = child_user_cpu_ns + child_system_cpu_ns
    process_wall_ns = monotonic_ended - monotonic_started
    voluntary_context_switches = int(resource_record["voluntary_context_switches"])
    involuntary_context_switches = int(resource_record["involuntary_context_switches"])
    return {
        "outcome": "success",
        "side": side,
        "phase": phase,
        "command": command,
        "process_started_unix_ns": started,
        "process_ended_unix_ns": ended,
        "process_wall_ns": process_wall_ns,
        "child_user_cpu_ns": child_user_cpu_ns,
        "child_system_cpu_ns": child_system_cpu_ns,
        "child_cpu_ns": child_cpu_ns,
        "child_cpu_parallelism": child_cpu_ns / process_wall_ns,
        "target_cpu_ns": target_cpu_ns,
        "cpu_time_per_transaction_ns": target_cpu_ns / int(scenario["target"]),
        "child_voluntary_context_switches": voluntary_context_switches,
        "child_involuntary_context_switches": involuntary_context_switches,
        "child_voluntary_context_switches_per_second": (
            voluntary_context_switches * 1e9 / process_wall_ns
        ),
        "child_involuntary_context_switches_per_second": (
            involuntary_context_switches * 1e9 / process_wall_ns
        ),
        "target_started_unix_ns": int(window["start"]),
        "target_ended_unix_ns": int(window["end"]),
        "elapsed_ns": elapsed_ns,
        "wall_window_ns": wall_window_ns,
        "clock_domain_delta_ns": clock_delta_ns,
        "clock_domain_tolerance_ns": clock_tolerance_ns,
        "profile_window_status": profile_window_status,
        "throughput_tps": float(parsed["throughput"]),
        "accepted": accepted,
        "p99_latency_ns": p99_latency_ns,
        "allocation_calls": allocation_calls,
        "allocated_bytes": allocated_bytes,
        "peak_rss_bytes": peak_rss_bytes,
        "reorg_latency_ns": reorg_latency_ns,
        "reorg_overlap_callbacks": reorg_overlap_callbacks,
        "shutdown_latency_ns": shutdown_latency_ns,
        "authority_span_elapsed_nanos": authority_span_elapsed_nanos,
        "authority_span_start_counts": authority_span_start_counts,
        "allocation_calls_per_transaction": allocation_calls / int(scenario["target"]),
        "allocated_bytes_per_transaction": allocated_bytes / int(scenario["target"]),
        "output": completed.stdout,
    }


def aggregate_side(
    attempts: list[dict[str, object]], target_per_attempt: int
) -> dict[str, object]:
    elapsed_ns = sum(int(attempt["elapsed_ns"]) for attempt in attempts)
    process_wall_ns = sum(int(attempt["process_wall_ns"]) for attempt in attempts)
    child_user_cpu_ns = sum(
        int(attempt["child_user_cpu_ns"]) for attempt in attempts
    )
    child_system_cpu_ns = sum(
        int(attempt["child_system_cpu_ns"]) for attempt in attempts
    )
    child_cpu_ns = child_user_cpu_ns + child_system_cpu_ns
    target_cpu_ns = sum(int(attempt["target_cpu_ns"]) for attempt in attempts)
    voluntary_context_switches = sum(
        int(attempt["child_voluntary_context_switches"]) for attempt in attempts
    )
    involuntary_context_switches = sum(
        int(attempt["child_involuntary_context_switches"]) for attempt in attempts
    )
    target = target_per_attempt * len(attempts)
    p99_latency_ns = max(int(attempt["p99_latency_ns"]) for attempt in attempts)
    allocation_calls = sum(int(attempt["allocation_calls"]) for attempt in attempts)
    allocated_bytes = sum(int(attempt["allocated_bytes"]) for attempt in attempts)
    peak_rss_bytes = max(int(attempt["peak_rss_bytes"]) for attempt in attempts)
    reorg_latency_ns = max(int(attempt["reorg_latency_ns"]) for attempt in attempts)
    reorg_overlap_callbacks = max(
        int(attempt["reorg_overlap_callbacks"]) for attempt in attempts
    )
    shutdown_latency_ns = max(int(attempt["shutdown_latency_ns"]) for attempt in attempts)
    authority_span_elapsed_nanos: dict[str, int] = {}
    for attempt in attempts:
        for name, elapsed in attempt["authority_span_elapsed_nanos"].items():
            authority_span_elapsed_nanos[name] = (
                authority_span_elapsed_nanos.get(name, 0) + int(elapsed)
            )
    return {
        "attempts": len(attempts),
        "target": target,
        "elapsed_ns": elapsed_ns,
        "throughput_tps": target * 1e9 / elapsed_ns,
        "process_wall_ns": process_wall_ns,
        "child_user_cpu_ns": child_user_cpu_ns,
        "child_system_cpu_ns": child_system_cpu_ns,
        "child_cpu_ns": child_cpu_ns,
        "child_cpu_parallelism": child_cpu_ns / process_wall_ns,
        "target_cpu_ns": target_cpu_ns,
        "cpu_time_per_transaction_ns": target_cpu_ns / target,
        "child_voluntary_context_switches": voluntary_context_switches,
        "child_involuntary_context_switches": involuntary_context_switches,
        "child_voluntary_context_switches_per_second": (
            voluntary_context_switches * 1e9 / process_wall_ns
        ),
        "child_involuntary_context_switches_per_second": (
            involuntary_context_switches * 1e9 / process_wall_ns
        ),
        "p99_latency_ns": p99_latency_ns,
        "allocation_calls": allocation_calls,
        "allocated_bytes": allocated_bytes,
        "peak_rss_bytes": peak_rss_bytes,
        "reorg_latency_ns": reorg_latency_ns,
        "reorg_overlap_callbacks": reorg_overlap_callbacks,
        "shutdown_latency_ns": shutdown_latency_ns,
        "authority_span_elapsed_nanos": dict(sorted(authority_span_elapsed_nanos.items())),
        "allocation_calls_per_transaction": allocation_calls / target,
        "allocated_bytes_per_transaction": allocated_bytes / target,
    }


def main() -> None:
    args = arguments()
    baseline_root = args.baseline_root.resolve()
    candidate_root = args.candidate_root.resolve()
    if baseline_root == candidate_root:
        raise RuntimeError("baseline and candidate source roots must be distinct")
    if len(str(baseline_root).encode()) != len(str(candidate_root).encode()):
        raise RuntimeError("measurement worktree paths must have equal UTF-8 length")
    baseline_source = git_record(baseline_root)
    candidate_source = git_record(candidate_root)
    scenarios = [parse_scenario(value) for value in args.scenario]
    harness_relative = Path("tx-pool/benches/profile_one_shot.rs")
    harness_hash = sha256(baseline_root / harness_relative)
    if sha256(candidate_root / harness_relative) != harness_hash:
        raise RuntimeError("baseline and candidate harnesses differ")
    baseline_target = args.baseline_target_dir or (
        baseline_root / "target" / "tx-pool-cross"
    )
    candidate_target = args.candidate_target_dir or (
        candidate_root / "target" / "tx-pool-cross"
    )
    if args.baseline_binary is None and args.candidate_binary is None:
        if len(str(baseline_target.resolve()).encode()) != len(
            str(candidate_target.resolve()).encode()
        ):
            raise RuntimeError("measurement target paths must have equal UTF-8 length")
        if baseline_target.resolve() == candidate_target.resolve():
            raise RuntimeError("baseline and candidate must use isolated target paths")
    baseline, baseline_build = prepare_binary(
        baseline_root,
        args.baseline_binary,
        args.baseline_target_dir,
        args.baseline_build_features,
    )
    candidate, candidate_build = prepare_binary(
        candidate_root,
        args.candidate_binary,
        args.candidate_target_dir,
        args.candidate_build_features,
    )
    record: dict[str, object] = {
        "schema": 3,
        "created_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "runner_sha256": sha256(Path(__file__)),
        "harness_sha256": harness_hash,
        "baseline_source": baseline_source,
        "candidate_source": candidate_source,
        "baseline_binary": baseline,
        "candidate_binary": candidate,
        "baseline_build": baseline_build,
        "candidate_build": candidate_build,
        "environment": {
            "platform": platform.platform(),
            "machine": platform.machine(),
            "python": platform.python_version(),
            "cpu_count": os.cpu_count(),
            "rustc": command_output(["rustc", "-Vv"]),
            "cargo": command_output(["cargo", "-V"]),
            "rustflags": os.environ.get("RUSTFLAGS", ""),
            "cargo_encoded_rustflags": os.environ.get(
                "CARGO_ENCODED_RUSTFLAGS", ""
            ),
            "battery": command_output(["pmset", "-g", "batt"]),
            "thermal": command_output(["pmset", "-g", "therm"]),
        },
        "runs": args.runs,
        "replicates_per_sample": args.replicates_per_sample,
        "initial_cooldown_seconds": args.initial_cooldown_seconds,
        "cooldown_seconds": args.cooldown_seconds,
        "max_paired_mad_percent": args.max_paired_mad_percent,
        "timeout_seconds": args.timeout_seconds,
        "scenarios": scenarios,
        "attempts": [],
        "measurements": [],
        "failures": [],
        "summary": {},
    }
    for field in ("rustc", "cargo"):
        if str(record["environment"][field]).startswith("unavailable:"):
            raise RuntimeError(f"required environment identity is unavailable: {field}")
    write_checkpoint(args.output, record)
    cool(args.initial_cooldown_seconds)

    attempts: list[dict[str, object]] = []
    measurements: list[dict[str, object]] = []
    failures: list[dict[str, object]] = []
    summaries: dict[str, object] = {}

    def checkpoint(attempt: dict[str, object]) -> None:
        attempts.append(attempt)
        if attempt["outcome"] == "failure":
            failures.append(attempt)
        elif not str(attempt["phase"]).startswith("pilot-"):
            measurements.append(attempt)
        record["attempts"] = attempts
        record["measurements"] = measurements
        record["failures"] = failures
        record["summary"] = summaries
        write_checkpoint(args.output, record)

    for scenario in scenarios:
        key = scenario_key(scenario)
        print(f">>> pilot {key}: candidate then baseline", flush=True)
        pilot_failed = False
        for side, binary, root in (
            ("candidate", candidate, candidate_root),
            ("baseline", baseline, baseline_root),
        ):
            attempt = run_attempt(
                binary, root, scenario, side, f"pilot-{key}", args.timeout_seconds
            )
            checkpoint(attempt)
            cool(args.cooldown_seconds)
            if attempt["outcome"] == "failure":
                summaries[key] = {
                    "status": "non_comparable",
                    "reason": "pilot_failure",
                    "failed_side": side,
                    "failure_category": attempt["category"],
                }
                record["summary"] = summaries
                write_checkpoint(args.output, record)
                pilot_failed = True
                break
        if pilot_failed:
            continue

        paired_ratios: list[float] = []
        paired_cpu_ratios: list[float] = []
        paired_p99_ratios: list[float] = []
        paired_allocation_call_ratios: list[float] = []
        paired_allocation_byte_ratios: list[float] = []
        paired_peak_rss_ratios: list[float] = []
        paired_reorg_latency_ratios: list[float] = []
        paired_shutdown_latency_ratios: list[float] = []
        baseline_rates: list[float] = []
        candidate_rates: list[float] = []
        baseline_cpu_parallelism: list[float] = []
        candidate_cpu_parallelism: list[float] = []
        baseline_involuntary_switch_rates: list[float] = []
        candidate_involuntary_switch_rates: list[float] = []
        paired_samples: list[dict[str, object]] = []
        measurement_failed = False
        for run_index in range(args.runs):
            pair: dict[str, list[dict[str, object]]] = {
                "baseline": [],
                "candidate": [],
            }
            for replicate_index in range(args.replicates_per_sample):
                order = [
                    ("baseline", baseline, baseline_root),
                    ("candidate", candidate, candidate_root),
                ]
                if (run_index + replicate_index) % 2:
                    order.reverse()
                for side, binary, root in order:
                    phase = f"{key}-pair-{run_index + 1}"
                    if args.replicates_per_sample > 1:
                        phase += f"-replicate-{replicate_index + 1}"
                    print(f">>> {phase}: {side}", flush=True)
                    attempt = run_attempt(
                        binary,
                        root,
                        scenario,
                        side,
                        phase,
                        args.timeout_seconds,
                    )
                    checkpoint(attempt)
                    cool(args.cooldown_seconds)
                    if attempt["outcome"] == "failure":
                        summaries[key] = {
                            "status": "non_comparable",
                            "reason": "measurement_failure",
                            "failed_side": side,
                            "failed_pair": run_index + 1,
                            "failed_replicate": replicate_index + 1,
                            "failure_category": attempt["category"],
                        }
                        record["summary"] = summaries
                        write_checkpoint(args.output, record)
                        measurement_failed = True
                        break
                    pair[side].append(attempt)
                if measurement_failed:
                    break
            if measurement_failed:
                break
            baseline_sample = aggregate_side(
                pair["baseline"], int(scenario["target"])
            )
            candidate_sample = aggregate_side(
                pair["candidate"], int(scenario["target"])
            )
            baseline_rate = float(baseline_sample["throughput_tps"])
            candidate_rate = float(candidate_sample["throughput_tps"])
            baseline_rates.append(baseline_rate)
            candidate_rates.append(candidate_rate)
            throughput_ratio = candidate_rate / baseline_rate
            target_cpu_ratio = float(candidate_sample["target_cpu_ns"]) / float(
                baseline_sample["target_cpu_ns"]
            )
            paired_ratios.append(throughput_ratio)
            paired_cpu_ratios.append(target_cpu_ratio)
            paired_p99_ratios.append(
                float(candidate_sample["p99_latency_ns"])
                / float(baseline_sample["p99_latency_ns"])
            )
            paired_allocation_call_ratios.append(
                float(candidate_sample["allocation_calls_per_transaction"])
                / float(baseline_sample["allocation_calls_per_transaction"])
            )
            paired_allocation_byte_ratios.append(
                float(candidate_sample["allocated_bytes_per_transaction"])
                / float(baseline_sample["allocated_bytes_per_transaction"])
            )
            paired_peak_rss_ratios.append(
                float(candidate_sample["peak_rss_bytes"])
                / float(baseline_sample["peak_rss_bytes"])
            )
            paired_reorg_latency_ratios.append(
                float(candidate_sample["reorg_latency_ns"])
                / float(baseline_sample["reorg_latency_ns"])
            )
            paired_shutdown_latency_ratios.append(
                float(candidate_sample["shutdown_latency_ns"])
                / float(baseline_sample["shutdown_latency_ns"])
            )
            baseline_cpu_parallelism.append(
                float(baseline_sample["child_cpu_parallelism"])
            )
            candidate_cpu_parallelism.append(
                float(candidate_sample["child_cpu_parallelism"])
            )
            baseline_involuntary_switch_rates.append(
                float(
                    baseline_sample[
                        "child_involuntary_context_switches_per_second"
                    ]
                )
            )
            candidate_involuntary_switch_rates.append(
                float(
                    candidate_sample[
                        "child_involuntary_context_switches_per_second"
                    ]
                )
            )
            paired_samples.append(
                {
                    "pair": run_index + 1,
                    "replicates_per_side": args.replicates_per_sample,
                    "baseline": baseline_sample,
                    "candidate": candidate_sample,
                    "candidate_over_baseline": throughput_ratio,
                    "candidate_over_baseline_target_cpu": target_cpu_ratio,
                }
            )
        if measurement_failed:
            continue
        median_ratio = statistics.median(paired_ratios)
        paired_mad = relative_mad(paired_ratios)
        summaries[key] = {
            "status": (
                "comparable"
                if paired_mad <= args.max_paired_mad_percent
                else "noisy"
            ),
            "replicates_per_sample": args.replicates_per_sample,
            "candidate_over_baseline_ratios": paired_ratios,
            "paired_samples": paired_samples,
            "median_candidate_over_baseline": median_ratio,
            "median_delta_percent": (median_ratio - 1.0) * 100.0,
            "paired_ratio_relative_mad_percent": paired_mad,
            "median_candidate_over_baseline_target_cpu": statistics.median(
                paired_cpu_ratios
            ),
            "paired_target_cpu_relative_mad_percent": relative_mad(
                paired_cpu_ratios
            ),
            "median_candidate_over_baseline_p99_latency": statistics.median(
                paired_p99_ratios
            ),
            "paired_p99_latency_relative_mad_percent": relative_mad(
                paired_p99_ratios
            ),
            "median_candidate_over_baseline_allocation_calls": statistics.median(
                paired_allocation_call_ratios
            ),
            "median_candidate_over_baseline_allocated_bytes": statistics.median(
                paired_allocation_byte_ratios
            ),
            "median_candidate_over_baseline_peak_rss": statistics.median(
                paired_peak_rss_ratios
            ),
            "median_candidate_over_baseline_reorg_latency": statistics.median(
                paired_reorg_latency_ratios
            ),
            "median_candidate_over_baseline_shutdown_latency": statistics.median(
                paired_shutdown_latency_ratios
            ),
            "baseline_median_child_cpu_parallelism": statistics.median(
                baseline_cpu_parallelism
            ),
            "candidate_median_child_cpu_parallelism": statistics.median(
                candidate_cpu_parallelism
            ),
            "baseline_median_involuntary_context_switches_per_second": (
                statistics.median(baseline_involuntary_switch_rates)
            ),
            "candidate_median_involuntary_context_switches_per_second": (
                statistics.median(candidate_involuntary_switch_rates)
            ),
            "baseline_throughput_spread_percent": spread(baseline_rates),
            "candidate_throughput_spread_percent": spread(candidate_rates),
            "baseline_median_tps": statistics.median(baseline_rates),
            "candidate_median_tps": statistics.median(candidate_rates),
            "baseline_median_p99_latency_ns": statistics.median(
                float(sample["baseline"]["p99_latency_ns"])
                for sample in paired_samples
            ),
            "candidate_median_p99_latency_ns": statistics.median(
                float(sample["candidate"]["p99_latency_ns"])
                for sample in paired_samples
            ),
            "baseline_median_cpu_time_per_transaction_ns": statistics.median(
                float(sample["baseline"]["cpu_time_per_transaction_ns"])
                for sample in paired_samples
            ),
            "candidate_median_cpu_time_per_transaction_ns": statistics.median(
                float(sample["candidate"]["cpu_time_per_transaction_ns"])
                for sample in paired_samples
            ),
            "baseline_median_peak_rss_bytes": statistics.median(
                float(sample["baseline"]["peak_rss_bytes"])
                for sample in paired_samples
            ),
            "candidate_median_peak_rss_bytes": statistics.median(
                float(sample["candidate"]["peak_rss_bytes"])
                for sample in paired_samples
            ),
            "baseline_median_allocation_calls_per_transaction": statistics.median(
                float(sample["baseline"]["allocation_calls_per_transaction"])
                for sample in paired_samples
            ),
            "candidate_median_allocation_calls_per_transaction": statistics.median(
                float(sample["candidate"]["allocation_calls_per_transaction"])
                for sample in paired_samples
            ),
            "baseline_median_allocated_bytes_per_transaction": statistics.median(
                float(sample["baseline"]["allocated_bytes_per_transaction"])
                for sample in paired_samples
            ),
            "candidate_median_allocated_bytes_per_transaction": statistics.median(
                float(sample["candidate"]["allocated_bytes_per_transaction"])
                for sample in paired_samples
            ),
            "baseline_median_reorg_latency_ns": statistics.median(
                float(sample["baseline"]["reorg_latency_ns"])
                for sample in paired_samples
            ),
            "candidate_median_reorg_latency_ns": statistics.median(
                float(sample["candidate"]["reorg_latency_ns"])
                for sample in paired_samples
            ),
            "baseline_median_shutdown_latency_ns": statistics.median(
                float(sample["baseline"]["shutdown_latency_ns"])
                for sample in paired_samples
            ),
            "candidate_median_shutdown_latency_ns": statistics.median(
                float(sample["candidate"]["shutdown_latency_ns"])
                for sample in paired_samples
            ),
            "baseline_median_authority_span_elapsed_nanos": {
                name: statistics.median(
                    float(
                        sample["baseline"]["authority_span_elapsed_nanos"][name]
                    )
                    for sample in paired_samples
                )
                for name in sorted(
                    paired_samples[0]["baseline"]["authority_span_elapsed_nanos"]
                )
            },
            "candidate_median_authority_span_elapsed_nanos": {
                name: statistics.median(
                    float(
                        sample["candidate"]["authority_span_elapsed_nanos"][name]
                    )
                    for sample in paired_samples
                )
                for name in sorted(
                    paired_samples[0]["candidate"]["authority_span_elapsed_nanos"]
                )
            },
        }
        record["summary"] = summaries
        write_checkpoint(args.output, record)

    ratios = [
        float(summary["median_candidate_over_baseline"])
        for summary in summaries.values()
        if summary["status"] == "comparable"
    ]
    summaries["aggregate"] = {
        "comparable_scenario_count": len(ratios),
        "geometric_mean_candidate_over_baseline": (
            math.exp(sum(math.log(ratio) for ratio in ratios) / len(ratios))
            if ratios
            else None
        ),
    }
    if sha256(Path(str(baseline["path"]))) != baseline["sha256"]:
        raise RuntimeError("baseline binary changed during measurement")
    if sha256(Path(str(candidate["path"]))) != candidate["sha256"]:
        raise RuntimeError("candidate binary changed during measurement")
    if git_record(baseline_root) != baseline_source:
        raise RuntimeError("baseline source changed during measurement")
    if git_record(candidate_root) != candidate_source:
        raise RuntimeError("candidate source changed during measurement")
    if sha256(baseline_root / harness_relative) != harness_hash:
        raise RuntimeError("baseline harness changed during measurement")
    if sha256(candidate_root / harness_relative) != harness_hash:
        raise RuntimeError("candidate harness changed during measurement")
    if sha256(Path(__file__)) != record["runner_sha256"]:
        raise RuntimeError("benchmark runner changed during measurement")
    record["summary"] = summaries
    write_checkpoint(args.output, record)
    print(json.dumps(summaries, indent=2, sort_keys=True))
    print(f">>> saved {args.output}")
    unacceptable = [
        key
        for key, summary in summaries.items()
        if key != "aggregate" and summary["status"] != "comparable"
    ]
    if unacceptable and not args.allow_noncomparable:
        raise SystemExit(2)


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "__measure_child__":
        raise SystemExit(measure_child())
    main()
