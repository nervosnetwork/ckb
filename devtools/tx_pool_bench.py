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
        default=0.0,
        help=(
            "allowed per-scenario regression before failure; the architectural "
            "acceptance gate uses the strict default of 0"
        ),
    )
    args = parser.parse_args()
    if args.runs < 1:
        parser.error("--runs must be at least 1")
    if args.regression_threshold_percent < 0:
        parser.error("--regression-threshold-percent cannot be negative")
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
    print(
        f"\n>>> Run {run}/{ARGS.runs}: {' '.join(cmd)} ({matrix_mode()} matrix)",
        flush=True,
    )

    env = os.environ.copy()
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
            }
        )
    return {
        "schema": 1,
        "generated_at_utc": datetime.datetime.now(
            datetime.timezone.utc
        ).isoformat(),
        "mode": matrix_mode(),
        "runs": ARGS.runs,
        "git_commit": command_output(["git", "rev-parse", "HEAD"]),
        "rustc": command_output(["rustc", "--version", "--verbose"]),
        "platform": platform.platform(),
        "machine": platform.machine(),
        "python": platform.python_version(),
        "results": entries,
    }


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


def print_summary(results: Dict[ResultKey, Result]) -> None:
    print("\n# Tx-Pool Pipeline Benchmark\n")
    print("| scenario | median time | median throughput |")
    print("|---|---:|---:|")
    for key, value in sorted(results.items()):
        print(
            f"| {format_key(key)} | {value['time_ms']:.3f} ms | "
            f"{value['throughput']:.2f} tx/s |"
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
    aggregate_regressed = aggregate_delta < 0.0
    failed = failed or aggregate_regressed
    print(
        f"\nThroughput geometric mean: {aggregate_delta:+.2f}% "
        f"({'REGRESSION' if aggregate_regressed else 'pass'})"
    )
    return failed


def main() -> None:
    run_results = [
        parse_output(run_cargo_bench(run))
        for run in range(1, ARGS.runs + 1)
    ]
    medians, samples = aggregate_runs(run_results)
    record = make_record(medians, samples)
    print_summary(medians)

    if ARGS.save_json:
        ARGS.save_json.parent.mkdir(parents=True, exist_ok=True)
        ARGS.save_json.write_text(
            json.dumps(record, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        print(f"\nSaved benchmark record to {ARGS.save_json}")

    regressed = False
    if ARGS.compare_json:
        baseline_record = json.loads(ARGS.compare_json.read_text(encoding="utf-8"))
        if baseline_record.get("mode") != record["mode"]:
            raise RuntimeError(
                "baseline matrix mode differs: "
                f"{baseline_record.get('mode')} != {record['mode']}"
            )
        regressed = compare_results(record_results(baseline_record), medians)

    if ARGS.fail_on_regression and regressed:
        print("\nPerformance gate failed.", file=sys.stderr)
        raise SystemExit(2)


if __name__ == "__main__":
    main()
