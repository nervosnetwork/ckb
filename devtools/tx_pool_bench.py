#!/usr/bin/env python3
"""Run, record, and compare the tx-pool Criterion benchmark.

Examples:
    # One quick smoke run
    python3 devtools/tx_pool_bench.py --quick

    # Record an inter-run median baseline
    python3 devtools/tx_pool_bench.py --runs 3 --save-json /tmp/tx-pool-baseline.json

    # Compare the current tree and fail on any measured regression
    python3 devtools/tx_pool_bench.py --runs 3 \
        --compare-json /tmp/tx-pool-baseline.json \
        --save-json /tmp/tx-pool-candidate.json \
        --fail-on-regression
"""

import argparse
import datetime
import hashlib
import json
import math
import os
import platform
import re
import statistics
import subprocess
import sys
from pathlib import Path
from typing import Dict, Iterable, List, Tuple

ResultKey = Tuple[int, int, bool, str, int]
Result = Dict[str, float]
WORKSPACE_ROOT = Path(__file__).resolve().parent.parent
# Cargo's fingerprint cache is not worktree-aware when callers force two
# checkouts through one CARGO_TARGET_DIR. Keep each checkout's benchmark binary
# isolated so a baseline executable can never be mistaken for the candidate.
BENCH_TARGET_DIR = WORKSPACE_ROOT / "target" / "tx-pool-bench"
HARNESS_FILES = (
    Path(__file__).resolve(),
    WORKSPACE_ROOT / "tx-pool" / "src" / "benchmark.rs",
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    matrix = parser.add_mutually_exclusive_group()
    matrix.add_argument("--quick", action="store_true", help="run the reduced matrix")
    matrix.add_argument("--full", action="store_true", help="run the full matrix")
    parser.add_argument(
        "--runs",
        type=int,
        default=1,
        help="number of complete benchmark repetitions to aggregate by median",
    )
    parser.add_argument(
        "--save-json",
        type=Path,
        help="write machine-readable metadata, samples, and medians",
    )
    parser.add_argument(
        "--compare-json",
        type=Path,
        help="compare against a JSON record produced by this script",
    )
    parser.add_argument(
        "--fail-on-regression",
        action="store_true",
        help="exit non-zero when any scenario or aggregate throughput regresses",
    )
    parser.add_argument(
        "--regression-threshold-percent",
        type=float,
        default=None,
        help=(
            "allowed per-scenario regression before failure (default: 2 for "
            "quick diagnostics, strict 0 for medium/full)"
        ),
    )
    parser.add_argument(
        "--max-run-spread-percent",
        type=float,
        default=None,
        help=(
            "maximum allowed max-min throughput spread across repetitions; "
            "a noisier record is invalid for --fail-on-regression (default: "
            "7 for quick diagnostics, 5 for medium/full)"
        ),
    )
    parser.add_argument(
        "--filter",
        dest="benchmark_filter",
        help=(
            "run only benchmark IDs containing this text (for example "
            "'always_success_100' or 'child_first_20')"
        ),
    )
    args = parser.parse_args()
    if args.regression_threshold_percent is None:
        args.regression_threshold_percent = 2.0 if args.quick else 0.0
    if args.max_run_spread_percent is None:
        args.max_run_spread_percent = 7.0 if args.quick else 5.0
    if args.runs < 1:
        parser.error("--runs must be at least 1")
    if args.regression_threshold_percent < 0:
        parser.error("--regression-threshold-percent cannot be negative")
    if args.max_run_spread_percent <= 0:
        parser.error("--max-run-spread-percent must be positive")
    return args


ARGS = parse_args()


def matrix_mode() -> str:
    if ARGS.quick:
        return "quick"
    if ARGS.full:
        return "full"
    return "medium"


def command_output(args: List[str]) -> str:
    try:
        return subprocess.check_output(args, text=True).strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


def files_sha256(paths: Iterable[Path]) -> str:
    digest = hashlib.sha256()
    for path in paths:
        digest.update(str(path.relative_to(WORKSPACE_ROOT)).encode("utf-8"))
        digest.update(b"\0")
        digest.update(path.read_bytes())
        digest.update(b"\0")
    return digest.hexdigest()


def run_cargo_bench(run: int) -> str:
    cmd = [
        "cargo",
        "bench",
        "-p",
        "ckb-tx-pool",
        "--features",
        "internal",
        "--bench",
        "pipeline",
    ]
    if ARGS.benchmark_filter:
        cmd.extend(["--", ARGS.benchmark_filter])
    print(
        f"\n>>> Run {run}/{ARGS.runs}: {' '.join(cmd)} ({matrix_mode()} matrix)",
        flush=True,
    )

    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = str(BENCH_TARGET_DIR)
    env.pop("QUICK_BENCH", None)
    env.pop("FULL_BENCH", None)
    if ARGS.quick:
        env["QUICK_BENCH"] = "1"
    elif ARGS.full:
        env["FULL_BENCH"] = "1"

    proc = subprocess.Popen(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        env=env,
    )
    lines: List[str] = []
    assert proc.stdout is not None
    for line in proc.stdout:
        print(line, end="", flush=True)
        lines.append(line)
    proc.wait()
    if proc.returncode != 0:
        raise RuntimeError(f"cargo bench failed in repetition {run}")
    return "".join(lines)


TIME_RE = re.compile(
    r"time:\s+\["
    r"(?P<lo>[\d.]+)\s+(?P<lo_unit>\w+)\s+"
    r"(?P<med>[\d.]+)\s+(?P<med_unit>\w+)\s+"
    r"(?P<hi>[\d.]+)\s+(?P<hi_unit>\w+)"
    r"\]"
)
THRPT_RE = re.compile(
    r"thrpt:\s+\["
    r"(?P<lo>[\d.]+)\s+(?P<lo_unit>[\w/]+)\s+"
    r"(?P<med>[\d.]+)\s+(?P<med_unit>[\w/]+)\s+"
    r"(?P<hi>[\d.]+)\s+(?P<hi_unit>[\w/]+)"
    r"\]"
)
NAME_RE = re.compile(
    r"^tx_pool_pipeline/pipeline_"
    r"(?P<peers>\d+)peer_"
    r"(?P<workers>\d+)worker_"
    r"(?P<warm>warm_)?"
    r"(?P<tx_type>always_success|secp256k1|"
    r"dependent_always_success_(?:parent_first|child_first)|"
    r"dependent_secp_(?:parent_first|child_first))_"
    r"(?P<size>\d+)$"
)


def to_ms(value: float, unit: str) -> float:
    factors = {"ns": 1e-6, "us": 1e-3, "ms": 1.0, "s": 1e3}
    try:
        return value * factors[unit]
    except KeyError as exc:
        raise ValueError(f"unknown time unit: {unit}") from exc


def to_elem_s(value: float, unit: str) -> float:
    factors = {"elem/s": 1.0, "Kelem/s": 1e3, "Melem/s": 1e6}
    try:
        return value * factors[unit]
    except KeyError as exc:
        raise ValueError(f"unknown throughput unit: {unit}") from exc


def parse_output(text: str) -> Dict[ResultKey, Result]:
    lines = text.splitlines()
    results: Dict[ResultKey, Result] = {}
    i = 0
    while i < len(lines):
        line = lines[i].strip()
        match = NAME_RE.match(line)
        if not match:
            i += 1
            continue

        key: ResultKey = (
            int(match.group("peers")),
            int(match.group("workers")),
            match.group("warm") is not None,
            match.group("tx_type"),
            int(match.group("size")),
        )
        time_ms = None
        throughput = None
        j = i + 1
        while j < len(lines) and not NAME_RE.match(lines[j].strip()):
            time_match = TIME_RE.search(lines[j])
            if time_match:
                time_ms = to_ms(
                    float(time_match.group("med")), time_match.group("med_unit")
                )
            throughput_match = THRPT_RE.search(lines[j])
            if throughput_match:
                throughput = to_elem_s(
                    float(throughput_match.group("med")),
                    throughput_match.group("med_unit"),
                )
            j += 1

        if time_ms is None or throughput is None:
            raise RuntimeError(f"could not parse complete result for {format_key(key)}")
        results[key] = {"time_ms": time_ms, "throughput": throughput}
        i = j

    if not results:
        raise RuntimeError("no tx-pool benchmark results were parsed")
    return results


def format_key(key: ResultKey) -> str:
    peers, workers, warm, tx_type, size = key
    return (
        f"{peers}peer/{workers}worker/"
        f"{'warm' if warm else 'cold'}/{tx_type}/{size}"
    )


def aggregate_runs(
    runs: Iterable[Dict[ResultKey, Result]],
) -> Tuple[Dict[ResultKey, Result], Dict[ResultKey, Dict[str, List[float]]]]:
    runs = list(runs)
    expected = set(runs[0])
    for index, run in enumerate(runs[1:], start=2):
        if set(run) != expected:
            missing = sorted(format_key(key) for key in expected - set(run))
            extra = sorted(format_key(key) for key in set(run) - expected)
            raise RuntimeError(
                f"benchmark repetition {index} has a different matrix; "
                f"missing={missing}, extra={extra}"
            )

    samples: Dict[ResultKey, Dict[str, List[float]]] = {}
    medians: Dict[ResultKey, Result] = {}
    for key in sorted(expected):
        times = [run[key]["time_ms"] for run in runs]
        throughputs = [run[key]["throughput"] for run in runs]
        samples[key] = {"time_ms": times, "throughput": throughputs}
        medians[key] = {
            "time_ms": statistics.median(times),
            "throughput": statistics.median(throughputs),
        }
    return medians, samples


def environment_metadata() -> Dict:
    return {
        "git_commit": command_output(["git", "rev-parse", "HEAD"]),
        "git_tracked_changes": command_output(
            ["git", "status", "--porcelain", "--untracked-files=no"]
        ),
        "rustc": command_output(["rustc", "--version", "--verbose"]),
        "platform": platform.platform(),
        "machine": platform.machine(),
        "python": platform.python_version(),
        "benchmark_harness_sha256": files_sha256(HARNESS_FILES),
        "benchmark_target_dir": str(BENCH_TARGET_DIR),
        "benchmark_filter": ARGS.benchmark_filter,
    }


def make_record(
    medians: Dict[ResultKey, Result],
    samples: Dict[ResultKey, Dict[str, List[float]]],
) -> Dict:
    entries = []
    for key in sorted(medians):
        peers, workers, warm, tx_type, size = key
        entries.append(
            {
                "peers": peers,
                "workers": workers,
                "warm": warm,
                "tx_type": tx_type,
                "size": size,
                "time_ms": medians[key]["time_ms"],
                "throughput": medians[key]["throughput"],
                "time_ms_samples": samples[key]["time_ms"],
                "throughput_samples": samples[key]["throughput"],
                "throughput_run_spread_percent": relative_run_spread(
                    samples[key]["throughput"]
                ),
            }
        )
    record = {
        "schema": 1,
        "generated_at_utc": datetime.datetime.now(
            datetime.timezone.utc
        ).isoformat(),
        "mode": matrix_mode(),
        "runs": ARGS.runs,
        "results": entries,
    }
    record.update(environment_metadata())
    return record


def record_results(record: Dict) -> Dict[ResultKey, Result]:
    if record.get("schema") != 1:
        raise RuntimeError(f"unsupported benchmark JSON schema: {record.get('schema')}")
    results: Dict[ResultKey, Result] = {}
    for item in record["results"]:
        key: ResultKey = (
            int(item["peers"]),
            int(item["workers"]),
            bool(item["warm"]),
            str(item["tx_type"]),
            int(item["size"]),
        )
        results[key] = {
            "time_ms": float(item["time_ms"]),
            "throughput": float(item["throughput"]),
        }
    return results


def relative_run_spread(values: List[float]) -> float:
    if not values:
        raise RuntimeError("benchmark record has no run samples")
    median = statistics.median(values)
    if median <= 0:
        raise RuntimeError("benchmark run sample median must be positive")
    return (max(values) - min(values)) / median * 100.0


def validate_run_stability(record: Dict, label: str) -> None:
    unstable = []
    for item in record.get("results", []):
        samples = [float(value) for value in item.get("throughput_samples", [])]
        if len(samples) != int(record.get("runs", 1)):
            raise RuntimeError(
                f"{label} record has incomplete run samples for "
                f"{item.get('tx_type')}: {len(samples)} != {record.get('runs')}"
            )
        spread = relative_run_spread(samples)
        if spread > ARGS.max_run_spread_percent:
            unstable.append(
                f"{item.get('tx_type')} "
                f"({'warm' if item.get('warm') else 'cold'})={spread:.2f}%"
            )
    if unstable:
        raise RuntimeError(
            f"{label} benchmark is too noisy for a release decision "
            f"(limit={ARGS.max_run_spread_percent:.2f}%): " + ", ".join(unstable)
        )


def validate_comparison_environment(baseline: Dict, current: Dict) -> None:
    """Reject comparisons whose sampling or host context is not symmetric."""
    if baseline.get("mode") != current.get("mode"):
        raise RuntimeError(
            "baseline matrix mode differs: "
            f"{baseline.get('mode')} != {current.get('mode')}"
        )

    if baseline.get("benchmark_filter") != current.get("benchmark_filter"):
        raise RuntimeError(
            "baseline benchmark filter differs: "
            f"{baseline.get('benchmark_filter')!r} != "
            f"{current.get('benchmark_filter')!r}"
        )

    mismatches = []
    for field in ("rustc", "platform", "machine", "benchmark_harness_sha256"):
        if baseline.get(field) != current.get(field):
            mismatches.append(
                f"{field}: {baseline.get(field)!r} != {current.get(field)!r}"
            )
    if mismatches:
        raise RuntimeError(
            "benchmark environment differs; record a new same-host baseline: "
            + "; ".join(mismatches)
        )

    if ARGS.fail_on_regression:
        for label, record in (("baseline", baseline), ("candidate", current)):
            tracked_changes = record.get("git_tracked_changes", "unknown")
            if tracked_changes != "":
                raise RuntimeError(
                    f"{label} benchmark must come from a clean tracked tree; "
                    f"git status was {tracked_changes!r}"
                )
        baseline_runs = int(baseline.get("runs", 1))
        current_runs = int(current.get("runs", 1))
        if baseline_runs < 3 or current_runs < 3:
            raise RuntimeError(
                "--fail-on-regression requires at least three complete runs "
                f"on both sides (baseline={baseline_runs}, current={current_runs})"
            )
        if baseline_runs != current_runs:
            raise RuntimeError(
                "--fail-on-regression requires symmetric repetition counts "
                f"(baseline={baseline_runs}, current={current_runs})"
            )
        if baseline.get("results") is not None:
            validate_run_stability(baseline, "baseline")
        if current.get("results") is not None:
            validate_run_stability(current, "candidate")


def print_summary(
    results: Dict[ResultKey, Result],
    samples: Dict[ResultKey, Dict[str, List[float]]],
) -> None:
    print("\n# Tx-Pool Pipeline Benchmark\n")
    print("| scenario | median time | median throughput | run spread |")
    print("|---|---:|---:|---:|")
    for key, value in sorted(results.items()):
        print(
            f"| {format_key(key)} | {value['time_ms']:.3f} ms | "
            f"{value['throughput']:.2f} tx/s | "
            f"{relative_run_spread(samples[key]['throughput']):.2f}% |"
        )


def compare_results(
    baseline: Dict[ResultKey, Result], current: Dict[ResultKey, Result]
) -> bool:
    baseline_keys = set(baseline)
    current_keys = set(current)
    if baseline_keys != current_keys:
        missing = sorted(format_key(key) for key in baseline_keys - current_keys)
        extra = sorted(format_key(key) for key in current_keys - baseline_keys)
        raise RuntimeError(
            f"comparison matrix differs; missing={missing}, extra={extra}"
        )

    threshold = ARGS.regression_threshold_percent
    ratios = []
    failed = False
    print("\n# Comparison against baseline\n")
    print("| scenario | throughput delta | latency delta | verdict |")
    print("|---|---:|---:|---|")
    for key in sorted(current):
        old = baseline[key]
        new = current[key]
        throughput_ratio = new["throughput"] / old["throughput"]
        latency_ratio = new["time_ms"] / old["time_ms"]
        throughput_delta = (throughput_ratio - 1.0) * 100.0
        latency_delta = (latency_ratio - 1.0) * 100.0
        ratios.append(throughput_ratio)
        regressed = (
            throughput_delta < -threshold or latency_delta > threshold
        )
        failed = failed or regressed
        print(
            f"| {format_key(key)} | {throughput_delta:+.2f}% | "
            f"{latency_delta:+.2f}% | {'REGRESSION' if regressed else 'pass'} |"
        )

    geomean = math.exp(sum(math.log(ratio) for ratio in ratios) / len(ratios))
    aggregate_delta = (geomean - 1.0) * 100.0
    aggregate_regressed = aggregate_delta < -threshold
    failed = failed or aggregate_regressed
    print(
        f"\nThroughput geometric mean: {aggregate_delta:+.2f}% "
        f"({'REGRESSION' if aggregate_regressed else 'pass'})"
    )
    return failed


def main() -> None:
    baseline_record = None
    if ARGS.compare_json:
        baseline_record = json.loads(ARGS.compare_json.read_text(encoding="utf-8"))
        # Reject an invalid release comparison before spending minutes or hours
        # running its candidate side.
        current_environment = {
            "mode": matrix_mode(),
            "runs": ARGS.runs,
            **environment_metadata(),
        }
        validate_comparison_environment(baseline_record, current_environment)

    run_results = [
        parse_output(run_cargo_bench(run))
        for run in range(1, ARGS.runs + 1)
    ]
    medians, samples = aggregate_runs(run_results)
    record = make_record(medians, samples)
    print_summary(medians, samples)

    if ARGS.save_json:
        ARGS.save_json.parent.mkdir(parents=True, exist_ok=True)
        ARGS.save_json.write_text(
            json.dumps(record, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        print(f"\nSaved benchmark record to {ARGS.save_json}")

    regressed = False
    if baseline_record is not None:
        validate_comparison_environment(baseline_record, record)
        regressed = compare_results(record_results(baseline_record), medians)

    if ARGS.fail_on_regression and regressed:
        print("\nPerformance gate failed.", file=sys.stderr)
        raise SystemExit(2)


if __name__ == "__main__":
    try:
        main()
    except RuntimeError as error:
        print(f"benchmark error: {error}", file=sys.stderr)
        raise SystemExit(2) from None
