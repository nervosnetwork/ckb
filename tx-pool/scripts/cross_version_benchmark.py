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
import shlex
import statistics
import subprocess
import time
from pathlib import Path

RESULT = re.compile(
    r"^BENCH_RESULT scenario=(?P<scenario>\S+) target=(?P<target>\d+) "
    r"warm=(?P<warm>\d+) workers=(?P<workers>\d+) peers=(?P<peers>\d+) "
    r"elapsed_ns=(?P<elapsed_ns>\d+) throughput_tps=(?P<throughput>[0-9.]+) "
    r"accepted=(?P<accepted>\d+)$",
    re.MULTILINE,
)
WINDOW = re.compile(
    r"^PROFILE_WINDOW start_unix_ns=(?P<start>\d+) end_unix_ns=(?P<end>\d+)$",
    re.MULTILINE,
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
            timeout=10,
        ).strip()
    except (OSError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
        return f"unavailable: {type(error).__name__}"


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
        or target + warm > 4_096
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
    command = [
        str(path),
        str(scenario["name"]),
        str(scenario["target"]),
        str(scenario["warm"]),
        str(scenario["workers"]),
        str(scenario["peers"]),
    ]
    started = time.time_ns()
    try:
        completed = subprocess.run(
            command,
            cwd=root,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            timeout=timeout,
            env=os.environ.copy(),
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
    ended = time.time_ns()
    if completed.returncode != 0:
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
    if len(results) != 1 or len(windows) != 1:
        return failure_attempt(
            side=side,
            phase=phase,
            command=command,
            started=started,
            ended=ended,
            category="invalid_evidence",
            detail=f"observed {len(results)} results and {len(windows)} windows",
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
    window = windows[0].groupdict()
    elapsed_ns = int(parsed["elapsed_ns"])
    evidence_error = None
    if observed != scenario:
        evidence_error = f"scenario drift: {observed} != {scenario}"
    elif accepted != expected_accepted:
        evidence_error = f"accepted {accepted}, expected {expected_accepted}"
    elif int(window["end"]) - int(window["start"]) < elapsed_ns:
        evidence_error = "target window is shorter than elapsed result"
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
    return {
        "outcome": "success",
        "side": side,
        "phase": phase,
        "command": command,
        "process_started_unix_ns": started,
        "process_ended_unix_ns": ended,
        "target_started_unix_ns": int(window["start"]),
        "target_ended_unix_ns": int(window["end"]),
        "elapsed_ns": elapsed_ns,
        "throughput_tps": float(parsed["throughput"]),
        "accepted": accepted,
        "output": completed.stdout,
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
        "schema": 2,
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
        baseline_rates: list[float] = []
        candidate_rates: list[float] = []
        measurement_failed = False
        for run_index in range(args.runs):
            order = [
                ("baseline", baseline, baseline_root),
                ("candidate", candidate, candidate_root),
            ]
            if run_index % 2:
                order.reverse()
            pair: dict[str, dict[str, object]] = {}
            for side, binary, root in order:
                phase = f"{key}-pair-{run_index + 1}"
                print(f">>> {phase}: {side}", flush=True)
                attempt = run_attempt(binary, root, scenario, side, phase, args.timeout_seconds)
                checkpoint(attempt)
                cool(args.cooldown_seconds)
                if attempt["outcome"] == "failure":
                    summaries[key] = {
                        "status": "non_comparable",
                        "reason": "measurement_failure",
                        "failed_side": side,
                        "failed_pair": run_index + 1,
                        "failure_category": attempt["category"],
                    }
                    record["summary"] = summaries
                    write_checkpoint(args.output, record)
                    measurement_failed = True
                    break
                pair[side] = attempt
            if measurement_failed:
                break
            baseline_rate = float(pair["baseline"]["throughput_tps"])
            candidate_rate = float(pair["candidate"]["throughput_tps"])
            baseline_rates.append(baseline_rate)
            candidate_rates.append(candidate_rate)
            paired_ratios.append(candidate_rate / baseline_rate)
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
            "candidate_over_baseline_ratios": paired_ratios,
            "median_candidate_over_baseline": median_ratio,
            "median_delta_percent": (median_ratio - 1.0) * 100.0,
            "paired_ratio_relative_mad_percent": paired_mad,
            "baseline_throughput_spread_percent": spread(baseline_rates),
            "candidate_throughput_spread_percent": spread(candidate_rates),
            "baseline_median_tps": statistics.median(baseline_rates),
            "candidate_median_tps": statistics.median(candidate_rates),
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
    main()
