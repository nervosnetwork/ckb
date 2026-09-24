#!/usr/bin/env python3
"""Run resumable paired fixed-binary tx-pool cross-version benchmarks."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import random
import re
import resource
import statistics
import subprocess
import sys
import tempfile
import time
import tomllib
from pathlib import Path

import rejection_diagnostics
from benchmark_build import (binary_record, build_binary, effective_features,
                             git_record, host_identity, load_build, sha256, validate_build)
from benchmark_scenario import validate_scenario
from measurement_observation import parse_observation
from measurement_process import run_process
from measurement_window import parse_measurement_window, parse_readiness, wall_alignment


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
    r"debug_assertions=(?P<debug_assertions>true|false) "
    r"measurement_window=(?P<measurement_window>\S+)"
    r"(?: comparison_contract=(?P<comparison_contract>\S+))?$",
    re.MULTILINE,
)
CORPUS_PREFIX = "BENCH_CORPUS "
SCHEMA_VERSION = 15
PROTOCOL_CONTRACT = "protocol"
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
    "mean_peak_rss_bytes",
    "voluntary_context_switches",
    "involuntary_context_switches",
    "reorg_latency_ns",
    "shutdown_latency_ns",
)
PRECISION_METRICS = ("throughput_tps", "target_cpu_ns", "mean_peak_rss_bytes")
METRIC_SCOPES = {
    "target_terminal_window": ["elapsed_ns", "throughput_tps", "target_cpu_ns",
                               "allocation_calls", "allocated_bytes"],
    "target_callback_latencies": ["p99_latency_ns"],
    "whole_process_lifetime": ["peak_rss_bytes", "mean_peak_rss_bytes", "voluntary_context_switches",
                               "involuntary_context_switches"],
    "separately_timed_operations": ["reorg_latency_ns", "shutdown_latency_ns"],
}


HARNESS_FILES = ("tx-pool/benches/profile_one_shot.rs",
                 "tx-pool/benches/allocation_observation/mod.rs",
                 "tx-pool/benches/profile_spans/mod.rs",
                 "tx-pool/benches/relay_batches/mod.rs",
                 "tx-pool/benches/measurement_clock/mod.rs",
                 "tx-pool/benches/resource_phases/mod.rs")


def harness_bundle(root: Path) -> dict[str, str]:
    paths = set(HARNESS_FILES)
    paths.update(str(path.relative_to(root)) for path in
                 (root / "tx-pool/benches").rglob("*.rs") if path.is_file())
    return {path: sha256(root / path) for path in sorted(paths)}


def bundle_hash(root: Path) -> str:
    return hashlib.sha256(json.dumps(harness_bundle(root), sort_keys=True).encode()).hexdigest()


def balanced_schedule(runs: int, replicates: int, seed: int, key: str) -> list[list[list[str]]]:
    """Freeze randomized balanced AB/BA blocks; replicates remain one sampling unit."""
    rng = random.Random(f"txpool-order-v1:{seed}:{key}")
    if replicates == 1:
        starts = [0, 1] * (runs // 2)
        rng.shuffle(starts)
        blocks = [[start] for start in starts]
    else:
        blocks = []
        for _ in range(runs):
            block = [0, 1] * (replicates // 2)
            rng.shuffle(block)
            blocks.append(block)
    return [[["baseline", "candidate"] if start == 0 else ["candidate", "baseline"]
             for start in block] for block in blocks]


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
            run_process(
                command,
                cwd=root,
                text=True,
                stdout=subprocess.PIPE,
                stderr=None,
                timeout=120,
                check=True,
            ).stdout
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


def parse_scenario(value: str) -> dict[str, object]:
    fields = value.split(",")
    if len(fields) != 5:
        raise ValueError(f"invalid scenario: {value}")
    try:
        numbers = [int(field) for field in fields[1:]]
    except ValueError as error:
        raise ValueError(f"invalid scenario: {value}") from error
    try:
        validate_scenario(fields[0], *numbers)
    except ValueError as error:
        raise ValueError(f"invalid scenario: {value}: {error}") from error
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


def timing_build_observation(
    output: str, spans: object, allocation_observation: str,
) -> tuple[dict[str, str] | None, str | None]:
    if any(line.startswith("BENCH_DIAGNOSTICS ") for line in output.splitlines()):
        return None, "final timing contains diagnostic instrumentation"
    matches = list(BUILD.finditer(output))
    if len(matches) != 1:
        return None, f"observed {len(matches)} BENCH_BUILD records"
    build = matches[0].groupdict()
    # Binaries without a contract marker are production-only.
    build["comparison_contract"] = build["comparison_contract"] or PROTOCOL_CONTRACT
    if build["comparison_contract"] != PROTOCOL_CONTRACT:
        return None, "benchmark does not preserve the protocol contract"
    expected_allocation = "true" if allocation_observation == "enabled" else "false"
    if build["profiling"] != "false" or build["debug_assertions"] != "false":
        return None, "final timing binary enables profiling or debug assertions"
    if build["measurement_window"] != "terminal_completion_v3":
        return None, "timing measurement window is unsupported"
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
    # The outer owned-process runner must receive output before this child exits.
    completed = subprocess.run(command, stderr=subprocess.STDOUT, check=False)
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
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
        "rejection_diagnostics": rejection_diagnostics.inspect_failure(output),
    }


def timeout_output(error: BaseException) -> str:
    output = getattr(error, "output", None) or ""
    return output.decode(errors="replace") if isinstance(output, bytes) else output


def unique_match(pattern: re.Pattern[str], output: str, label: str) -> dict[str, str]:
    matches = list(pattern.finditer(output))
    if len(matches) != 1:
        raise ValueError(f"observed {len(matches)} {label} records")
    return matches[0].groupdict()


def parse_attempt(output: str, spans: object, scenario: dict[str, object],
                  allocation_observation: str) -> dict[str, object]:
    if not isinstance(output, str):
        raise ValueError("benchmark output is not text")
    rejection_diagnostics.validate_success(output)
    resources = unique_match(RESOURCE_RESULT, output, "RESOURCE_RESULT")
    corpus = parse_json_record(output, CORPUS_PREFIX)
    build, error = timing_build_observation(output, spans, allocation_observation)
    if error is not None or build is None:
        raise ValueError(error)
    expected = {"scenario": scenario["name"], **{name: scenario[name] for name in ("target", "warm", "workers", "peers")}}
    # Only a validated adapter identity may select legacy victim-notice semantics.
    observation = parse_observation(output, expected, victim_notices=build["adapter"] == "bounded_remote_batch")
    error = corpus_observation_error(corpus, scenario["target"] + scenario["warm"])
    if error is not None:
        raise ValueError(error)
    elapsed_ns = observation["elapsed_nanos"]
    window = parse_measurement_window(output, str(scenario["name"]), elapsed_ns)
    readiness = parse_readiness(output, str(scenario["name"]), int(scenario["target"]), elapsed_ns)
    metrics = {
        "elapsed_ns": elapsed_ns,
        "throughput_tps": int(scenario["target"]) * 1e9 / elapsed_ns,
        "target_cpu_ns": observation["target_cpu_nanos"],
        "p99_latency_ns": observation["p99_latency_nanos"],
        "allocation_calls": observation["allocation_calls"],
        "allocated_bytes": observation["allocated_bytes"],
        "peak_rss_bytes": int(resources["max_rss_bytes"]),
        "voluntary_context_switches": int(resources["voluntary_context_switches"]),
        "involuntary_context_switches": int(resources["involuntary_context_switches"]),
        "reorg_latency_ns": observation["reorg_latency_nanos"],
        "reorg_overlap_callbacks": observation["reorg_overlap_callbacks"],
        "shutdown_latency_ns": observation["shutdown_latency_nanos"],
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
    if any(not math.isfinite(metrics[name]) or metrics[name] <= 0 for name in positive):
        raise ValueError("benchmark emitted a non-positive required metric")
    allocations = metrics["allocation_calls"], metrics["allocated_bytes"]
    if allocation_observation == "enabled" and min(allocations) <= 0:
        raise ValueError("enabled allocation observation is empty")
    if allocation_observation == "disabled" and any(allocations):
        raise ValueError("timing binary emitted allocation counts")
    return {
        "build": build,
        "window": window,
        "wall_alignment": wall_alignment(window),
        "readiness": readiness,
        "corpus": corpus,
        "metrics": metrics,
    }


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
        environment["TX_POOL_BENCH_COMPARISON_CONTRACT"] = PROTOCOL_CONTRACT
        try:
            completed = run_process(
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
        observation = parse_attempt(completed.stdout, spans, scenario, allocation_observation)
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
        **observation,
        "output": completed.stdout,
    }


def aggregate_side(attempts: list[dict[str, object]], target_per_attempt: int) -> dict[str, object]:
    metrics = [attempt["metrics"] for attempt in attempts]
    target = target_per_attempt * len(attempts)
    aggregate = {name: sum(int(row[name]) for row in metrics) for name in SUM_METRICS}
    aggregate.update({name: max(int(row[name]) for row in metrics) for name in MAX_METRICS})
    aggregate["mean_peak_rss_bytes"] = statistics.mean(row["peak_rss_bytes"] for row in metrics)
    aggregate["throughput_tps"] = target * 1e9 / aggregate["elapsed_ns"]
    return {
        "attempt_ids": [attempt["id"] for attempt in attempts],
        "target_transactions": target,
        "minimum_target_elapsed_ns": min(int(row["elapsed_ns"]) for row in metrics),
        "metrics": aggregate,
    }


def median_interval(values: list[float], confidence: float) -> dict[str, object] | None:
    """Exact conservative order-statistic interval for independent paired ratios.

    For rank k, coverage is 1 - 2*P(Binomial(n, 1/2) < k). Ties make
    this conservative. This is pointwise coverage, not simultaneous coverage
    for a workload matrix, and it does not establish sampling independence.
    """
    if not 0 < confidence < 1:
        raise ValueError("confidence must be between zero and one")
    ordered = sorted(values)
    if any(not math.isfinite(value) or value <= 0 for value in ordered):
        raise ValueError("median ratios must be finite and positive")
    count = len(ordered)
    tail = 0
    selected = None
    for rank in range(1, count // 2 + 1):
        tail += math.comb(count, rank - 1)
        coverage = 1 - 2 * tail / (2 ** count)
        if coverage < confidence:
            break
        selected = (rank, coverage)
    if selected is None:
        return None
    rank, coverage = selected
    lower, upper = ordered[rank - 1], ordered[-rank]
    return {
        "lower": lower,
        "upper": upper,
        "lower_rank": rank,
        "upper_rank": count - rank + 1,
        "sample_count": count,
        "requested_confidence": confidence,
        "coverage_under_independent_sampling": coverage,
        "relative_width_percent": 100 * (upper - lower) / statistics.median(ordered),
    }


def metric_summary(samples: list[dict[str, object]], name: str, confidence: float = 0.95) -> dict[str, object]:
    baseline = [float(sample["baseline"]["metrics"][name]) for sample in samples]
    candidate = [float(sample["candidate"]["metrics"][name]) for sample in samples]
    ratios = [right / left for left, right in zip(baseline, candidate) if left != 0]
    interval = median_interval(ratios, confidence) if ratios and min(ratios) > 0 else None
    return {
        "baseline_median": statistics.median(baseline),
        "candidate_median": statistics.median(candidate),
        "candidate_over_baseline_ratios": ratios,
        "median_candidate_over_baseline": statistics.median(ratios) if ratios else None,
        "ratio_relative_mad_percent": relative_mad(ratios) if ratios else None,
        "median_ratio_interval": interval,
        "ratio_direction": (
            "unresolved" if interval is None
            else "higher" if interval["lower"] > 1
            else "lower" if interval["upper"] < 1
            else "unresolved"
        ),
    }


def summarize_pairs(
    samples: list[dict[str, object]],
    corpus: dict[str, object],
    mode: str,
    max_mad: float,
    confidence: float = 0.95,
    max_interval_width: float = 4.0,
    min_target_seconds: float = 0.25,
) -> dict[str, object]:
    metrics = {name: metric_summary(samples, name, confidence) for name in SUMMARY_METRICS}
    throughput_mad = metrics["throughput_tps"]["ratio_relative_mad_percent"]
    minimum_elapsed = min(sample[side]["minimum_target_elapsed_ns"] for sample in samples
                          for side in ("baseline", "candidate"))
    imprecise = [name for name in PRECISION_METRICS
                 if metrics[name]["median_ratio_interval"] is None
                 or metrics[name]["median_ratio_interval"]["relative_width_percent"] > max_interval_width]
    status = (
        "allocation_observation"
        if mode == "enabled"
        else "short_target_window"
        if minimum_elapsed < min_target_seconds * 1e9
        else "noisy"
        if throughput_mad > max_mad
        else "imprecise"
        if imprecise
        else "comparable"
    )
    metric_quality = {}
    for name, metric in metrics.items():
        interval = metric["median_ratio_interval"]
        quality = (
            "allocation_observation" if mode == "enabled"
            else "short_target_window" if minimum_elapsed < min_target_seconds * 1e9
            else "noisy" if name == "throughput_tps" and throughput_mad > max_mad
            else "imprecise" if interval is None or interval["relative_width_percent"] > max_interval_width
            else "comparable"
        )
        metric_quality[name] = {"status": quality, "required_for_overall": name in PRECISION_METRICS,
                                "aa_disposition": "not_evaluated"}
    return {
        "status": status,
        "metric_quality": metric_quality,
        "corpus": corpus,
        "paired_samples": samples,
        "metrics": metrics,
        "uncertainty_rule": {
            "method": "exact_binomial_order_statistic_median_ratio_interval",
            "coverage_scope": "pointwise_per_metric_under_independent_stable_sampling",
            "sampling_independence_established": False,
            "confidence": confidence,
            "maximum_relative_interval_width_percent": max_interval_width,
            "required_metrics": list(PRECISION_METRICS),
            "imprecise_metrics": imprecise,
        },
        "duration_rule": {
            "minimum_target_seconds": min_target_seconds,
            "observed_minimum_target_seconds": minimum_elapsed / 1e9,
        },
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


def classify_aa_equivalence(summary: dict[str, object], margin_percent: float) -> None:
    """Require quality and whole pointwise intervals inside the declared margin."""
    lower, upper = 1 - margin_percent / 100, 1 + margin_percent / 100
    within = {}
    for name in PRECISION_METRICS:
        interval = summary.get("metrics", {}).get(name, {}).get("median_ratio_interval")
        within[name] = (interval is not None and interval["lower"] >= lower
                        and interval["upper"] <= upper)
    for name, quality in summary.get("metric_quality", {}).items():
        interval = summary["metrics"][name]["median_ratio_interval"]
        equivalent = (quality["status"] == "comparable" and interval is not None
                      and interval["lower"] >= lower and interval["upper"] <= upper)
        quality["aa_disposition"] = "equivalent" if equivalent else "unresolved"
    quality_status = summary["status"]
    passed = quality_status == "comparable" and all(within.values())
    summary["aa_equivalence"] = {
        "margin_percent": margin_percent,
        "lower_ratio": lower,
        "upper_ratio": upper,
        "quality_status": quality_status,
        "whole_interval_within_margin": within,
        "passed": passed,
        "scope": "pointwise_per_metric_under_independent_stable_sampling",
        "interpretation": "Failure to establish equivalence is not proof of inequivalence. No simultaneous matrix confidence or sampling independence is established.",
    }
    summary["production_ranking_permitted"] = False
    if quality_status == "comparable":
        summary["status"] = "aa_equivalent" if passed else "aa_equivalence_unresolved"


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


def environment_snapshot() -> dict[str, object]:
    return {"captured_unix_ns": time.time_ns(), "load_average": list(os.getloadavg())}


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    for side in ("baseline", "candidate"):
        parser.add_argument(f"--{side}-root", type=Path, required=True)
        parser.add_argument(f"--{side}-binary", type=Path)
        parser.add_argument(f"--{side}-build-receipt", type=Path)
        parser.add_argument(f"--{side}-aa-result", type=Path)
        parser.add_argument(f"--{side}-target-dir", type=Path)
        parser.add_argument(f"--{side}-build-features", default="")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--resume", action="store_true")
    parser.add_argument("--runs", type=int, default=10)
    parser.add_argument("--replicates-per-sample", type=int, default=1)
    parser.add_argument("--order-seed", type=int, default=0, help="Frozen balanced block-order seed")
    parser.add_argument("--initial-cooldown-seconds", type=float, default=15.0)
    parser.add_argument("--cooldown-seconds", type=float, default=10.0)
    parser.add_argument("--max-paired-mad-percent", type=float, default=1.5)
    parser.add_argument("--confidence-level", type=float, default=0.95)
    parser.add_argument("--max-ratio-interval-width-percent", type=float, default=4.0)
    parser.add_argument("--min-target-seconds", type=float, default=0.25)
    parser.add_argument("--calibration-only", action="store_true")
    parser.add_argument("--comparison", choices=("ab", "aa"), default="ab")
    parser.add_argument("--aa-equivalence-margin-percent", type=float)
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
    numeric = (args.initial_cooldown_seconds, args.cooldown_seconds, args.timeout_seconds,
               args.max_paired_mad_percent, args.confidence_level,
               args.max_ratio_interval_width_percent, args.min_target_seconds)
    if not all(math.isfinite(value) for value in numeric) or min(
        args.initial_cooldown_seconds, args.cooldown_seconds
    ) < 0 or min(numeric[2:]) <= 0 or args.confidence_level >= 1:
        parser.error("cooldowns must be non-negative and limits positive")
    if median_interval([1.0] * args.runs, args.confidence_level) is None:
        parser.error("--runs cannot support a finite interval at --confidence-level")
    if args.comparison == "aa" and (args.baseline_build_receipt is None or args.candidate_build_receipt is None):
        parser.error("A/A requires build receipts for the same fixed binary")
    margin = args.aa_equivalence_margin_percent
    if margin is not None and (args.comparison != "aa" or not math.isfinite(margin) or not 0 < margin < 100):
        parser.error("--aa-equivalence-margin-percent requires A/A and a finite value between 0 and 100")
    if args.comparison == "aa" and not args.calibration_only and margin is None:
        parser.error("A/A measurements require --aa-equivalence-margin-percent declared before execution")
    if args.comparison == "aa" and args.allocation_observation != "disabled":
        parser.error("A/A timing/CPU/RSS equivalence requires allocation observation disabled")
    if args.resume != args.output.exists():
        parser.error("--resume requires an existing output; a new run requires a new output")
    for side in ("baseline", "candidate"):
        receipt = getattr(args, f"{side}_build_receipt")
        if getattr(args, f"{side}_binary") is not None and receipt is None:
            parser.error(f"--{side}-binary requires --{side}-build-receipt")
        if receipt is not None and getattr(args, f"{side}_target_dir") is not None:
            parser.error(f"--{side}-target-dir cannot accompany a build receipt")
        if getattr(args, f"{side}_aa_result") is not None and (
                args.comparison != "ab" or args.allocation_observation != "disabled" or args.calibration_only):
            parser.error("A/A evidence applies only to uninstrumented A/B measurements")
        try:
            setattr(args, f"{side}_build_features", effective_features(
                getattr(args, f"{side}_build_features"), args.allocation_observation == "enabled", "profile_one_shot"))
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
        "order_seed": args.order_seed,
        "schedule": {scenario_key(scenario): balanced_schedule(args.runs, args.replicates_per_sample, args.order_seed, scenario_key(scenario)) for scenario in scenarios},
        "replicates_per_sample": args.replicates_per_sample,
        "initial_cooldown_seconds": args.initial_cooldown_seconds,
        "cooldown_seconds": args.cooldown_seconds,
        "max_paired_mad_percent": args.max_paired_mad_percent,
        "confidence_level": args.confidence_level,
        "max_ratio_interval_width_percent": args.max_ratio_interval_width_percent,
        "min_target_seconds": args.min_target_seconds,
        "calibration_only": args.calibration_only,
        "comparison": args.comparison,
        "aa_equivalence_margin_percent": args.aa_equivalence_margin_percent,
        "timeout_seconds": args.timeout_seconds,
        "allocation_observation": args.allocation_observation,
        "build_receipts": {side: binary_record(path) if (path := getattr(args, f"{side}_build_receipt")) else None
                           for side in ("baseline", "candidate")},
        "aa_evidence": {side: binary_record(path) if (path := getattr(args, f"{side}_aa_result")) else None
                        for side in ("baseline", "candidate")},
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
    previous_outcome = cached.get("outcome") if cached is not None else None
    if cached is not None:
        if cached.get("side") != side or cached.get("scenario") != scenario:
            raise RuntimeError(f"checkpoint attempt identity drifted: {attempt_id}")
        if cached.get("outcome") == "running":
            cached.update(outcome="failure", category="interrupted_attempt",
                          detail="The recorded attempt started but did not complete; it cannot be silently rerun.")
        if cached.get("outcome") not in {"success", "failure"}:
            raise RuntimeError(f"checkpoint attempt outcome is invalid: {attempt_id}")
        attempt = cached
        if attempt["outcome"] == "success":
            # Resume and A/A replay share the raw observation authority. Cached
            # metrics are derived data; command, timestamps and environment survive.
            try:
                attempt.update(parse_attempt(attempt["output"], None, scenario,
                                             args.allocation_observation))
            except (KeyError, TypeError, ValueError) as error:
                attempt.update(outcome="failure", category="invalid_evidence", detail=str(error))
    else:
        print(f">>> {attempt_id}", flush=True)
        attempt = {"id": attempt_id, "side": side, "scenario": scenario,
                   "outcome": "running", "started_unix_ns": time.time_ns(),
                   "environment_before": environment_snapshot()}
        record["attempts"].append(attempt)
        indexed[attempt_id] = attempt
        write_checkpoint(output, record)
        try:
            result = run_attempt(
                context["binary"],
                Path(context["source"]["root"]),
                scenario,
                side,
                attempt_id,
                args.timeout_seconds,
                args.allocation_observation,
            )
        except BaseException as error:
            attempt.update(outcome="failure", category="runner_interrupted",
                           detail=type(error).__name__, ended_unix_ns=time.time_ns(),
                           output=timeout_output(error), environment_after=environment_snapshot())
            write_checkpoint(output, record)
            raise
        attempt.update(result)
        attempt["environment_after"] = environment_snapshot()
    if attempt["outcome"] == "success" and expected_corpus is not None and attempt["corpus"] != expected_corpus:
        attempt.update(
            outcome="failure",
            category="corpus_drift",
            detail="corpus changed after the paired pilot",
        )
    # Rebuilt cached observations are saved with the scenario summary. Only new
    # attempts or newly discovered failures need an immediate whole-ledger write.
    if cached is None or attempt["outcome"] != previous_outcome:
        write_checkpoint(output, record)
    if cached is None:
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
    minimum_elapsed = min(attempt["metrics"]["elapsed_ns"] for attempt in pilots)
    if args.calibration_only or (args.allocation_observation == "disabled"
                                and minimum_elapsed < args.min_target_seconds * 1e9):
        record["summary"][key] = {
            "status": "calibrated" if minimum_elapsed >= args.min_target_seconds * 1e9 else "short_target_window",
            "pilot_attempt_ids": [attempt["id"] for attempt in pilots],
            "minimum_target_seconds": args.min_target_seconds,
            "observed_minimum_target_seconds": minimum_elapsed / 1e9,
            "ranking_permitted": False,
        }
        write_checkpoint(output, record)
        return
    samples = []
    failures = []
    schedule = balanced_schedule(args.runs, args.replicates_per_sample, args.order_seed, key)
    for pair_number in range(1, args.runs + 1):
        paired: dict[str, list[dict[str, object]]] = {"baseline": [], "candidate": []}
        for replicate in range(1, args.replicates_per_sample + 1):
            order = schedule[pair_number - 1][replicate - 1]
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
            args.confidence_level,
            args.max_ratio_interval_width_percent,
            args.min_target_seconds,
        )
    )
    if args.comparison == "aa":
        classify_aa_equivalence(record["summary"][key], args.aa_equivalence_margin_percent)
    write_checkpoint(output, record)


def replay_aa_row(record: dict[str, object], scenario: dict[str, object]) -> dict[str, object]:
    """Rebuild paired samples from raw scheduled attempts, never a saved summary."""
    config, key = record["configuration"], scenario_key(scenario)
    indexed = attempt_index(record)
    corpus = None

    def observation(side, attempt_id):
        nonlocal corpus
        attempt = indexed.get(attempt_id)
        if attempt is None or attempt.get("outcome") != "success":
            raise ValueError(f"A/A attempt is missing or failed: {attempt_id}")
        if attempt.get("side") != side or attempt.get("scenario") != scenario:
            raise ValueError(f"A/A attempt identity differs: {attempt_id}")
        parsed = parse_attempt(attempt["output"], None, scenario, "disabled")
        if corpus is not None and parsed["corpus"] != corpus:
            raise ValueError("A/A corpus changed between attempts")
        corpus = parsed["corpus"]
        return dict(parsed, id=attempt_id)

    try:
        pilots = [observation(side, f"{key}/pilot/{side}") for side in ("candidate", "baseline")]
        if min(pilot["metrics"]["elapsed_ns"] for pilot in pilots) < config["min_target_seconds"] * 1e9:
            raise ValueError("A/A pilot target window is short")
        schedule = balanced_schedule(config["runs"], config["replicates_per_sample"], config["order_seed"], key)
        if config["schedule"].get(key) != schedule:
            raise ValueError("A/A schedule differs from its frozen configuration")
        samples = []
        expected_ids = [pilot["id"] for pilot in pilots]
        for pair, block in enumerate(schedule, 1):
            paired = {"baseline": [], "candidate": []}
            for replicate, order in enumerate(block, 1):
                for side in order:
                    attempt_id = f"{key}/pair-{pair}/replicate-{replicate}/{side}"
                    expected_ids.append(attempt_id)
                    paired[side].append(observation(side, attempt_id))
            samples.append({side: aggregate_side(attempts, scenario["target"])
                            for side, attempts in paired.items()})
        actual_ids = [attempt["id"] for attempt in record["attempts"] if attempt.get("scenario") == scenario]
        if actual_ids != expected_ids:
            raise ValueError("A/A attempt membership or execution order differs")
        summary = summarize_pairs(samples, corpus, "disabled", config["max_paired_mad_percent"],
                                  config["confidence_level"], config["max_ratio_interval_width_percent"],
                                  config["min_target_seconds"])
        classify_aa_equivalence(summary, config["aa_equivalence_margin_percent"])
        return summary
    except (KeyError, TypeError, ValueError) as error:
        return {"status": "non_comparable", "reason": str(error), "production_ranking_permitted": False}


def load_aa_evidence(record: dict[str, object]) -> dict[str, object]:
    """Bind each control to its measured arm; different production arms stay independent."""
    evidence = {}
    config = record["configuration"]
    for side, identity in config["aa_evidence"].items():
        if identity is None:
            continue
        path = Path(identity["path"])
        if binary_record(path) != identity:
            raise RuntimeError(f"{side} A/A evidence changed")
        control = read_checkpoint(path)
        aa_config = control.get("configuration", {})
        if (control.get("schema") != SCHEMA_VERSION or control.get("complete") is not True
                or aa_config.get("comparison") != "aa" or aa_config.get("calibration_only") is not False
                or aa_config.get("allocation_observation") != "disabled"):
            raise RuntimeError(f"{side} A/A evidence is incomplete or not a timing control")
        for field in ("runner_sha256", "build_runner_sha256", "process_runner_sha256",
                      "harness_sha256", "measurement_window_sha256", "rejection_diagnostics_sha256", "scenario_parser_sha256", "observation_parser_sha256",
                      "host", "metric_scopes"):
            if control.get(field) != record[field]:
                raise RuntimeError(f"{side} A/A {field} differs")
        for field in ("replicates_per_sample", "initial_cooldown_seconds", "cooldown_seconds",
                      "max_paired_mad_percent", "confidence_level", "max_ratio_interval_width_percent",
                      "min_target_seconds", "timeout_seconds"):
            if aa_config.get(field) != config[field]:
                raise RuntimeError(f"{side} A/A estimator or execution setting differs: {field}")
        margin = aa_config.get("aa_equivalence_margin_percent")
        if not isinstance(margin, (float, int)) or not math.isfinite(margin) or not 0 < margin < 100:
            raise RuntimeError(f"{side} A/A equivalence margin is invalid")
        measured = record["sides"][side]
        # Checkout locations are not source inputs; executable bytes are checked separately.
        source_inputs = {name: value for name, value in measured["source"].items() if name != "root"}
        for arm in ("baseline", "candidate"):
            control_arm = control.get("sides", {}).get(arm, {})
            control_source = control_arm.get("source", {})
            if ({name: value for name, value in control_source.items() if name != "root"} != source_inputs
                    or control_arm.get("consensus") != measured["consensus"]):
                raise RuntimeError(f"{side} A/A source or consensus differs")
            validate_build(control_arm["build"], control_source, "profile_one_shot",
                           measured["build"]["features"], control_arm["binary"])
            if any(control_arm.get("build", {}).get(field) != measured["build"].get(field)
                   for field in ("bench", "features", "profile", "toolchain")):
                raise RuntimeError(f"{side} A/A build contract differs")
            if any(control_arm.get("binary", {}).get(field) != measured["binary"][field]
                   for field in ("sha256", "size")):
                raise RuntimeError(f"{side} A/A binary differs")
        evidence[side] = {scenario_key(scenario): replay_aa_row(control, scenario)
                          for scenario in config["scenarios"] if scenario in aa_config.get("scenarios", [])}
    return evidence


def qualify_ranking(summary: dict[str, object], evidence: dict[str, object],
                    scenario: dict[str, object], allocation: str) -> None:
    """Quality is observable without a control; timing ranking needs both applicable controls."""
    summary["production_ranking_permitted"] = False
    if allocation == "enabled":
        for name, quality in summary.get("metric_quality", {}).items():
            quality["aa_disposition"] = "not_applicable"
            interval = summary["metrics"][name]["median_ratio_interval"]
            quality["ranking_permitted"] = (name in ("allocation_calls", "allocated_bytes")
                and interval is not None
                and interval["relative_width_percent"] <= summary["uncertainty_rule"]["maximum_relative_interval_width_percent"])
        return
    summary["aa_controls"] = {}
    if "corpus" not in summary:
        return
    controls = {side: evidence.get(side, {}).get(scenario_key(scenario)) for side in ("baseline", "candidate")}
    for side, row in controls.items():
        status = ("missing" if row is None else "non_comparable" if "reason" in row
                  else "corpus_mismatch" if row.get("corpus") != summary.get("corpus") else row["status"])
        summary["aa_controls"][side] = {"status": status}
        if row is not None and "reason" in row:
            summary["aa_controls"][side]["reason"] = row["reason"]
    for name, quality in summary.get("metric_quality", {}).items():
        equivalent = all(row is not None and row.get("corpus") == summary["corpus"]
            and row.get("metric_quality", {}).get(name, {}).get("aa_disposition") == "equivalent"
            for row in controls.values())
        quality["aa_disposition"] = "equivalent" if equivalent else "not_evaluated" if not evidence else "unresolved"
        quality["ranking_permitted"] = quality["status"] == "comparable" and equivalent
    summary["production_ranking_permitted"] = (summary["status"] == "comparable"
        and all(summary["metric_quality"][name]["ranking_permitted"] for name in PRECISION_METRICS))


def validate_frozen(
    record: dict[str, object],
    contexts: dict[str, dict[str, object]],
    harness_hash: str,
    host: dict[str, object],
    supplied: dict[str, Path | None],
) -> None:
    for name in ("build_receipts", "aa_evidence"):
        for identity in record.get("configuration", {}).get(name, {}).values():
            if identity is not None and binary_record(Path(identity["path"])) != identity:
                raise RuntimeError(f"frozen {name} input changed")
    if record.get("metric_scopes") != METRIC_SCOPES:
        raise RuntimeError("measurement metric scopes changed")
    if record.get("build_runner_sha256") != sha256(Path(__file__).with_name("benchmark_build.py")):
        raise RuntimeError("benchmark build runner changed")
    if record.get("runner_sha256") != sha256(Path(__file__)):
        raise RuntimeError("benchmark runner changed")
    if record.get("process_runner_sha256") != sha256(Path(__file__).with_name("measurement_process.py")):
        raise RuntimeError("measurement process runner changed")
    if record.get("measurement_window_sha256") != sha256(Path(__file__).with_name("measurement_window.py")):
        raise RuntimeError("measurement window parser changed")
    if record.get("rejection_diagnostics_sha256") != sha256(Path(__file__).with_name("rejection_diagnostics.py")):
        raise RuntimeError("rejection diagnostic verifier changed")
    if record.get("scenario_parser_sha256") != sha256(Path(__file__).with_name("benchmark_scenario.py")):
        raise RuntimeError("benchmark scenario parser changed")
    if record.get("observation_parser_sha256") != sha256(Path(__file__).with_name("measurement_observation.py")):
        raise RuntimeError("workload observation parser changed")
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
        root = Path(current["source"]["root"])
        if git_record(root) != current["source"] or bundle_hash(root) != harness_hash:
            raise RuntimeError(f"{side} source changed during measurement")
        if binary_record(Path(str(frozen["binary"]["path"]))) != frozen["binary"]:
            raise RuntimeError(f"{side} binary changed")
        if supplied[side] is not None and binary_record(supplied[side]) != frozen["binary"]:
            raise RuntimeError(f"supplied {side} binary differs from the checkpoint")
        current["binary"], current["build"] = frozen["binary"], frozen["build"]


def main() -> None:
    args = arguments()
    scenarios = [parse_scenario(value) for value in args.scenario]
    if len({scenario_key(scenario) for scenario in scenarios}) != len(scenarios):
        raise RuntimeError("duplicate scenarios would reuse the same attempt IDs")
    roots = {side: getattr(args, f"{side}_root").resolve() for side in ("baseline", "candidate")}
    if roots["baseline"] == roots["candidate"] and args.comparison != "aa":
        raise RuntimeError("baseline and candidate roots must be distinct")
    harness_hash = bundle_hash(roots["baseline"])
    if bundle_hash(roots["candidate"]) != harness_hash:
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
        if all(getattr(args, f"{side}_build_receipt") is None for side in supplied) and targets["baseline"].resolve() == targets["candidate"].resolve():
            raise RuntimeError("baseline and candidate target directories must be isolated")
        for side in ("baseline", "candidate"):
            features = getattr(args, f"{side}_build_features")
            receipt = getattr(args, f"{side}_build_receipt")
            if receipt is not None:
                binary, build = load_build(receipt, roots[side], "profile_one_shot", features, supplied[side])
            else:
                build = build_binary(roots[side], targets[side], features, "profile_one_shot")
                binary = build["binary"]
            contexts[side].update(binary=binary, build=build)
        record = {
            "schema": SCHEMA_VERSION,
            "runner_sha256": sha256(Path(__file__)),
            "scenario_parser_sha256": sha256(Path(__file__).with_name("benchmark_scenario.py")),
            "observation_parser_sha256": sha256(Path(__file__).with_name("measurement_observation.py")),
            "build_runner_sha256": sha256(Path(__file__).with_name("benchmark_build.py")),
            "process_runner_sha256": sha256(Path(__file__).with_name("measurement_process.py")),
            "harness_sha256": harness_hash,
            "harness_bundle": harness_bundle(roots["baseline"]),
            "measurement_window_sha256": sha256(Path(__file__).with_name("measurement_window.py")),
            "rejection_diagnostics_sha256": sha256(Path(__file__).with_name("rejection_diagnostics.py")),
            "host": host,
            "configuration": config,
            "metric_scopes": METRIC_SCOPES,
            "sides": contexts,
            "environment": {"starts": [], "end": None},
            "attempts": [],
            "summary": {},
            "complete": False,
        }
    if args.comparison == "aa" and contexts["baseline"]["binary"]["sha256"] != contexts["candidate"]["binary"]["sha256"]:
        raise RuntimeError("A/A binary hashes differ")
    aa_evidence = load_aa_evidence(record)
    record["environment"]["starts"].append(environment_snapshot())
    write_checkpoint(args.output, record)
    cool(args.initial_cooldown_seconds)
    indexed = attempt_index(record)
    for scenario in scenarios:
        run_scenario(record, indexed, args.output, contexts, scenario, args)
        if args.comparison == "ab" and not args.calibration_only:
            qualify_ranking(record["summary"][scenario_key(scenario)], aa_evidence, scenario,
                            args.allocation_observation)
            write_checkpoint(args.output, record)
    validate_frozen(record, contexts, harness_hash, host, supplied)
    record["environment"]["end"] = environment_snapshot()
    record["complete"] = True
    write_checkpoint(args.output, record)
    print(json.dumps(record["summary"], indent=2, sort_keys=True))
    print(f">>> saved {args.output}")
    accepted_status = ("calibrated" if args.calibration_only else "aa_equivalent"
                       if args.comparison == "aa" else "comparable"
                       if args.allocation_observation == "disabled" else "allocation_observation")
    if any(summary["status"] != accepted_status for summary in record["summary"].values()) and not args.allow_noncomparable:
        raise SystemExit(2)


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "__measure_child__":
        raise SystemExit(measure_child())
    main()
