#!/usr/bin/env python3
"""Capture and replay production-controller chain-transition measurements."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

from benchmark_build import binary_record, git_record, host_identity, load_build, sha256, validate_build
from cross_version_benchmark import RESOURCE_RESULT, harness_bundle
from measurement_process import run_process


def require(condition, message):
    if not condition:
        raise ValueError(message)


def hash_set(values):
    require(isinstance(values, list) and all(isinstance(value, str) and len(value) == 64
            and set(value) <= set("0123456789abcdef") for value in values), "invalid chain transaction hashes")
    require(len(set(values)) == len(values), "duplicate chain transaction hash")
    return set(values)


def parse(output, options, *, minimum_sample_ns=1_000_000):
    records = []
    for line in output.splitlines():
        if line.startswith("CHAIN_"):
            prefix, value = line.split(" ", 1)
            record = json.loads(value)
            require(isinstance(record, dict) and type(record.get("schema")) is int
                    and record["schema"] == 1, "unsupported chain observation schema")
            records.append((prefix, record))
    pressure = options[0] == "recovery-pressure"
    count, workers = int(options[1]), int(options[-1])
    expected_order = (["CHAIN_CORPUS", "CHAIN_SAMPLE", "CHAIN_RECOVERY", "CHAIN_COMPLETE"] if pressure else
                      ["CHAIN_CORPUS"] + ["CHAIN_SAMPLE"] * 4 + ["CHAIN_RECOVERY", "CHAIN_SAMPLE", "CHAIN_COMPLETE"])
    require([prefix for prefix, _ in records] == expected_order, "incomplete or reordered chain observation")
    corpus = records[0][1]
    require(corpus.get("options") == options and corpus.get("transactions") == count
            and corpus.get("workers") == workers and corpus.get("trusted_synthetic_blocks") is True
            and type(corpus.get("runtime_threads")) is int and corpus["runtime_threads"] > 0,
            "chain fixture differs from requested workload")
    require(all(corpus.get(field) is False for field in ("profiling", "allocation_observation", "debug_assertions")),
            "chain timing requires an uninstrumented prod executor")
    hash_set([corpus.get("fixture_blake2b")])
    hashes = hash_set(corpus.get("transaction_hashes"))
    require(len(hashes) == count, "chain corpus cardinality differs")
    if pressure:
        stages = {"detach_recovery_pressure_paused": (0, (count + 1) // 2, 0, count, 0)}
    else:
        roots = int(options[2])
        stages = {
            "pending_to_gap": (1, 0, 1, 0, 0), "gap_to_proposed": (2, 0, 1, 0, 0),
            "attach_committed_roots": (3, 0, 1, 0, roots),
            "detach_roots_with_descendants_paused": (2, 1, 0, roots, 0),
            "proposed_to_pending": (0, 2, 0, 0, 0),
        }
    samples = [record for prefix, record in records if prefix == "CHAIN_SAMPLE"]
    require([sample.get("stage") for sample in samples] == list(stages), "chain stages differ")
    for sample in samples:
        expected = stages[sample["stage"]]
        actual = tuple(sample.get(field) for field in ("tip_number", "detached_blocks", "attached_blocks",
                                                       "detached_transactions", "attached_transactions"))
        require(all(type(value) is int for value in actual) and actual == expected, "block delta differs from the measured stage")
        require(type(sample.get("elapsed_ns")) is int and sample["elapsed_ns"] > 0
                and type(sample.get("cpu_ns")) is int and sample["cpu_ns"] >= 0,
                "invalid chain duration/CPU observation")
        require(sample.get("scope") == "controller_reconciliation_and_required_publication", "chain timing scope differs")
        hash_set([sample.get("tip_hash")])
    if not pressure:
        require(samples[1]["tip_hash"] == samples[3]["tip_hash"], "rollback did not restore the exact proposed tip")
    recovery = next(record for prefix, record in records if prefix == "CHAIN_RECOVERY")
    offered, retained, accepted, refused = (hash_set(recovery.get(field)) for field in ("offered", "retained", "accepted", "refused"))
    require(type(recovery.get("elapsed_ns")) is int and recovery["elapsed_ns"] > 0
            and recovery.get("scope") == "resume_to_observed_empty_recovery_queue", "recovery clock/scope differs")
    if pressure:
        require(recovery.get("stage") == "bounded_recovery_pressure" and offered == hashes
                and retained and retained < offered and accepted and accepted.isdisjoint(refused)
                and accepted | refused == retained, "bounded recovery terminals differ from retained input")
        require(type(recovery.get("serialized_bytes")) is int and type(recovery.get("serialized_limit")) is int
                and 0 < recovery["serialized_bytes"] <= recovery["serialized_limit"], "accepted recovery exceeded its byte bound")
    else:
        require(recovery.get("stage") == "recover_detached_roots_and_descendants"
                and offered == set(corpus["transaction_hashes"][:4 * roots]) and retained == offered
                and accepted == hashes and not refused, "roundtrip recovery lost an affected transaction")
    require(records[-1][1] == {"schema": 1, "validated": True}, "chain completion did not validate")
    resources = list(RESOURCE_RESULT.finditer(output))
    require(len(resources) == 1, "missing or duplicate whole-process resource observation")
    return {"corpus": corpus, "samples": samples, "recovery": recovery,
            "resources": {key: int(value) for key, value in resources[0].groupdict().items()},
            "minimum_sample_ns": minimum_sample_ns,
            "stage_qualification": {sample["stage"]: "eligible_for_comparison" if sample["elapsed_ns"] >= minimum_sample_ns
                                    else "functional_only" for sample in samples},
            "scope": "trusted synthetic block deltas and real cell-state changes; storage preparation and full-block verification excluded"}


def save(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def capture(root, build_path, supplied, output, options, timeout):
    require(not output.resolve().is_relative_to(root.resolve()), "capture artifacts must be outside the source checkout")
    output.mkdir(parents=True, exist_ok=False)
    receipt = {"schema": 1, "contract": "chain_transitions_v1", "state": "preparing", "options": options}
    save(output / "receipt.json", receipt)
    try:
        build = json.loads(build_path.read_text())
        binary, build = load_build(build_path, root, "profile_one_shot", build["features"], supplied)
        receipt.update({"binary": binary, "build": build, "harness": harness_bundle(root), "host": host_identity(),
                        "tools": {name: sha256(Path(__file__).with_name(name)) for name in
                                  ("chain_benchmark.py", "benchmark_build.py", "measurement_process.py", "cross_version_benchmark.py")}})
        command = [sys.executable, str(Path(__file__).with_name("cross_version_benchmark.py")),
                   "__measure_child__", binary["path"], "chain-transition", *options]
        receipt.update({"state": "running", "command": command})
        save(output / "receipt.json", receipt)
        with (output / "stdout.log").open("w") as log:
            completed = run_process(command, cwd=root, text=True, stdout=log, stderr=subprocess.STDOUT, timeout=timeout)
        require(completed.returncode == 0, "chain executor failed; preserve the original log")
        receipt["observation"] = parse((output / "stdout.log").read_text(), options)
        require(binary_record(Path(binary["path"])) == binary, "chain executable changed during capture")
        require(git_record(root) == build["source"] and harness_bundle(root) == receipt["harness"],
                "chain source changed during capture")
        for name, digest in receipt["tools"].items():
            require(sha256(Path(__file__).with_name(name)) == digest, "capture tool changed during measurement")
        receipt["state"] = "complete"
    except BaseException as error:
        receipt.update({"state": "failed", "error": f"{type(error).__name__}: {error}"})
        raise
    finally:
        if (output / "stdout.log").exists():
            receipt["stdout_sha256"] = sha256(output / "stdout.log")
        save(output / "receipt.json", receipt)
    return receipt["observation"]


def replay(output):
    receipt = json.loads((output / "receipt.json").read_text())
    require(receipt.get("schema") == 1 and receipt.get("contract") == "chain_transitions_v1"
            and receipt.get("state") == "complete", "chain capture is incomplete or unsupported")
    require(sha256(output / "stdout.log") == receipt["stdout_sha256"], "chain raw log changed")
    build = receipt["build"]
    validate_build(build, build["source"], "profile_one_shot", build["features"], receipt["binary"])
    observation = parse((output / "stdout.log").read_text(), receipt["options"])
    require(observation == receipt["observation"], "chain observation differs from raw evidence")
    return observation


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    recording = commands.add_parser("capture")
    recording.add_argument("--root", type=Path, required=True)
    recording.add_argument("--build-receipt", type=Path, required=True)
    recording.add_argument("--binary", type=Path)
    recording.add_argument("--mode", choices=("roundtrip", "recovery-pressure"), required=True)
    recording.add_argument("--count", type=int, required=True)
    recording.add_argument("--roots", type=int)
    recording.add_argument("--workers", type=int, default=8)
    recording.add_argument("--timeout", type=float, default=300)
    recording.add_argument("--output", type=Path, required=True)
    replaying = commands.add_parser("replay")
    replaying.add_argument("output", type=Path)
    args = parser.parse_args()
    if args.command == "replay":
        result = replay(args.output.resolve())
    else:
        require((args.roots is not None) == (args.mode == "roundtrip"), "only roundtrip requires --roots")
        options = [args.mode, str(args.count)]
        if args.roots is not None:
            options.append(str(args.roots))
        options.append(str(args.workers))
        result = capture(args.root.resolve(), args.build_receipt.resolve(), args.binary,
                         args.output.resolve(), options, args.timeout)
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
