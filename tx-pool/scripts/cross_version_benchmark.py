#!/usr/bin/env python3
"""Run resumable paired fixed-binary tx-pool cross-version benchmarks."""

from __future__ import annotations

import argparse
import hashlib
import json
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
import tomllib
from pathlib import Path


RESULT = re.compile(
    r"^BENCH_RESULT scenario=(?P<scenario>\S+) target=(?P<target>\d+) "
    r"warm=(?P<warm>\d+) workers=(?P<workers>\d+) peers=(?P<peers>\d+) "
    r"elapsed_ns=(?P<elapsed_ns>\d+) throughput_tps=(?P<throughput>[0-9.]+) "
    r"accepted=(?P<accepted>\d+) callback_duplicates=(?P<callback_duplicates>\d+) "
    r"relay_ok=(?P<relay_ok>\d+) relay_duplicate_ok=(?P<relay_duplicate_ok>\d+) "
    r"relay_rejects=(?P<relay_rejects>\d+) "
    r"relay_unknown_parents=(?P<relay_unknown_parents>\d+) "
    r"relay_generation_resets=(?P<relay_generation_resets>\d+) "
    r"p99_latency_ns=(?P<p99_latency_ns>\d+) target_cpu_ns=(?P<target_cpu_ns>\d+) "
    r"allocation_calls=(?P<allocation_calls>\d+) allocated_bytes=(?P<allocated_bytes>\d+) "
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
    r"^RESOURCE_RESULT max_rss_bytes=(?P<max_rss_bytes>\d+) "
    r"voluntary_context_switches=(?P<voluntary_context_switches>\d+) "
    r"involuntary_context_switches=(?P<involuntary_context_switches>\d+)$",
    re.MULTILINE,
)
BUILD = re.compile(
    r"^BENCH_BUILD profiling=(?P<profiling>true|false) "
    r"allocation_observation=(?P<allocation_observation>true|false) "
    r"callback_observer=(?P<callback_observer>\S+) adapter=(?P<adapter>\S+) "
    r"debug_assertions=(?P<debug_assertions>true|false)$",
    re.MULTILINE,
)
CORPUS_PREFIX = "BENCH_CORPUS "
TERMINALS_PREFIX = "BENCH_TERMINALS "
MIN_CLOCK_TOLERANCE_NS = 1_000_000
CLOCK_TOLERANCE_DIVISOR = 10_000
MAX_SCENARIO_TRANSACTIONS = 65_536
FINAL_BUILD_PROFILE = "prod"
SCHEMA_VERSION = 7
CONSENSUS_LOCK_PACKAGES = ("ckb-vm", "ckb-vm-definitions")
HEX_32 = re.compile(r"^[0-9a-f]{64}$")
CORPUS_KEYS = {
    "consensus_blake2b",
    "cycle_assignment_count",
    "cycles_blake2b",
    "cycles_sum",
    "script_preflight_count",
    "transaction_bytes_blake2b",
    "transaction_count",
    "transaction_hashes_blake2b",
}
TERMINAL_KEYS = {
    "callback_duplicates",
    "relay_duplicate_ok",
    "relay_generation_resets",
    "relay_ok",
    "relay_rejects",
    "relay_unknown_parent_observations",
}
SUM_METRICS = (
    "elapsed_ns",
    "target_cpu_ns",
    "allocation_calls",
    "allocated_bytes",
    "voluntary_context_switches",
    "involuntary_context_switches",
)
MAX_METRICS = (
    "p99_latency_ns",
    "peak_rss_bytes",
    "reorg_latency_ns",
    "reorg_overlap_callbacks",
    "shutdown_latency_ns",
)
SUMMARY_METRICS = (
    "throughput_tps",
    "elapsed_ns",
    "target_cpu_ns",
    "p99_latency_ns",
    "allocation_calls",
    "allocated_bytes",
    "peak_rss_bytes",
    "voluntary_context_switches",
    "involuntary_context_switches",
    "reorg_latency_ns",
    "shutdown_latency_ns",
)


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
            timeout=120,
        ).strip()
    except (OSError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
        raise RuntimeError(f"cannot run {command[0]} in {root or Path.cwd()}: {error}") from error


def git_record(root: Path) -> dict[str, str]:
    root = root.resolve()
    status = command_output(["git", "status", "--porcelain=v1", "--untracked-files=all"], root)
    if status:
        raise RuntimeError(f"measurement worktree is dirty: {root}")
    return {
        "root": str(root),
        "commit": command_output(["git", "rev-parse", "HEAD"], root),
        "cargo_lock_sha256": sha256(root / "Cargo.lock"),
        "cargo_manifest_sha256": sha256(root / "Cargo.toml"),
        "tx_pool_manifest_sha256": sha256(root / "tx-pool" / "Cargo.toml"),
    }


def consensus_dependency_identity(root: Path, build_features: str) -> dict[str, object]:
    lock_packages = tomllib.loads((root / "Cargo.lock").read_text()).get("package")
    if not isinstance(lock_packages, list):
        raise RuntimeError(f"Cargo.lock package table is unavailable in {root}")
    locked = {}
    for name in CONSENSUS_LOCK_PACKAGES:
        matches = [package for package in lock_packages if package.get("name") == name]
        if len(matches) != 1:
            raise RuntimeError(f"expected exactly one {name} package in {root}/Cargo.lock")
        locked[name] = {
            key: matches[0].get(key) for key in ("name", "version", "source", "checksum")
        }
    command = ["cargo", "metadata", "--locked", "--offline", "--format-version", "1"]
    if build_features:
        command.extend(("--features", build_features))
    try:
        metadata = json.loads(
            subprocess.check_output(
                command,
                cwd=root,
                text=True,
                stderr=subprocess.STDOUT,
                timeout=120,
            )
        )
    except (OSError, subprocess.CalledProcessError, subprocess.TimeoutExpired, json.JSONDecodeError) as error:
        raise RuntimeError(f"cannot bind CKB-VM Cargo features in {root}: {error}") from error
    packages = {
        package["id"]: package
        for package in metadata.get("packages", [])
        if package.get("name") in CONSENSUS_LOCK_PACKAGES
    }
    resolve = metadata.get("resolve")
    nodes = resolve.get("nodes", []) if isinstance(resolve, dict) else []
    enabled = {}
    for node in nodes:
        package = packages.get(node.get("id"))
        if package is not None:
            enabled[package["name"]] = sorted(node.get("features", []))
    if set(enabled) != set(CONSENSUS_LOCK_PACKAGES):
        raise RuntimeError(f"CKB-VM Cargo feature identity is incomplete in {root}")
    return {
        "locked_packages": locked,
        "enabled_features": enabled,
        "root_build_features": build_features,
    }


def binary_record(path: Path) -> dict[str, object]:
    resolved = path.expanduser().resolve()
    if not resolved.is_file():
        raise RuntimeError(f"fixed binary does not exist: {resolved}")
    return {"path": str(resolved), "sha256": sha256(resolved), "size": resolved.stat().st_size}


def require_final_build_profile(profile: str | None) -> str:
    if profile != FINAL_BUILD_PROFILE:
        raise ValueError("fixed binaries require an explicit prod profile attestation")
    return profile


def build_binary(
    root: Path, target_dir: Path, features: str
) -> tuple[dict[str, object], dict[str, object]]:
    root, target_dir = root.resolve(), target_dir.resolve()
    command = [
        "cargo",
        "bench",
        "-p",
        "ckb-tx-pool",
        "--bench",
        "profile_one_shot",
        "--no-run",
        "--locked",
        "--profile",
        FINAL_BUILD_PROFILE,
        "--message-format=json",
    ]
    if features:
        command.extend(("--features", features))
    environment = os.environ.copy()
    environment["CARGO_TARGET_DIR"] = str(target_dir)
    environment["CARGO_INCREMENTAL"] = "0"
    encoded = environment.get("CARGO_ENCODED_RUSTFLAGS")
    inherited = encoded.split("\x1f") if encoded else shlex.split(environment.get("RUSTFLAGS", ""))
    environment.pop("RUSTFLAGS", None)
    environment["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(
        [*inherited, f"--remap-path-prefix={root}=/ckb-txpool-cross-source"]
    )
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
            f"benchmark build failed ({completed.returncode}):\n"
            f"{completed.stdout[-4000:]}\n{completed.stderr[-4000:]}"
        )
    executables = set()
    for line in completed.stdout.splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        target = message.get("target", {})
        if (
            message.get("reason") == "compiler-artifact"
            and target.get("name") == "profile_one_shot"
            and "bench" in target.get("kind", [])
            and message.get("executable")
        ):
            executables.add(Path(message["executable"]).resolve())
    if len(executables) != 1:
        raise RuntimeError("Cargo did not report exactly one profile_one_shot executable")
    return binary_record(executables.pop()), {
        "provenance": "built_once_by_runner",
        "command": command,
        "target_dir": str(target_dir),
        "features": features,
        "profile": FINAL_BUILD_PROFILE,
        "inherited_rustflags": inherited,
    }


