#!/usr/bin/env python3
"""Capture a finite production packing workload with validated per-template windows.

This runner uses the benchmark process, frozen-source, and clock helpers. It
intentionally has a separate result contract from receive-to-accepted throughput.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import statistics
from pathlib import Path

from cross_version_benchmark import binary_record, git_record, host_identity, sha256
from measurement_process import run_process
from measurement_window import validate_measurement_window, wall_alignment

CONTRACT = "template_selection_v2"
ADAPTERS = {"authority_selection_v1", "develop_tx_selector_v1"}
HEX = frozenset("0123456789abcdef")


def require(condition, message):
    if not condition:
        raise ValueError(message)


def integer(value, name, *, positive=False):
    require(type(value) is int and value >= int(positive), f"invalid {name}")
    return value


def digest(value):
    require(isinstance(value, str) and len(value) == 64 and set(value) <= HEX, "invalid corpus/result digest")


def explicit_limits(value):
    fields = value.split(":")
    require(len(fields) == 3 and fields[0] == "budget", "invalid explicit transaction budget")
    require(all(field.isascii() and field.isdecimal() for field in fields[1:]), "invalid explicit budget integer")
    limits = tuple(int(field) for field in fields[1:])
    require(all(number <= 2**64 - 1 for number in limits), "explicit budget overflows")
    return limits


def parse(output: str, expected: dict, minimum_selection_ns: int = 1_000_000,
          observation_mode: str = "timing") -> dict:
    integer(minimum_selection_ns, "minimum_selection_ns", positive=True)
    require(observation_mode in ("timing", "allocation"), "unknown observation mode")
    allocation_enabled = observation_mode == "allocation"

    def allocation(record):
        if not allocation_enabled:
            require(record is None, "timing run contains allocation observations")
            return
        require(isinstance(record, dict) and set(record) == {"calls", "requested_bytes"},
                "missing or unsupported allocation observation")
        for name, value in record.items():
            integer(value, name)
    records = []
    for line in output.splitlines():
        if line.startswith("PACKING_"):
            prefix, body = line.split(" ", 1)
            record = json.loads(body)
            require(isinstance(record, dict) and type(record.get("schema_version")) is int
                    and record["schema_version"] == 1, "unsupported packing record schema")
            records.append((prefix, record))
    repeats = expected["repeats"]
    require([prefix for prefix, _ in records] == ["PACKING_BUILD", "PACKING_CORPUS"]
            + ["PACKING_SAMPLE"] * repeats + ["PACKING_COMPLETE"], "incomplete, duplicate, or reordered packing records")
    build, corpus = records[0][1], records[1][1]
    require(set(build) == {"schema_version", "contract", "adapter", "debug_assertions", "packing_bench",
                           "profiling", "allocation_observation", "tokio_trace"}, "unsupported packing build fields")
    require(build["contract"] == CONTRACT and build["adapter"] in ADAPTERS, "unsupported packing execution path")
    require(build["packing_bench"] is True and all(build[key] is False for key in
            ("debug_assertions", "profiling", "tokio_trace")), "requires a prod packing build without tracing")
    require(build["allocation_observation"] is allocation_enabled, "allocation feature differs from requested observation mode")
    require(set(corpus) == {"schema_version", "scenario", "shape", "fees", "count", "limit", "repeats", "warm",
        "fixture_digest", "fixture_ns", "setup_ns", "retained_source_entries", "causal_edges", "cell_dep_edges",
        "max_ancestors", "total_bytes", "total_cycles", "total_fees", "bytes_limit", "cycles_limit",
        "optimal_fee_small_fixture", "scope", "fixture_allocation", "setup_allocation", "reference_result"}, "unsupported packing corpus fields")
    allocation(corpus["fixture_allocation"])
    allocation(corpus["setup_allocation"])
    for name, value in expected.items():
        require(type(corpus.get(name)) is type(value) and corpus[name] == value, f"packing corpus differs: {name}")
    require(corpus["scenario"] == f"packing_{corpus['shape']}_{corpus['fees']}_{corpus['count']}_{corpus['limit']}", "scenario differs from corpus")
    digest(corpus["fixture_digest"])
    for key in ("fixture_ns", "setup_ns", "total_bytes", "total_cycles", "total_fees"):
        integer(corpus[key], key, positive=True)
    for key in ("causal_edges", "cell_dep_edges", "bytes_limit", "cycles_limit", "retained_source_entries", "max_ancestors"):
        integer(corpus[key], key)
    require(corpus["retained_source_entries"] == corpus["count"] and corpus["max_ancestors"] == 64, "source cardinality/ancestor policy differs")
    require(corpus["cell_dep_edges"] <= corpus["causal_edges"], "dependency edge counts differ")
    limits = {
        "all": (corpus["total_bytes"], corpus["total_cycles"]),
        "bytes": (corpus["total_bytes"] // 3, corpus["total_cycles"]),
        "cycles": (corpus["total_bytes"], corpus["total_cycles"] // 3),
        "both": (corpus["total_bytes"] * 2 // 3, corpus["total_cycles"] * 2 // 3),
        "zero": (0, 0),
    }
    requested = limits.get(corpus["limit"])
    if requested is None:
        requested = explicit_limits(corpus["limit"])
    require((corpus["bytes_limit"], corpus["cycles_limit"]) == requested, "capacity regime differs")
    optimum = corpus["optimal_fee_small_fixture"]
    require((optimum is not None) == (corpus["count"] <= 16), "small-fixture quality oracle missing or unexpected")
    if optimum is not None:
        integer(optimum, "optimal_fee_small_fixture")
        require(optimum <= corpus["total_fees"], "quality oracle exceeds available fee")
    def validate_result(result):
        require(isinstance(result, dict) and set(result) == {"selected_tx", "selected_bytes", "selected_cycles", "selected_fees",
            "ordered_digest", "set_digest", "byte_utilization", "cycle_utilization"}, "unsupported selection result")
        for key, upper in (("selected_tx", corpus["count"]), ("selected_bytes", min(corpus["bytes_limit"], corpus["total_bytes"])),
                           ("selected_cycles", min(corpus["cycles_limit"], corpus["total_cycles"])), ("selected_fees", corpus["total_fees"])):
            integer(result[key], key)
            require(result[key] <= upper, f"selection exceeds {key} bound")
        digest(result["ordered_digest"])
        digest(result["set_digest"])
        for label, actual, limit in (("byte_utilization", result["selected_bytes"], corpus["bytes_limit"]),
                                     ("cycle_utilization", result["selected_cycles"], corpus["cycles_limit"])):
            ratio = result[label]
            require(type(ratio) in (float, int) and math.isfinite(ratio)
                    and math.isclose(ratio, actual / limit if limit else 0, rel_tol=1e-12, abs_tol=1e-12), "capacity utilization differs")
        if corpus["limit"] == "all":
            require(result["selected_tx"] == corpus["count"] and result["selected_fees"] == corpus["total_fees"]
                    and result["selected_bytes"] == corpus["total_bytes"] and result["selected_cycles"] == corpus["total_cycles"], "full-capacity fixture lost entries")
        if corpus["limit"] == "zero":
            require(all(result[key] == 0 for key in ("selected_tx", "selected_bytes", "selected_cycles", "selected_fees")), "zero-capacity result is not empty")
        if optimum is not None:
            require(result["selected_fees"] <= optimum, "selected fee exceeds exhaustive optimum")
    reference = corpus["reference_result"]
    validate_result(reference)
    samples, elapsed = [], []
    for index, (_, sample) in enumerate(records[2:-1]):
        require(set(sample) == {"schema_version", "repeat", "window", "result", "ordered_deterministic", "set_deterministic", "allocation"}, "unsupported sample fields")
        allocation(sample["allocation"])
        require(type(sample["repeat"]) is int and sample["repeat"] == index, "sample sequence differs")
        window = validate_measurement_window(sample["window"])
        require(window["scenario"] == corpus["scenario"], "sample window scenario differs")
        # Separate windows may straddle a wall correction. Their independent
        # anchors qualify profile alignment; monotonic durations remain usable.
        result = sample["result"]
        validate_result(result)
        for kind in ("ordered", "set"):
            observed = sample[f"{kind}_deterministic"]
            require(type(observed) is bool and observed == (result[f"{kind}_digest"] == reference[f"{kind}_digest"]),
                    "determinism observation differs from preflight result")
        if build["adapter"] == "authority_selection_v1":
            require(result == reference, "current selection changed across identical calls")
        elapsed.append(window["elapsed_nanos"])
        samples.append(sample | {"wall_alignment": wall_alignment(window)})
    completion = records[-1][1]
    require(set(completion) == {"schema_version", "samples"} and type(completion["samples"]) is int
            and completion["samples"] == repeats, "packing completion differs")
    median = statistics.median(elapsed)
    qualification_failures = []
    if allocation_enabled:
        qualification_failures.append("allocation instrumentation is excluded from timing comparisons")
    if min(elapsed) < minimum_selection_ns:
        qualification_failures.append("a selection window is below the declared timing floor")
    if any(sample["result"]["selected_tx"] == 0 for sample in samples):
        qualification_failures.append("empty selection is a functional/rejection case")
    timing_qualified = not qualification_failures
    results = [sample["result"] for sample in samples]
    fields = ("selected_tx", "selected_bytes", "selected_cycles", "selected_fees", "byte_utilization", "cycle_utilization")
    result_range = {name: {"min": min(result[name] for result in results),
                           "median": statistics.median(result[name] for result in results),
                           "max": max(result[name] for result in results)} for name in fields}
    return {
        "build": build, "corpus": corpus, "samples": samples, "observation_mode": observation_mode,
        "summary": {"median_selection_ns": median, "median_templates_per_second": 1e9 / median if timing_qualified else None,
            "minimum_selection_ns": minimum_selection_ns,
            "timing_qualification": "allocation_only" if allocation_enabled else "eligible_for_comparison" if timing_qualified else "functional_only",
            "median_allocation": {name: statistics.median(sample["allocation"][name] for sample in samples)
                                  for name in ("calls", "requested_bytes")} if allocation_enabled else None,
            "allocation_scope": "process-wide alloc/realloc requested traffic; includes other threads, excludes retained bytes and RSS" if allocation_enabled else None,
            "qualification_failures": qualification_failures,
            "min_selection_ns": min(elapsed), "max_selection_ns": max(elapsed),
            "sum_selection_ns": sum(elapsed), "selection_windows": repeats,
            "setup_ns": corpus["setup_ns"], "fixture_ns": corpus["fixture_ns"],
            "fee_quality_ratio": result_range["selected_fees"]["median"] / optimum if optimum else None,
            "quality_scope": "exhaustive fee optimum only for count <= 16; no large-fixture optimality claim",
            "result": reference if all(result == reference for result in results) else None,
            "result_range": result_range,
            "distinct_selected_sets": len({result["set_digest"] for result in results}),
            "distinct_ordered_results": len({result["ordered_digest"] for result in results}),
            "all_match_preflight_set": all(sample["set_deterministic"] for sample in samples),
            "all_match_preflight_order": all(sample["ordered_deterministic"] for sample in samples),
            "sampling_scope": "within-process repeated fixed-source calls; not independent process replicates"},
    }


def source_identity(root: Path):
    # Formal captures require committed, clean roots. This freezes all tracked
    # production inputs; hash the executor and clock helper explicitly as well.
    return {"git": git_record(root), "harness": {
        name: sha256(root / "tx-pool" / name) for name in
        ("benches/packing_one_shot.rs", "benches/packing/mod.rs", "benches/measurement_clock/mod.rs",
         "benches/allocation_observation/mod.rs")}}


def replay(output: Path):
    receipt = json.loads((output / "receipt.json").read_text())
    require(receipt.get("schema_version") == 1 and receipt.get("contract") == CONTRACT
            and receipt.get("state") == "complete", "capture is incomplete or unsupported")
    require(set(receipt.get("artifacts", {})) == {"stdout.log", "stderr.log"}, "capture log membership differs")
    for name, identity in receipt["artifacts"].items():
        path = output / name
        require(path.stat().st_size == identity["size_bytes"] and sha256(path) == identity["sha256"], "capture artifact hash/size differs")
    observation = parse((output / "stdout.log").read_text(), receipt["expected"], receipt["minimum_selection_ns"], receipt["observation_mode"])
    require(observation == receipt["observation"], "saved packing observation differs from replay")
    return observation


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    subcommands = parser.add_subparsers(dest="mode", required=True)
    replay_parser = subcommands.add_parser("replay", help="verify a saved capture without its binary or checkout")
    replay_parser.add_argument("output", type=Path)
    capture = subcommands.add_parser("capture", help="run one fixed-source process with repeated selection windows")
    capture.add_argument("--root", type=Path, required=True)
    capture.add_argument("--binary", type=Path, required=True)
    capture.add_argument("--binary-profile", choices=("prod",), required=True,
                        help="attest this supplied binary was built from --root with --profile prod and the requested observation features")
    capture.add_argument("--observation", choices=("timing", "allocation"), default="timing",
                         help="allocation requires packing-bench,allocation-observation and never reports selection throughput")
    capture.add_argument("--output", type=Path, required=True, help="new directory outside the source checkout")
    capture.add_argument("--shape", choices=("independent", "chain", "fanout", "diamond", "mixed"), required=True)
    capture.add_argument("--fees", choices=("equal", "varied", "cpfp"), required=True)
    capture.add_argument("--count", type=int, required=True)
    capture.add_argument("--limit", required=True, help="all/bytes/cycles/both/zero or budget:BYTES:CYCLES for the remaining transaction budget")
    capture.add_argument("--repeats", type=int, default=5)
    capture.add_argument("--warm", type=int, default=2)
    capture.add_argument("--timeout-seconds", type=float, default=180)
    capture.add_argument("--min-selection-ns", type=int, default=1_000_000,
                         help="prospectively declared per-call floor; shorter/empty calls are functional-only")
    args = parser.parse_args()
    if args.mode == "replay":
        print(json.dumps(replay(args.output)["summary"], indent=2))
        return
    require(1 <= args.count <= 16_384 and 1 <= args.repeats <= 128 and 0 <= args.warm <= 32, "invalid population/repetition bound")
    if args.limit not in ("all", "bytes", "cycles", "both", "zero"):
        explicit_limits(args.limit)
    require(math.isfinite(args.timeout_seconds) and args.timeout_seconds > 0, "invalid timeout")
    integer(args.min_selection_ns, "min_selection_ns", positive=True)
    root, binary = args.root.resolve(strict=True), args.binary.resolve(strict=True)
    output = args.output.resolve()
    require(not output.is_relative_to(root), "capture artifacts must be outside the measured checkout")
    output.mkdir(parents=True, exist_ok=False)
    receipt_path = output / "receipt.json"
    expected = {name: getattr(args, name) for name in ("shape", "fees", "count", "limit", "repeats", "warm")}
    command = [str(binary)] + [str(expected[name]) for name in ("shape", "fees", "count", "limit", "repeats", "warm")]
    receipt = {"schema_version": 1, "contract": CONTRACT, "state": "preparing", "expected": expected,
               "command": command, "binary_profile_attestation": args.binary_profile,
               "minimum_selection_ns": args.min_selection_ns, "observation_mode": args.observation}
    try:
        receipt.update(source=source_identity(root), binary=binary_record(binary), host=host_identity())
        receipt["runner_sources"] = {name: sha256(Path(__file__).resolve().parent / name) for name in
                                     ("packing_benchmark.py", "measurement_process.py", "measurement_window.py", "cross_version_benchmark.py")}
        receipt["state"] = "running"
        receipt_path.write_text(json.dumps(receipt, indent=2) + "\n")
        env = {key: value for key, value in os.environ.items() if not key.startswith("TX_POOL_")}
        with (output / "stdout.log").open("w") as stdout, (output / "stderr.log").open("w") as stderr:
            completed = run_process(command, cwd=root, env=env, stdout=stdout, stderr=stderr,
                                    timeout=args.timeout_seconds)
        receipt["returncode"] = completed.returncode
        require(completed.returncode == 0, "packing executor failed; inspect retained logs")
        result = parse((output / "stdout.log").read_text(), expected, args.min_selection_ns, args.observation)
        require(source_identity(root) == receipt["source"] and binary_record(binary) == receipt["binary"], "source or binary changed during capture")
        receipt.update(state="complete", observation=result)
    except BaseException as error:
        receipt.update(state="failed", error=f"{type(error).__name__}: {error}")
        raise
    finally:
        receipt["artifacts"] = {name: {"sha256": sha256(output / name), "size_bytes": (output / name).stat().st_size}
                                for name in ("stdout.log", "stderr.log") if (output / name).exists()}
        receipt_path.write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps(receipt["observation"]["summary"], indent=2))


if __name__ == "__main__":
    main()
