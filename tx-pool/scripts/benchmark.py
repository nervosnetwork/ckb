#!/usr/bin/env python3
"""Run, record, and compare the tx-pool Criterion benchmark.

Examples:
    # One quick smoke run
    python3 tx-pool/scripts/benchmark.py --quick

    # Record an inter-run median baseline
    python3 tx-pool/scripts/benchmark.py --runs 3 --save-json /tmp/tx-pool-baseline.json

    # Compare the current tree and fail on any measured regression
    python3 tx-pool/scripts/benchmark.py --runs 3 \
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
import tempfile
import time
from pathlib import Path
from typing import Dict, Iterable, List, Optional, Tuple

ResultKey = Tuple[int, int, bool, str, int]
Result = Dict[str, float]
WORKSPACE_ROOT = Path(__file__).resolve().parents[2]
REMAPPED_SOURCE_ROOT = "/ckb-txpool-bench-source"
# Cargo's fingerprint cache is not worktree-aware when callers force two
# checkouts through one CARGO_TARGET_DIR. Keep each checkout's benchmark binary
# isolated so a baseline executable can never be mistaken for the candidate.


def bench_target_dir(workspace_root: Path) -> Path:
    return workspace_root / "target" / "tx-pool-bench"


def harness_files(workspace_root: Path) -> Tuple[Path, Path]:
    return (
        workspace_root / "tx-pool" / "scripts" / "benchmark.py",
        workspace_root / "tx-pool" / "src" / "benchmark.rs",
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
        "--baseline-worktree",
        type=Path,
        help=(
            "run an interleaved A/B against this baseline worktree instead "
            "of consuming a pre-recorded JSON baseline"
        ),
    )
    parser.add_argument(
        "--save-baseline-json",
        type=Path,
        help="save the baseline side of --baseline-worktree paired execution",
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
            "maximum allowed throughput noise across repetitions (max-min "
            "spread for independent records; maximum deviation from the "
            "median ratio for paired records); a noisier record is invalid "
            "for --fail-on-regression (default: 4 for quick diagnostics, "
            "5 for medium/full)"
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
    parser.add_argument(
        "--cooldown-seconds",
        type=float,
        default=15.0,
        help=(
            "idle interval after compiling the fixed benchmark binaries "
            "and before measurement (default: 15)"
        ),
    )
    args = parser.parse_args()
    if args.baseline_worktree and args.compare_json:
        parser.error("--baseline-worktree and --compare-json are mutually exclusive")
    if args.save_baseline_json and not args.baseline_worktree:
        parser.error("--save-baseline-json requires --baseline-worktree")
    if args.fail_on_regression and not (args.compare_json or args.baseline_worktree):
        parser.error(
            "--fail-on-regression requires --compare-json or --baseline-worktree"
        )
    if args.fail_on_regression and args.baseline_worktree:
        if args.runs < 4 or args.runs % 2 != 0:
            parser.error(
                "strict paired worktree comparison requires an even --runs "
                "value of at least 4 so A/B execution order is balanced"
            )
    if args.regression_threshold_percent is None:
        args.regression_threshold_percent = 2.0 if args.quick else 0.0
    if args.max_run_spread_percent is None:
        args.max_run_spread_percent = 4.0 if args.quick else 5.0
    if args.runs < 1:
        parser.error("--runs must be at least 1")
    if args.regression_threshold_percent < 0:
        parser.error("--regression-threshold-percent cannot be negative")
    if args.max_run_spread_percent <= 0:
        parser.error("--max-run-spread-percent must be positive")
    if args.cooldown_seconds < 0:
        parser.error("--cooldown-seconds cannot be negative")
    return args


ARGS = parse_args()


def matrix_mode() -> str:
    if ARGS.quick:
        return "quick"
    if ARGS.full:
        return "full"
    return "medium"


def command_output(args: List[str], workspace_root: Path = WORKSPACE_ROOT) -> str:
    try:
        return subprocess.check_output(args, cwd=workspace_root, text=True).strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


def files_sha256(paths: Iterable[Path], workspace_root: Path) -> str:
    digest = hashlib.sha256()
    for path in paths:
        digest.update(str(path.relative_to(workspace_root)).encode("utf-8"))
        digest.update(b"\0")
        digest.update(path.read_bytes())
        digest.update(b"\0")
    return digest.hexdigest()


def benchmark_environment(
    workspace_root: Path,
    criterion_home: Optional[Path] = None,
    preflight: bool = False,
) -> Dict[str, str]:
    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = str(bench_target_dir(workspace_root))
    env["CARGO_INCREMENTAL"] = "0"
    remap = f"--remap-path-prefix={workspace_root}={REMAPPED_SOURCE_ROOT}"
    if encoded_flags := env.get("CARGO_ENCODED_RUSTFLAGS"):
        env["CARGO_ENCODED_RUSTFLAGS"] = f"{encoded_flags}\x1f{remap}"
    else:
        rustflags = env.get("RUSTFLAGS", "")
        env["RUSTFLAGS"] = f"{rustflags} {remap}".strip()
    env.pop("QUICK_BENCH", None)
    env.pop("FULL_BENCH", None)
    env.pop("TX_POOL_BENCH_PREFLIGHT", None)
    if ARGS.quick:
        env["QUICK_BENCH"] = "1"
    elif ARGS.full:
        env["FULL_BENCH"] = "1"
    if preflight:
        env["TX_POOL_BENCH_PREFLIGHT"] = "1"
    if criterion_home is not None:
        env["CRITERION_HOME"] = str(criterion_home)
    return env


def benchmark_build_fingerprint(workspace_root: Path) -> str:
    env = benchmark_environment(workspace_root)
    flags = env.get("CARGO_ENCODED_RUSTFLAGS", env.get("RUSTFLAGS", ""))
    normalized = flags.replace(str(workspace_root), "$WORKSPACE")
    return f"CARGO_INCREMENTAL=0;RUSTFLAGS={normalized}"


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def source_file_sha256(workspace_root: Path, relative: str) -> str:
    path = workspace_root / relative
    if not path.is_file():
        raise RuntimeError(f"benchmark fingerprint input is missing: {path}")
    return file_sha256(path)


def prepare_benchmark_binary(workspace_root: Path, label: str = "") -> Dict[str, str]:
    cmd = [
        "cargo",
        "bench",
        "-p",
        "ckb-tx-pool",
        "--features",
        "internal",
        "--bench",
        "pipeline",
        "--no-run",
        "--message-format=json-render-diagnostics",
    ]
    print(
        f"\n>>> compiling {label + ' ' if label else ''}benchmark once: "
        f"{' '.join(cmd)}",
        flush=True,
    )
    completed = subprocess.run(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=benchmark_environment(workspace_root),
        cwd=workspace_root,
    )
    executable = None
    rendered_diagnostics: List[str] = []
    for line in completed.stdout.splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        if message.get("reason") == "compiler-message":
            rendered = message.get("message", {}).get("rendered")
            if rendered:
                rendered_diagnostics.append(rendered)
        target = message.get("target", {})
        if (
            message.get("reason") == "compiler-artifact"
            and target.get("name") == "pipeline"
            and "bench" in target.get("kind", [])
            and message.get("executable")
        ):
            executable = Path(message["executable"]).resolve()
    if completed.returncode != 0:
        for diagnostic in rendered_diagnostics:
            print(diagnostic, file=sys.stderr, end="")
        if completed.stderr:
            print(completed.stderr, file=sys.stderr, end="")
        raise RuntimeError(f"failed to compile {label or 'candidate'} benchmark")
    if executable is None or not executable.is_file():
        raise RuntimeError(
            f"cargo did not report the {label or 'candidate'} pipeline benchmark binary"
        )
    prepared = {"path": str(executable), "sha256": file_sha256(executable)}
    print(
        f">>> fixed {label or 'candidate'} binary: {prepared['path']} "
        f"({prepared['sha256'][:12]})",
        flush=True,
    )
    return prepared


def cool_down_after_build() -> None:
    if ARGS.cooldown_seconds == 0:
        return
    print(
        f"\n>>> cooling down for {ARGS.cooldown_seconds:g}s before measurement",
        flush=True,
    )
    time.sleep(ARGS.cooldown_seconds)


def verify_prepared_binary(prepared: Dict[str, str], label: str = "") -> None:
    executable = Path(prepared["path"])
    if not executable.is_file() or file_sha256(executable) != prepared["sha256"]:
        raise RuntimeError(f"{label or 'candidate'} benchmark binary changed during A/B")


def run_benchmark_binary(
    run: int,
    workspace_root: Path,
    prepared: Dict[str, str],
    label: str = "",
    preflight: bool = False,
    benchmark_filter: Optional[str] = None,
) -> str:
    executable = Path(prepared["path"])
    if not executable.is_file():
        raise RuntimeError(f"{label or 'candidate'} benchmark binary disappeared")
    # The Python runner owns A/B comparison. Criterion's implicit `base`
    # directory would compare against a stale prior invocation and plot/report
    # generation would add CPU work between scenarios, so disable both.
    cmd = [
        str(executable),
        "--bench",
        "--noplot",
        "--discard-baseline",
        "--color",
        "never",
    ]
    effective_filter = (
        ARGS.benchmark_filter if benchmark_filter is None else benchmark_filter
    )
    if effective_filter:
        cmd.append(effective_filter)
    phase = "preflight" if preflight else f"run {run}/{ARGS.runs}"
    print(
        f"\n>>> {label + ' ' if label else ''}{phase}: "
        f"{' '.join(cmd)} ({matrix_mode()} matrix)",
        flush=True,
    )
    lines: List[str] = []
    # Criterion compares against an existing `base` directory even when asked
    # not to save the current run. An empty per-invocation home prevents stale
    # historical samples from leaking into this runner's paired A/B output.
    with tempfile.TemporaryDirectory(prefix="txpool-criterion-") as scratch:
        proc = subprocess.Popen(
            cmd,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            env=benchmark_environment(workspace_root, Path(scratch), preflight),
            cwd=workspace_root,
        )
        if proc.stdout is None:
            raise RuntimeError("benchmark process stdout pipe was not created")
        for line in proc.stdout:
            if not preflight:
                print(line, end="", flush=True)
            lines.append(line)
        proc.wait()
    if proc.returncode != 0:
        if preflight:
            print("".join(lines), file=sys.stderr, end="")
        raise RuntimeError(
            f"benchmark binary failed during {'preflight' if preflight else f'repetition {run}'}"
        )
    return "".join(lines)


def preflight_benchmark_binary(
    workspace_root: Path, prepared: Dict[str, str], label: str = ""
) -> None:
    run_benchmark_binary(0, workspace_root, prepared, label, preflight=True)


def list_benchmark_scenarios(
    workspace_root: Path, prepared: Dict[str, str], label: str
) -> List[str]:
    executable = Path(prepared["path"])
    cmd = [str(executable), "--list", "--bench"]
    if ARGS.benchmark_filter:
        cmd.append(ARGS.benchmark_filter)
    completed = subprocess.run(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        env=benchmark_environment(workspace_root),
        cwd=workspace_root,
    )
    if completed.returncode != 0:
        print(completed.stdout, file=sys.stderr, end="")
        raise RuntimeError(f"failed to list {label} benchmark scenarios")

    suffix = ": benchmark"
    scenarios = []
    for line in completed.stdout.splitlines():
        stripped = line.strip()
        if stripped.endswith(suffix):
            scenarios.append(stripped[: -len(suffix)])
    if not scenarios:
        raise RuntimeError(f"{label} benchmark filter selected no scenarios")
    if len(scenarios) != len(set(scenarios)):
        raise RuntimeError(f"{label} benchmark listed duplicate scenarios")
    unsupported = [scenario for scenario in scenarios if not NAME_RE.match(scenario)]
    if unsupported:
        raise RuntimeError(f"{label} benchmark listed unsupported scenarios: {unsupported}")
    return scenarios


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


def environment_metadata(
    workspace_root: Path = WORKSPACE_ROOT,
    prepared: Optional[Dict[str, str]] = None,
) -> Dict:
    metadata = {
        "git_commit": command_output(["git", "rev-parse", "HEAD"], workspace_root),
        "git_tracked_changes": command_output(
            ["git", "status", "--porcelain", "--untracked-files=no"],
            workspace_root,
        ),
        "rustc": command_output(["rustc", "--version", "--verbose"]),
        "cargo": command_output(["cargo", "--version"]),
        "platform": platform.platform(),
        "machine": platform.machine(),
        "python": platform.python_version(),
        "cpu_count": os.cpu_count(),
        # Compile-time CARGO_MANIFEST_DIR strings are not rewritten by
        # --remap-path-prefix. Equal byte lengths keep those unavoidable path
        # differences from shifting native code/data layout between revisions.
        "source_root_bytes": len(os.fsencode(workspace_root)),
        "cargo_lock_sha256": source_file_sha256(workspace_root, "Cargo.lock"),
        "workspace_manifest_sha256": source_file_sha256(workspace_root, "Cargo.toml"),
        "benchmark_harness_sha256": files_sha256(
            harness_files(workspace_root), workspace_root
        ),
        "benchmark_target_dir": str(bench_target_dir(workspace_root)),
        "benchmark_filter": ARGS.benchmark_filter,
        "cooldown_seconds": ARGS.cooldown_seconds,
        "benchmark_build_fingerprint": benchmark_build_fingerprint(workspace_root),
    }
    if prepared is not None:
        metadata["benchmark_binary"] = prepared["path"]
        metadata["benchmark_binary_sha256"] = prepared["sha256"]
    return metadata


def make_record(
    medians: Dict[ResultKey, Result],
    samples: Dict[ResultKey, Dict[str, List[float]]],
    workspace_root: Path = WORKSPACE_ROOT,
    prepared: Optional[Dict[str, str]] = None,
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
    record.update(environment_metadata(workspace_root, prepared))
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


def relative_median_deviation(values: List[float]) -> float:
    if not values:
        raise RuntimeError("benchmark record has no paired ratio samples")
    median = statistics.median(values)
    if median <= 0:
        raise RuntimeError("benchmark paired ratio median must be positive")
    return max(abs(value - median) for value in values) / median * 100.0


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


def validate_paired_stability(record: Dict) -> None:
    unstable = []
    paired_samples = record.get("paired_ratio_samples", {})
    if not paired_samples:
        raise RuntimeError("paired candidate record has no ratio samples")
    for scenario, samples in paired_samples.items():
        throughputs = [float(value) for value in samples.get("throughput", [])]
        if len(throughputs) != int(record.get("runs", 1)):
            raise RuntimeError(
                f"paired record has incomplete ratio samples for {scenario}: "
                f"{len(throughputs)} != {record.get('runs')}"
            )
        deviation = relative_median_deviation(throughputs)
        if deviation > ARGS.max_run_spread_percent:
            unstable.append(f"{scenario}={deviation:.2f}%")
    if unstable:
        raise RuntimeError(
            "paired benchmark ratios deviate too far from their median "
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

    baseline_paired = bool(baseline.get("paired_execution", False))
    current_paired = bool(current.get("paired_execution", False))
    if baseline_paired != current_paired:
        raise RuntimeError(
            "baseline and candidate must both use paired execution or both use "
            "independent records"
        )

    mismatches = []
    for field in (
        "rustc",
        "cargo",
        "platform",
        "machine",
        "python",
        "cpu_count",
        "source_root_bytes",
        "cargo_lock_sha256",
        "workspace_manifest_sha256",
        "benchmark_harness_sha256",
        "benchmark_build_fingerprint",
        "cooldown_seconds",
    ):
        if baseline.get(field) != current.get(field):
            mismatches.append(
                f"{field}: {baseline.get(field)!r} != {current.get(field)!r}"
            )
        elif baseline.get(field) in (None, "", "unknown"):
            mismatches.append(f"{field}: unavailable on both sides")
    if mismatches:
        raise RuntimeError(
            "benchmark environment differs; record a new same-host baseline: "
            + "; ".join(mismatches)
        )

    if ARGS.fail_on_regression:
        for label, record in (("baseline", baseline), ("candidate", current)):
            if record.get("git_commit") in (None, "", "unknown"):
                raise RuntimeError(f"{label} benchmark has no reproducible git commit")
            tracked_changes = record.get("git_tracked_changes", "unknown")
            if tracked_changes != "":
                raise RuntimeError(
                    f"{label} benchmark must come from a clean tracked tree; "
                    f"git status was {tracked_changes!r}"
                )
        baseline_runs = int(baseline.get("runs", 1))
        current_runs = int(current.get("runs", 1))
        minimum_runs = 4 if current_paired else 3
        if baseline_runs < minimum_runs or current_runs < minimum_runs:
            raise RuntimeError(
                f"--fail-on-regression requires at least {minimum_runs} complete runs "
                f"on both sides (baseline={baseline_runs}, current={current_runs})"
            )
        if baseline_runs != current_runs:
            raise RuntimeError(
                "--fail-on-regression requires symmetric repetition counts "
                f"(baseline={baseline_runs}, current={current_runs})"
            )
        if current_paired and current_runs % 2 != 0:
            raise RuntimeError(
                "strict paired comparison requires an even repetition count "
                "so baseline-first and candidate-first orders are balanced"
            )
        if current_paired:
            validate_paired_stability(current)
        else:
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
    baseline: Dict[ResultKey, Result],
    current: Dict[ResultKey, Result],
    paired_samples: Optional[Dict[ResultKey, Dict[str, List[float]]]] = None,
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
        if paired_samples is None:
            throughput_ratio = new["throughput"] / old["throughput"]
            latency_ratio = new["time_ms"] / old["time_ms"]
        else:
            throughput_ratio = statistics.median(
                paired_samples[key]["throughput"]
            )
            latency_ratio = statistics.median(paired_samples[key]["time_ms"])
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


def write_record(path: Path, record: Dict, label: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(record, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    print(f"\nSaved {label} benchmark record to {path}")


def run_paired_worktrees(baseline_root: Path) -> bool:
    baseline_root = baseline_root.expanduser().resolve()
    if baseline_root == WORKSPACE_ROOT:
        raise RuntimeError("baseline worktree must differ from the candidate worktree")
    if not (baseline_root / "Cargo.toml").is_file():
        raise RuntimeError(f"baseline worktree has no Cargo.toml: {baseline_root}")

    # Build both revisions before measurement, then run these immutable
    # executables for every repetition. Besides avoiding repeated Cargo work,
    # this keeps compilation heat outside either side of an A/B pair.
    baseline_binary = prepare_benchmark_binary(baseline_root, "baseline")
    candidate_binary = prepare_benchmark_binary(WORKSPACE_ROOT, "candidate")
    baseline_scenarios = list_benchmark_scenarios(
        baseline_root, baseline_binary, "baseline"
    )
    candidate_scenarios = list_benchmark_scenarios(
        WORKSPACE_ROOT, candidate_binary, "candidate"
    )
    if baseline_scenarios != candidate_scenarios:
        raise RuntimeError(
            "baseline and candidate benchmark scenario lists differ: "
            f"{baseline_scenarios!r} != {candidate_scenarios!r}"
        )
    # The recorded schedule starts with baseline. Warm the candidate first and
    # baseline second so both executable/code paths are resident without giving
    # the first measured side a uniquely cold VM path.
    preflight_benchmark_binary(WORKSPACE_ROOT, candidate_binary, "candidate")
    preflight_benchmark_binary(baseline_root, baseline_binary, "baseline")
    cool_down_after_build()

    baseline_runs: List[Dict[ResultKey, Result]] = []
    candidate_runs: List[Dict[ResultKey, Result]] = []
    for run in range(1, ARGS.runs + 1):
        baseline_run: Dict[ResultKey, Result] = {}
        candidate_run: Dict[ResultKey, Result] = {}
        scenarios = list(baseline_scenarios)
        if run % 2 == 0:
            scenarios.reverse()
        for scenario in scenarios:
            pair = [
                ("baseline", baseline_root, baseline_binary, baseline_run),
                ("candidate", WORKSPACE_ROOT, candidate_binary, candidate_run),
            ]
            # Keep both measurements of the exact scenario adjacent. Running
            # an entire matrix per side leaves corresponding samples roughly
            # one matrix apart and aliases host-frequency drift into the code
            # delta. Reverse side and scenario order on alternate repetitions
            # so neither revision or scenario owns a privileged time slot.
            if run % 2 == 0:
                pair.reverse()
            for label, root, prepared, destination in pair:
                result = parse_output(
                    run_benchmark_binary(
                        run,
                        root,
                        prepared,
                        label,
                        benchmark_filter=scenario,
                    )
                )
                if len(result) != 1:
                    raise RuntimeError(
                        f"exact benchmark scenario selected {len(result)} results: "
                        f"{scenario}"
                    )
                duplicate = destination.keys() & result.keys()
                if duplicate:
                    names = ", ".join(sorted(format_key(key) for key in duplicate))
                    raise RuntimeError(f"duplicate paired benchmark result: {names}")
                destination.update(result)
        baseline_runs.append(baseline_run)
        candidate_runs.append(candidate_run)
    verify_prepared_binary(baseline_binary, "baseline")
    verify_prepared_binary(candidate_binary, "candidate")

    baseline_medians, baseline_samples = aggregate_runs(baseline_runs)
    candidate_medians, candidate_samples = aggregate_runs(candidate_runs)
    baseline_record = make_record(
        baseline_medians, baseline_samples, baseline_root, baseline_binary
    )
    candidate_record = make_record(
        candidate_medians, candidate_samples, WORKSPACE_ROOT, candidate_binary
    )
    paired_samples: Dict[ResultKey, Dict[str, List[float]]] = {}
    for key in baseline_medians:
        paired_samples[key] = {
            "throughput": [
                candidate[key]["throughput"] / baseline[key]["throughput"]
                for baseline, candidate in zip(baseline_runs, candidate_runs)
            ],
            "time_ms": [
                candidate[key]["time_ms"] / baseline[key]["time_ms"]
                for baseline, candidate in zip(baseline_runs, candidate_runs)
            ],
        }
    baseline_record["paired_execution"] = True
    candidate_record["paired_execution"] = True
    baseline_record["paired_schedule"] = "scenario-adjacent-alternating-v1"
    candidate_record["paired_schedule"] = "scenario-adjacent-alternating-v1"
    candidate_record["paired_ratio_samples"] = {
        format_key(key): values for key, values in paired_samples.items()
    }

    print("\n## Interleaved baseline")
    print_summary(baseline_medians, baseline_samples)
    print("\n## Interleaved candidate")
    print_summary(candidate_medians, candidate_samples)

    if ARGS.save_baseline_json:
        write_record(ARGS.save_baseline_json, baseline_record, "baseline")
    if ARGS.save_json:
        write_record(ARGS.save_json, candidate_record, "candidate")

    validate_comparison_environment(baseline_record, candidate_record)
    return compare_results(baseline_medians, candidate_medians, paired_samples)


def main() -> None:
    if ARGS.baseline_worktree:
        regressed = run_paired_worktrees(ARGS.baseline_worktree)
        if ARGS.fail_on_regression and regressed:
            print("\nPerformance gate failed.", file=sys.stderr)
            raise SystemExit(2)
        return

    baseline_record = None
    if ARGS.compare_json:
        baseline_record = json.loads(ARGS.compare_json.read_text(encoding="utf-8"))
        # Reject an invalid release comparison before spending minutes or hours
        # running its candidate side.
        current_environment = {
            "mode": matrix_mode(),
            "runs": ARGS.runs,
            **environment_metadata(WORKSPACE_ROOT),
        }
        validate_comparison_environment(baseline_record, current_environment)

    candidate_binary = prepare_benchmark_binary(WORKSPACE_ROOT)
    preflight_benchmark_binary(WORKSPACE_ROOT, candidate_binary)
    cool_down_after_build()
    run_results = [
        parse_output(run_benchmark_binary(run, WORKSPACE_ROOT, candidate_binary))
        for run in range(1, ARGS.runs + 1)
    ]
    verify_prepared_binary(candidate_binary)
    medians, samples = aggregate_runs(run_results)
    record = make_record(medians, samples, prepared=candidate_binary)
    print_summary(medians, samples)

    if ARGS.save_json:
        write_record(ARGS.save_json, record, "candidate")

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