def prepare_binary(
    root: Path,
    supplied: Path | None,
    target_dir: Path | None,
    features: str,
    supplied_profile: str | None,
) -> tuple[dict[str, object], dict[str, object]]:
    if supplied is not None:
        return binary_record(supplied), {
            "provenance": "supplied_by_sha256",
            "profile": require_final_build_profile(supplied_profile),
        }
    return build_binary(root, target_dir or root / "target" / "tx-pool-cross", features)


def parse_scenario(value: str) -> dict[str, object]:
    fields = value.split(",")
    if len(fields) != 5:
        raise ValueError(f"invalid scenario: {value}")
    try:
        numbers = [int(field) for field in fields[1:]]
    except ValueError as error:
        raise ValueError(f"invalid scenario: {value}") from error
    target, warm, workers, peers = numbers
    if (
        not fields[0]
        or target <= 0
        or warm < 0
        or workers <= 0
        or peers <= 0
        or target + warm > MAX_SCENARIO_TRANSACTIONS
    ):
        raise ValueError(f"invalid scenario: {value}")
    return dict(zip(("name", "target", "warm", "workers", "peers"), [fields[0], *numbers]))


def scenario_key(scenario: dict[str, object]) -> str:
    return (
        f"{scenario['name']}-t{scenario['target']}-w{scenario['warm']}-"
        f"v{scenario['workers']}-p{scenario['peers']}"
    )


