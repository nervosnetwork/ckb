"""Validate fixed-corpus RPC/chain evidence and report observation bounds."""

from __future__ import annotations

import heapq
import json
import math


def require(condition, message):
    if not condition:
        raise ValueError(message)


def integer(value, name):
    require(type(value) is int and value >= 0, f"invalid {name}")
    return value


def hash_value(value):
    require(isinstance(value, str) and len(value) == 66 and value.startswith("0x")
            and set(value[2:]) <= set("0123456789abcdef"), "invalid chain/transaction hash")
    return value


def percentiles(values):
    ordered = sorted(values)
    return {name: ordered[max(0, math.ceil(len(ordered) * fraction) - 1)]
            for name, fraction in (("p50", .5), ("p95", .95), ("p99", .99), ("p999", .999), ("max", 1))}


def percentile_bounds(lower, upper):
    lower, upper = percentiles(lower), percentiles(upper)
    return {name: [value, upper[name]] for name, value in lower.items()}


def parse(output, metadata, hashes, *, load, minimum_window_ns=1_000_000_000):
    """A successful native process is necessary but does not establish this contract."""
    sends, blocks, results = [], [], []
    ready = 0
    phase = "sends"
    for line in output.splitlines():
        if line == "EXTERNAL_READY":
            ready += 1
            continue
        if not line.startswith("EXTERNAL_"):
            continue
        prefix, body = line.split(" ", 1)
        record = json.loads(body)
        require(isinstance(record, dict), "external record is not an object")
        if prefix == "EXTERNAL_SEND":
            require(phase == "sends", "submission records are reordered")
            sends.append(record)
        elif prefix == "EXTERNAL_BLOCK":
            require(phase != "complete", "block records follow completion")
            phase = "blocks"
            blocks.append(record)
        elif prefix == "EXTERNAL_RESULT":
            phase = "complete"
            results.append(record)
        else:
            raise ValueError("unknown external record")
    require(ready == 1 and len(results) == 1, "missing or duplicate readiness/completion")
    result = results[0]
    require(type(result.get("schema")) is int and result["schema"] == 2
            and result.get("outcome") == "success" and result.get("error") is None,
            "external execution failed")
    require(set(load) == {"concurrency", "poll_ms", "timeout_s", "arrival_rate"}, "expected load settings are incomplete")
    for name in ("concurrency", "poll_ms", "timeout_s"):
        value = load[name]
        require(type(result.get(name)) is int and result[name] == value, f"external load setting differs: {name}")
    rate = load["arrival_rate"]
    require(rate is None or type(rate) is int and 1 <= rate <= 65_536, "invalid expected arrival rate")
    require("arrival_rate" in result and type(result["arrival_rate"]) is type(rate)
            and result["arrival_rate"] == rate, "external arrival rate differs")
    require(result.get("corpus") == metadata, "external corpus identity differs")
    require(result.get("automatic_retries") == 0 and result.get("clock") == "process_monotonic"
            and result.get("confirmation") == "first_observed_canonical_block_with_last_negative_bound",
            "external retry/clock/confirmation contract differs")
    require(result.get("build") == {"profiling": False, "allocation_observation": False,
                                    "debug_assertions": False}, "external measurement requires an uninstrumented prod executor")
    count = integer(metadata.get("count"), "corpus count")
    require(count > 0 and len(hashes) == count and len(set(hashes)) == count, "corpus transaction membership differs")
    require(len(sends) == count, "submission count differs from corpus")
    elapsed = integer(result.get("elapsed_ns"), "run elapsed")
    by_hash = {}
    for index, (send, expected_hash) in enumerate(zip(sends, hashes)):
        require(type(send.get("index")) is int and send["index"] == index
                and type(send.get("attempt")) is int and send["attempt"] == 0
                and send.get("hash") == hash_value(expected_hash), "submission identity/attempt differs")
        started = integer(send.get("submitted_ns"), "submission timestamp")
        ended = integer(send.get("rpc_completed_ns"), "RPC completion")
        require(started <= ended <= elapsed, "RPC timestamps are outside the run")
        require("scheduled_ns" in send, "arrival observation is missing")
        scheduled = send["scheduled_ns"]
        if rate is None:
            require(scheduled is None, "closed-loop submission has an arrival schedule")
        else:
            require(integer(scheduled, "scheduled arrival") == index * 1_000_000_000 // rate
                    and scheduled <= started, "submission differs from its fixed arrival schedule")
        response = send.get("response")
        require(isinstance(response, dict) and set(response) == {"jsonrpc", "id", "result"}
                and response["jsonrpc"] == "2.0" and response["id"] == 1
                and response["result"] == expected_hash, "RPC refusal, transport failure or returned hash mismatch")
        by_hash[expected_hash] = send
    active, peak = [], 0
    for send in sorted(sends, key=lambda row: row["submitted_ns"]):
        while active and active[0] <= send["submitted_ns"]:
            heapq.heappop(active)
        heapq.heappush(active, send["rpc_completed_ns"])
        peak = max(peak, len(active))
    require(peak <= load["concurrency"], "observed RPC concurrency exceeds its bound")
    previous_hash = hash_value(metadata["tip_hash"])
    previous_number = integer(metadata["tip_number"], "seed height")
    previous_fetch_end = 0
    observed = {}
    for block in blocks:
        require(type(block.get("number")) is int and block["number"] == previous_number + 1
                and block.get("parent_hash") == previous_hash, "observed chain is discontinuous")
        previous_number = block["number"]
        previous_hash = hash_value(block.get("hash"))
        lower = integer(block.get("last_negative_tip_start_ns"), "last negative observation")
        poll_start = integer(block.get("tip_rpc_start_ns"), "tip RPC start")
        poll_end = integer(block.get("tip_rpc_end_ns"), "tip RPC end")
        fetch_start = integer(block.get("block_rpc_start_ns"), "block RPC start")
        upper = integer(block.get("first_positive_block_end_ns"), "first positive observation")
        require(lower <= poll_start <= poll_end <= fetch_start <= upper <= elapsed
                and previous_fetch_end <= fetch_start, "chain observation timestamps are inconsistent")
        previous_fetch_end = upper
        require(isinstance(block.get("transactions"), list), "block transaction list missing")
        for transaction in block["transactions"]:
            require(transaction in by_hash and transaction not in observed, "unexpected or duplicate confirmation")
            submitted = by_hash[transaction]["submitted_ns"]
            require(submitted <= upper, "transaction was observed before submission")
            observed[transaction] = (max(lower, submitted), upper)
    require(set(observed) == set(hashes), "exact canonical confirmation set differs")
    first_submit = min(row["submitted_ns"] for row in sends)
    last_rpc = max(row["rpc_completed_ns"] for row in sends)
    final_lower = max(bounds[0] for bounds in observed.values())
    final_upper = max(bounds[1] for bounds in observed.values())
    duration = final_upper - first_submit
    require(duration > 0 and last_rpc > first_submit, "zero-duration external window")
    lower_latencies = [observed[key][0] - by_hash[key]["submitted_ns"] for key in hashes]
    upper_latencies = [observed[key][1] - by_hash[key]["submitted_ns"] for key in hashes]
    widths = [upper - lower for lower, upper in observed.values()]
    observation = {
        "count": count, "unique_submitted": count, "unique_confirmed": count,
        "rejected": 0, "duplicate_submissions": 0, "duplicate_confirmations": 0, "retries": 0,
        "observed_confirmation_window_ns": duration,
        "observed_confirmation_tps": count * 1e9 / duration,
        "completion_duration_bounds_ns": [final_lower - first_submit, duration],
        "completion_observation_uncertainty_ns": final_upper - final_lower,
        "rpc_completion_tps": count * 1e9 / (last_rpc - first_submit),
        "rpc_latency_ns": percentiles([row["rpc_completed_ns"] - row["submitted_ns"] for row in sends]),
        "confirmation_latency_bounds_ns": percentile_bounds(lower_latencies, upper_latencies),
        "observation_interval_width_ns": percentiles(widths),
        "peak_rpc_concurrency": peak,
        "arrivals": None,
        "canonical_blocks_observed": len(blocks),
        "timing_qualification": "eligible_for_comparison" if duration >= minimum_window_ns else "functional_only",
        "minimum_window_ns": minimum_window_ns,
        "scope": "fixed independent signed transactions; submission to first observed canonical inclusion, with observation bounds",
    }
    if rate is not None:
        observation["arrivals"] = {
            "rate_tx_s": rate,
            "last_scheduled_ns": sends[-1]["scheduled_ns"],
            "submission_delay_ns": percentiles([row["submitted_ns"] - row["scheduled_ns"] for row in sends]),
            "arrival_to_rpc_ns": percentiles([row["rpc_completed_ns"] - row["scheduled_ns"] for row in sends]),
            "arrival_to_confirmation_bounds_ns": percentile_bounds(
                [observed[key][0] - by_hash[key]["scheduled_ns"] for key in hashes],
                [observed[key][1] - by_hash[key]["scheduled_ns"] for key in hashes]),
        }
    return observation