def relative_mad(values: list[float]) -> float:
    median = statistics.median(values)
    return 0.0 if median == 0 else statistics.median(abs(value - median) for value in values) / median * 100


def parse_json_record(output: str, prefix: str) -> dict[str, object]:
    records = [line.removeprefix(prefix) for line in output.splitlines() if line.startswith(prefix)]
    if len(records) != 1:
        raise ValueError(f"observed {len(records)} {prefix.strip()} records")
    value = json.loads(records[0])
    if not isinstance(value, dict):
        raise ValueError(f"{prefix.strip()} record is not an object")
    return value


def corpus_observation_error(corpus: object, expected_transactions: int) -> str | None:
    if not isinstance(corpus, dict) or set(corpus) != CORPUS_KEYS:
        return "benchmark corpus identity has an unsupported shape"
    if corpus["transaction_count"] != expected_transactions or corpus["cycle_assignment_count"] != expected_transactions:
        return "benchmark corpus transaction/cycle assignment count differs"
    preflight, cycles = corpus["script_preflight_count"], corpus["cycles_sum"]
    if type(preflight) is not int or not 1 <= preflight <= expected_transactions:
        return "benchmark script preflight count is invalid"
    if type(cycles) is not int or cycles <= 0:
        return "benchmark assigned cycle sum is invalid"
    for field in (
        "consensus_blake2b",
        "cycles_blake2b",
        "transaction_bytes_blake2b",
        "transaction_hashes_blake2b",
    ):
        if not isinstance(corpus[field], str) or HEX_32.fullmatch(corpus[field]) is None:
            return f"benchmark corpus {field} is not a digest"
    return None


def paired_corpus_error(baseline_corpus: object, candidate_corpus: object) -> str | None:
    return None if baseline_corpus == candidate_corpus else "baseline and candidate corpus identities differ"


def terminal_observation_error(
    *,
    scenario_name: str,
    expected_accepted: int,
    accepted: int,
    callback_duplicates: int,
    relay_ok: int,
    relay_duplicate_ok: int,
    relay_rejects: int,
    relay_unknown_parents: int,
    relay_generation_resets: int,
    expected_relay_rejects: int,
) -> str | None:
    checks = (
        (accepted != expected_accepted, f"accepted {accepted}, expected {expected_accepted}"),
        (relay_ok != expected_accepted, f"relay Ok {relay_ok}, expected {expected_accepted}"),
        (callback_duplicates != 0 and scenario_name != "reorg_in_flight", "unexpected duplicate callbacks"),
        (relay_duplicate_ok != 0, "unexpected duplicate relay Ok results"),
        (relay_rejects != expected_relay_rejects, f"relay rejects {relay_rejects}, expected {expected_relay_rejects}"),
        (relay_generation_resets != 0, "unexpected relay generation resets"),
        (relay_unknown_parents != 0 and not scenario_name.endswith("_reverse"), "unexpected unknown-parent relay results"),
    )
    return next((message for failed, message in checks if failed), None)


def terminal_record_error(
    terminals: object,
    *,
    callback_duplicates: int,
    relay_ok: int,
    relay_duplicate_ok: int,
    relay_rejects: int,
    relay_unknown_parents: int,
    relay_generation_resets: int,
) -> str | None:
    if not isinstance(terminals, dict) or set(terminals) != TERMINAL_KEYS:
        return "benchmark terminal identity has an unsupported shape"
    expected = {
        "callback_duplicates": callback_duplicates,
        "relay_ok": relay_ok,
        "relay_duplicate_ok": relay_duplicate_ok,
        "relay_rejects": relay_rejects,
        "relay_generation_resets": relay_generation_resets,
    }
    if any(terminals[field] != value for field, value in expected.items()):
        return "benchmark terminal JSON differs from scalar evidence"
    observations = terminals["relay_unknown_parent_observations"]
    if not isinstance(observations, list):
        return "benchmark unknown-parent multiset is unavailable"
    normalized = []
    for observation in observations:
        if not isinstance(observation, dict) or set(observation) != {"peer", "parents", "count"}:
            return "benchmark unknown-parent observation has an unsupported shape"
        peer, parents, count = observation["peer"], observation["parents"], observation["count"]
        if (
            type(peer) is not int
            or peer < 0
            or type(count) is not int
            or count <= 0
            or not isinstance(parents, list)
            or not parents
            or parents != sorted(set(parents))
            or any(not isinstance(parent, str) or HEX_32.fullmatch(parent) is None for parent in parents)
        ):
            return "benchmark unknown-parent observation is invalid"
        normalized.append((peer, tuple(parents), count))
    if normalized != sorted(normalized) or len(normalized) != len({row[:2] for row in normalized}):
        return "benchmark unknown-parent multiset is not canonical"
    if sum(row[2] for row in normalized) != relay_unknown_parents:
        return "benchmark unknown-parent multiset differs from scalar evidence"
    return None


def timing_build_observation(
    output: str, spans: object, allocation_observation: str
) -> tuple[dict[str, str] | None, str | None]:
    matches = list(BUILD.finditer(output))
    if len(matches) != 1:
        return None, f"observed {len(matches)} BENCH_BUILD records"
    build = matches[0].groupdict()
    expected_allocation = "true" if allocation_observation == "enabled" else "false"
    if build["profiling"] != "false" or build["debug_assertions"] != "false":
        return None, "final timing binary enables profiling or debug assertions"
    if build["allocation_observation"] != expected_allocation:
        return None, "allocation observation build identity differs"
    if build["callback_observer"] != "preallocated_atomic_slots_sharded_completion":
        return None, "timing callback observer is unsupported"
    if build["adapter"] not in {"bounded_remote_batch", "legacy_peer_local_sequential"}:
        return None, f"unsupported benchmark adapter {build['adapter']}"
    if spans is not None:
        return None, "profiling-disabled timing emitted span evidence"
    return build, None


def measure_child() -> int:
    command = sys.argv[2:]
    if not command:
        raise RuntimeError("resource wrapper has no child command")
    before = resource.getrusage(resource.RUSAGE_CHILDREN)
    completed = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, check=False)
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    sys.stdout.buffer.write(completed.stdout)
    rss_scale = 1 if sys.platform == "darwin" else 1024
    print(
        "RESOURCE_RESULT "
        f"max_rss_bytes={round(after.ru_maxrss * rss_scale)} "
        f"voluntary_context_switches={after.ru_nvcsw - before.ru_nvcsw} "
        f"involuntary_context_switches={after.ru_nivcsw - before.ru_nivcsw}"
    )
    return completed.returncode


def failure_attempt(
    attempt_id: str,
    side: str,
    command: list[str],
    started: int,
    category: str,
    detail: str,
    output: str,
) -> dict[str, object]:
    return {
        "id": attempt_id,
        "outcome": "failure",
        "side": side,
        "command": command,
        "started_unix_ns": started,
        "ended_unix_ns": time.time_ns(),
        "category": category,
        "detail": detail,
        "output": output,
    }


def timeout_output(error: subprocess.TimeoutExpired) -> str:
    output = error.stdout or ""
    return output.decode(errors="replace") if isinstance(output, bytes) else output


def unique_match(pattern: re.Pattern[str], output: str, label: str) -> dict[str, str]:
    matches = list(pattern.finditer(output))
    if len(matches) != 1:
        raise ValueError(f"observed {len(matches)} {label} records")
    return matches[0].groupdict()


def run_attempt(
    binary: dict[str, object],
    root: Path,
    scenario: dict[str, object],
    side: str,
    attempt_id: str,
    timeout: float,
    allocation_observation: str,
) -> dict[str, object]:
    path = Path(str(binary["path"]))
    if binary_record(path) != binary:
        raise RuntimeError(f"{side} binary changed before {attempt_id}")
    benchmark = [
        str(path),
        str(scenario["name"]),
        str(scenario["target"]),
        str(scenario["warm"]),
        str(scenario["workers"]),
        str(scenario["peers"]),
    ]
    command = [sys.executable, str(Path(__file__).resolve()), "__measure_child__", *benchmark]
    started = time.time_ns()
    with tempfile.TemporaryDirectory(prefix="ckb-txpool-span-") as temporary:
        span_path = Path(temporary) / "spans.json"
        environment = os.environ.copy()
        environment["TX_POOL_PROFILE_TRACE_PATH"] = str(span_path)
        try:
            completed = subprocess.run(
                command,
                cwd=root,
                env=environment,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                timeout=timeout,
                check=False,
            )
        except subprocess.TimeoutExpired as error:
            return failure_attempt(
                attempt_id,
                side,
                command,
                started,
                "runner_timeout",
                f"process exceeded {timeout:.3f} seconds",
                timeout_output(error),
            )
        except OSError as error:
            return failure_attempt(attempt_id, side, command, started, "spawn_failure", str(error), "")
        spans = {} if span_path.exists() else None
    if completed.returncode != 0:
        return failure_attempt(
            attempt_id,
            side,
            command,
            started,
            "nonzero_exit",
            f"process exited with status {completed.returncode}",
            completed.stdout,
        )
    try:
        result = unique_match(RESULT, completed.stdout, "BENCH_RESULT")
        window = unique_match(WINDOW, completed.stdout, "PROFILE_WINDOW")
        resources = unique_match(RESOURCE_RESULT, completed.stdout, "RESOURCE_RESULT")
        corpus = parse_json_record(completed.stdout, CORPUS_PREFIX)
        terminals = parse_json_record(completed.stdout, TERMINALS_PREFIX)
        build, error = timing_build_observation(completed.stdout, spans, allocation_observation)
        if error is not None or build is None:
            raise ValueError(error)
        observed = {
            "name": result["scenario"],
            "target": int(result["target"]),
            "warm": int(result["warm"]),
            "workers": int(result["workers"]),
            "peers": int(result["peers"]),
        }
        if observed != scenario:
            raise ValueError(f"scenario drift: {observed} != {scenario}")
        counts = {
            name: int(result[name])
            for name in (
                "accepted",
                "callback_duplicates",
                "relay_ok",
                "relay_duplicate_ok",
                "relay_rejects",
                "relay_unknown_parents",
                "relay_generation_resets",
            )
        }
        expected_accepted = int(scenario["target"]) + int(scenario["warm"])
        expected_rejects = (
            int(scenario["warm"])
            if scenario["name"] == "rbf_pairs" and build["adapter"] == "bounded_remote_batch"
            else 0
        )
        error = terminal_observation_error(
            scenario_name=str(scenario["name"]),
            expected_accepted=expected_accepted,
            expected_relay_rejects=expected_rejects,
            **counts,
        ) or corpus_observation_error(corpus, expected_accepted)
        if error is None:
            error = terminal_record_error(terminals, **{key: counts[key] for key in (
                "callback_duplicates",
                "relay_ok",
                "relay_duplicate_ok",
                "relay_rejects",
                "relay_unknown_parents",
                "relay_generation_resets",
            )})
        if error is not None:
            raise ValueError(error)
        elapsed_ns = int(result["elapsed_ns"])
        wall_ns = int(window["end"]) - int(window["start"])
        tolerance = max(MIN_CLOCK_TOLERANCE_NS, elapsed_ns // CLOCK_TOLERANCE_DIVISOR)
        metrics = {
            "elapsed_ns": elapsed_ns,
            "throughput_tps": float(result["throughput"]),
            "target_cpu_ns": int(result["target_cpu_ns"]),
            "p99_latency_ns": int(result["p99_latency_ns"]),
            "allocation_calls": int(result["allocation_calls"]),
            "allocated_bytes": int(result["allocated_bytes"]),
            "peak_rss_bytes": int(resources["max_rss_bytes"]),
            "voluntary_context_switches": int(resources["voluntary_context_switches"]),
            "involuntary_context_switches": int(resources["involuntary_context_switches"]),
            "reorg_latency_ns": int(result["reorg_latency_ns"]),
            "reorg_overlap_callbacks": int(result["reorg_overlap_callbacks"]),
            "shutdown_latency_ns": int(result["shutdown_latency_ns"]),
        }
        positive = (
            "elapsed_ns",
            "throughput_tps",
            "target_cpu_ns",
            "p99_latency_ns",
            "peak_rss_bytes",
            "reorg_latency_ns",
            "shutdown_latency_ns",
        )
        if wall_ns <= 0 or any(metrics[name] <= 0 for name in positive):
            raise ValueError("benchmark emitted a non-positive required metric")
        if wall_ns - elapsed_ns < -tolerance:
            raise ValueError("target wall-clock window is shorter than monotonic elapsed time")
        allocations = metrics["allocation_calls"], metrics["allocated_bytes"]
        if allocation_observation == "enabled" and min(allocations) <= 0:
            raise ValueError("enabled allocation observation is empty")
        if allocation_observation == "disabled" and any(allocations):
            raise ValueError("timing binary emitted allocation counts")
        if (scenario["name"] == "reorg_in_flight") != (metrics["reorg_overlap_callbacks"] > 0):
            raise ValueError("reorg overlap differs from its scenario")
    except (KeyError, TypeError, ValueError, json.JSONDecodeError) as error:
        return failure_attempt(
            attempt_id,
            side,
            command,
            started,
            "invalid_evidence",
            str(error),
            completed.stdout,
        )
    return {
        "id": attempt_id,
        "outcome": "success",
        "side": side,
        "command": command,
        "started_unix_ns": started,
        "ended_unix_ns": time.time_ns(),
        "scenario": scenario,
        "build": build,
        "window": {
            "start_unix_ns": int(window["start"]),
            "end_unix_ns": int(window["end"]),
            "wall_ns": wall_ns,
            "clock_tolerance_ns": tolerance,
        },
        "corpus": corpus,
        "terminals": terminals,
        "relay_unknown_parents": counts["relay_unknown_parents"],
        "metrics": metrics,
        "output": completed.stdout,
    }


def aggregate_side(attempts: list[dict[str, object]], target_per_attempt: int) -> dict[str, object]:
    metrics = [attempt["metrics"] for attempt in attempts]
    target = target_per_attempt * len(attempts)
    aggregate = {name: sum(int(row[name]) for row in metrics) for name in SUM_METRICS}
    aggregate.update({name: max(int(row[name]) for row in metrics) for name in MAX_METRICS})
    aggregate["throughput_tps"] = target * 1e9 / aggregate["elapsed_ns"]
    return {
        "attempt_ids": [attempt["id"] for attempt in attempts],
        "target_transactions": target,
        "metrics": aggregate,
    }


def metric_summary(samples: list[dict[str, object]], name: str) -> dict[str, object]:
    baseline = [float(sample["baseline"]["metrics"][name]) for sample in samples]
    candidate = [float(sample["candidate"]["metrics"][name]) for sample in samples]
    ratios = [right / left for left, right in zip(baseline, candidate) if left != 0]
    return {
        "baseline_median": statistics.median(baseline),
        "candidate_median": statistics.median(candidate),
        "candidate_over_baseline_ratios": ratios,
        "median_candidate_over_baseline": statistics.median(ratios) if ratios else None,
        "ratio_relative_mad_percent": relative_mad(ratios) if ratios else None,
    }


def summarize_pairs(
    samples: list[dict[str, object]],
    corpus: dict[str, object],
    mode: str,
    max_mad: float,
) -> dict[str, object]:
    metrics = {name: metric_summary(samples, name) for name in SUMMARY_METRICS}
    throughput_mad = metrics["throughput_tps"]["ratio_relative_mad_percent"]
    status = (
        "allocation_observation"
        if mode == "enabled"
        else "comparable"
        if throughput_mad <= max_mad
        else "noisy"
    )
    return {
        "status": status,
        "corpus": corpus,
        "paired_samples": samples,
        "metrics": metrics,
        "noise_rule": {
            "metric": "throughput_tps",
            "maximum_relative_mad_percent": max_mad,
            "observed_relative_mad_percent": throughput_mad,
            "ranking_boundary": (
                "timing_cpu_p99_rss"
                if mode == "disabled"
                else "allocation_only"
            ),
        },
    }


def write_checkpoint(path: Path, record: dict[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(record, indent=2, sort_keys=True) + "\n")
    temporary.replace(path)


def read_checkpoint(path: Path) -> dict[str, object]:
    try:
        record = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise RuntimeError(f"cannot read checkpoint {path}: {error}") from error
    if not isinstance(record, dict):
        raise RuntimeError("checkpoint is not a JSON object")
    return record


def host_identity() -> dict[str, object]:
    return {
        "platform": platform.platform(),
        "machine": platform.machine(),
        "python": platform.python_version(),
        "cpu_count": os.cpu_count(),
        "rustc": command_output(["rustc", "-Vv"]),
        "cargo": command_output(["cargo", "-V"]),
    }


def environment_snapshot() -> dict[str, object]:
    return {"captured_unix_ns": time.time_ns(), "load_average": list(os.getloadavg())}


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    for side in ("baseline", "candidate"):
        parser.add_argument(f"--{side}-root", type=Path, required=True)
        parser.add_argument(f"--{side}-binary", type=Path)
        parser.add_argument(f"--{side}-binary-profile")
        parser.add_argument(f"--{side}-target-dir", type=Path)
        parser.add_argument(f"--{side}-build-features", default="")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--resume", action="store_true")
    parser.add_argument("--runs", type=int, default=10)
    parser.add_argument("--replicates-per-sample", type=int, default=1)
    parser.add_argument("--initial-cooldown-seconds", type=float, default=15.0)
    parser.add_argument("--cooldown-seconds", type=float, default=10.0)
    parser.add_argument("--max-paired-mad-percent", type=float, default=1.5)
    parser.add_argument("--timeout-seconds", type=float, default=180.0)
    parser.add_argument("--allocation-observation", choices=("disabled", "enabled"), default="disabled")
    parser.add_argument("--allow-noncomparable", action="store_true")
    parser.add_argument("--scenario", action="append", required=True, metavar="NAME,TARGET,WARM,WORKERS,PEERS")
    args = parser.parse_args()
    if args.runs < 6 or args.runs % 2:
        parser.error("--runs must be even and at least 6")
    if args.replicates_per_sample <= 0 or args.replicates_per_sample > 8 or (
        args.replicates_per_sample != 1 and args.replicates_per_sample % 2
    ):
        parser.error("--replicates-per-sample must be 1 or an even value from 2 to 8")
    if min(args.initial_cooldown_seconds, args.cooldown_seconds) < 0 or min(
        args.timeout_seconds, args.max_paired_mad_percent
    ) <= 0:
        parser.error("cooldowns must be non-negative and limits positive")
    if args.resume != args.output.exists():
        parser.error("--resume requires an existing output; a new run requires a new output")
    for side in ("baseline", "candidate"):
        binary = getattr(args, f"{side}_binary")
        target = getattr(args, f"{side}_target_dir")
        profile = getattr(args, f"{side}_binary_profile")
        if binary is not None and target is not None:
            parser.error(f"--{side}-target-dir cannot accompany --{side}-binary")
        if binary is None and profile is not None:
            parser.error(f"--{side}-binary-profile requires --{side}-binary")
        if binary is not None:
            try:
                require_final_build_profile(profile)
            except ValueError as error:
                parser.error(str(error))
    return args


def configuration(args: argparse.Namespace, scenarios: list[dict[str, object]]) -> dict[str, object]:
    return {
        "baseline_root": str(args.baseline_root.resolve()),
        "candidate_root": str(args.candidate_root.resolve()),
        "baseline_build_features": args.baseline_build_features,
        "candidate_build_features": args.candidate_build_features,
        "scenarios": scenarios,
        "runs": args.runs,
        "replicates_per_sample": args.replicates_per_sample,
        "initial_cooldown_seconds": args.initial_cooldown_seconds,
        "cooldown_seconds": args.cooldown_seconds,
        "max_paired_mad_percent": args.max_paired_mad_percent,
        "timeout_seconds": args.timeout_seconds,
        "allocation_observation": args.allocation_observation,
    }


def attempt_index(record: dict[str, object]) -> dict[str, dict[str, object]]:
    attempts = record.get("attempts")
    if not isinstance(attempts, list) or any(not isinstance(item, dict) or not isinstance(item.get("id"), str) for item in attempts):
        raise RuntimeError("checkpoint attempt ledger is invalid")
    indexed = {attempt["id"]: attempt for attempt in attempts}
    if len(indexed) != len(attempts):
        raise RuntimeError("checkpoint contains duplicate attempt IDs")
    return indexed


def cool(seconds: float) -> None:
    if seconds:
        time.sleep(seconds)


def obtain_attempt(
    record: dict[str, object],
    indexed: dict[str, dict[str, object]],
    output: Path,
    context: dict[str, object],
    scenario: dict[str, object],
    side: str,
    attempt_id: str,
    args: argparse.Namespace,
    expected_corpus: dict[str, object] | None = None,
) -> dict[str, object]:
    cached = indexed.get(attempt_id)
    if cached is not None:
        if cached.get("side") != side or (
            cached.get("outcome") == "success" and cached.get("scenario") != scenario
        ):
            raise RuntimeError(f"checkpoint attempt identity drifted: {attempt_id}")
        return cached
    print(f">>> {attempt_id}", flush=True)
    attempt = run_attempt(
        context["binary"],
        Path(context["source"]["root"]),
        scenario,
        side,
        attempt_id,
        args.timeout_seconds,
        args.allocation_observation,
    )
    if attempt["outcome"] == "success" and expected_corpus is not None and attempt["corpus"] != expected_corpus:
        attempt.update(
            outcome="failure",
            category="corpus_drift",
            detail="corpus changed after the paired pilot",
        )
    record["attempts"].append(attempt)
    indexed[attempt_id] = attempt
    write_checkpoint(output, record)
    cool(args.cooldown_seconds)
    return attempt


def failure_summary(reason: str, attempts: list[dict[str, object]]) -> dict[str, object]:
    return {
        "status": "non_comparable",
        "reason": reason,
        "failures": [
            {"id": attempt["id"], "side": attempt["side"], "category": attempt.get("category")}
            for attempt in attempts
            if attempt["outcome"] == "failure"
        ],
    }


def run_scenario(
    record: dict[str, object],
    indexed: dict[str, dict[str, object]],
    output: Path,
    contexts: dict[str, dict[str, object]],
    scenario: dict[str, object],
    args: argparse.Namespace,
) -> None:
    key = scenario_key(scenario)
    pilots = [
        obtain_attempt(
            record,
            indexed,
            output,
            contexts[side],
            scenario,
            side,
            f"{key}/pilot/{side}",
            args,
        )
        for side in ("candidate", "baseline")
    ]
    if any(attempt["outcome"] == "failure" for attempt in pilots):
        record["summary"][key] = failure_summary("pilot_failure", pilots)
        write_checkpoint(output, record)
        return
    if paired_corpus_error(pilots[1]["corpus"], pilots[0]["corpus"]):
        record["summary"][key] = failure_summary("pilot_corpus_mismatch", pilots)
        write_checkpoint(output, record)
        return
    corpus = pilots[0]["corpus"]
    samples = []
    failures = []
    for pair_number in range(1, args.runs + 1):
        paired: dict[str, list[dict[str, object]]] = {"baseline": [], "candidate": []}
        for replicate in range(1, args.replicates_per_sample + 1):
            order = ["baseline", "candidate"]
            if (pair_number + replicate) % 2:
                order.reverse()
            for side in order:
                attempt = obtain_attempt(
                    record,
                    indexed,
                    output,
                    contexts[side],
                    scenario,
                    side,
                    f"{key}/pair-{pair_number}/replicate-{replicate}/{side}",
                    args,
                    corpus,
                )
                if attempt["outcome"] == "failure":
                    failures.append(attempt)
                    break
                paired[side].append(attempt)
            if failures:
                break
        if failures:
            break
        baseline = aggregate_side(paired["baseline"], int(scenario["target"]))
        candidate = aggregate_side(paired["candidate"], int(scenario["target"]))
        samples.append(
            {
                "pair": pair_number,
                "baseline": baseline,
                "candidate": candidate,
                "ratios": {
                    name: (
                        candidate["metrics"][name] / baseline["metrics"][name]
                        if baseline["metrics"][name]
                        else None
                    )
                    for name in SUMMARY_METRICS
                },
            }
        )
    record["summary"][key] = (
        failure_summary("measurement_failure", failures)
        if failures
        else summarize_pairs(
            samples,
            corpus,
            args.allocation_observation,
            args.max_paired_mad_percent,
        )
    )
    write_checkpoint(output, record)


def validate_frozen(
    record: dict[str, object],
    contexts: dict[str, dict[str, object]],
    harness_hash: str,
    host: dict[str, object],
    supplied: dict[str, Path | None],
) -> None:
    if record.get("runner_sha256") != sha256(Path(__file__)):
        raise RuntimeError("benchmark runner changed")
    if record.get("harness_sha256") != harness_hash or record.get("host") != host:
        raise RuntimeError("benchmark harness or host identity changed")
    recorded = record.get("sides")
    if not isinstance(recorded, dict):
        raise RuntimeError("checkpoint side identity is unavailable")
    for side, current in contexts.items():
        frozen = recorded.get(side)
        if not isinstance(frozen, dict):
            raise RuntimeError(f"checkpoint {side} identity is unavailable")
        if frozen.get("source") != current["source"] or frozen.get("consensus") != current["consensus"]:
            raise RuntimeError(f"{side} source or consensus identity changed")
        if binary_record(Path(str(frozen["binary"]["path"]))) != frozen["binary"]:
            raise RuntimeError(f"{side} binary changed")
        if supplied[side] is not None and binary_record(supplied[side]) != frozen["binary"]:
            raise RuntimeError(f"supplied {side} binary differs from the checkpoint")
        current["binary"], current["build"] = frozen["binary"], frozen["build"]


def main() -> None:
    args = arguments()
    scenarios = [parse_scenario(value) for value in args.scenario]
    roots = {side: getattr(args, f"{side}_root").resolve() for side in ("baseline", "candidate")}
    if roots["baseline"] == roots["candidate"]:
        raise RuntimeError("baseline and candidate roots must be distinct")
    harness = Path("tx-pool/benches/profile_one_shot.rs")
    harness_hash = sha256(roots["baseline"] / harness)
    if sha256(roots["candidate"] / harness) != harness_hash:
        raise RuntimeError("baseline and candidate harnesses differ")
    contexts = {
        side: {
            "source": git_record(roots[side]),
            "consensus": consensus_dependency_identity(
                roots[side], getattr(args, f"{side}_build_features")
            ),
        }
        for side in ("baseline", "candidate")
    }
    for field in ("locked_packages", "enabled_features"):
        if contexts["baseline"]["consensus"][field] != contexts["candidate"]["consensus"][field]:
            raise RuntimeError("baseline and candidate CKB-VM identities differ")
    config = configuration(args, scenarios)
    host = host_identity()
    supplied = {side: getattr(args, f"{side}_binary") for side in ("baseline", "candidate")}
    if args.resume:
        record = read_checkpoint(args.output)
        if record.get("schema") != SCHEMA_VERSION or record.get("configuration") != config:
            raise RuntimeError("checkpoint schema or configuration differs")
        validate_frozen(record, contexts, harness_hash, host, supplied)
        record["complete"] = False
    else:
        targets = {
            side: getattr(args, f"{side}_target_dir") or roots[side] / "target" / "tx-pool-cross"
            for side in ("baseline", "candidate")
        }
        if all(supplied[side] is None for side in supplied) and targets["baseline"].resolve() == targets["candidate"].resolve():
            raise RuntimeError("baseline and candidate target directories must be isolated")
        for side in ("baseline", "candidate"):
            contexts[side]["binary"], contexts[side]["build"] = prepare_binary(
                roots[side],
                supplied[side],
                getattr(args, f"{side}_target_dir"),
                getattr(args, f"{side}_build_features"),
                getattr(args, f"{side}_binary_profile"),
            )
        record = {
            "schema": SCHEMA_VERSION,
            "runner_sha256": sha256(Path(__file__)),
            "harness_sha256": harness_hash,
            "host": host,
            "configuration": config,
            "sides": contexts,
            "environment": {"starts": [], "end": None},
            "attempts": [],
            "summary": {},
            "complete": False,
        }
    record["environment"]["starts"].append(environment_snapshot())
    write_checkpoint(args.output, record)
    cool(args.initial_cooldown_seconds)
    indexed = attempt_index(record)
    for scenario in scenarios:
        run_scenario(record, indexed, args.output, contexts, scenario, args)
    validate_frozen(record, contexts, harness_hash, host, supplied)
    record["environment"]["end"] = environment_snapshot()
    record["complete"] = True
    write_checkpoint(args.output, record)
    print(json.dumps(record["summary"], indent=2, sort_keys=True))
    print(f">>> saved {args.output}")
    accepted_status = "comparable" if args.allocation_observation == "disabled" else "allocation_observation"
    if any(summary["status"] != accepted_status for summary in record["summary"].values()) and not args.allow_noncomparable:
        raise SystemExit(2)


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "__measure_child__":
        raise SystemExit(measure_child())
    main()
