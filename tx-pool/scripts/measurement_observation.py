"""Decode the finite executor's single successful workload observation.

Timing and profile tools share these workload facts, not their qualification
rules. The caller derives victim_notices from its validated execution adapter.
"""

import json
import math
import re

PREFIX = "TX_POOL_PROFILE_OBSERVATION "
SCHEMA_VERSION = 3
SCENARIO_FIELDS = ("scenario", "target", "warm", "workers", "peers")
INTEGER_FIELDS = {
    "schema_version", "target", "warm", "workers", "peers", "elapsed_nanos",
    "accepted", "callback_duplicates", "p99_latency_nanos", "target_cpu_nanos",
    "target_user_cpu_nanos", "target_system_cpu_nanos", "allocation_calls",
    "allocated_bytes", "reorg_latency_nanos", "reorg_overlap_callbacks", "relay_ok",
    "relay_duplicate_ok", "relay_rejects", "relay_unknown_parents",
    "relay_generation_resets", "shutdown_latency_nanos",
}
FIELDS = INTEGER_FIELDS | {"scenario", "throughput_tps", "relay_unknown_parent_observations"}
HEX_32 = re.compile(r"[0-9a-f]{64}")


def parse_observation(output: str, expected: dict[str, object], *, victim_notices: bool) -> dict[str, object]:
    records = [line[len(PREFIX):] for line in output.splitlines() if line.startswith(PREFIX)]
    if len(records) != 1:
        raise ValueError(f"expected one workload observation, observed {len(records)}")
    observation = json.loads(records[0])
    if (not isinstance(observation, dict) or set(observation) != FIELDS
            or observation["schema_version"] != SCHEMA_VERSION):
        raise ValueError("workload observation schema is unsupported")
    if any(type(observation[name]) is not int or observation[name] < 0 for name in INTEGER_FIELDS):
        raise ValueError("workload observation has an invalid integer")
    if (not isinstance(observation["scenario"], str)
            or any(type(expected.get(name)) is not int for name in SCENARIO_FIELDS[1:])
            or {name: observation[name] for name in SCENARIO_FIELDS} != expected):
        raise ValueError("workload observation scenario differs from the request")
    throughput, elapsed = observation["throughput_tps"], observation["elapsed_nanos"]
    try:
        valid_throughput = type(throughput) in (int, float) and math.isfinite(throughput) and throughput > 0
    except OverflowError:
        valid_throughput = False
    if not valid_throughput or elapsed <= 0:
        raise ValueError("workload observation throughput or duration is invalid")
    # JSON retains the producer's full f64 precision; no decimal text rounding
    # allowance belongs to this contract.
    if not math.isclose(throughput, expected["target"] * 1_000_000_000 / elapsed, rel_tol=1e-12):
        raise ValueError("throughput differs from target count and elapsed time")
    if observation["target_user_cpu_nanos"] + observation["target_system_cpu_nanos"] != observation["target_cpu_nanos"]:
        raise ValueError("workload observation CPU components do not sum to total")

    name = expected["scenario"]
    accepted = expected["target"] + expected["warm"]
    if observation["accepted"] != accepted or observation["relay_ok"] != accepted:
        raise ValueError("workload observation did not complete the exact workload")
    if ((observation["callback_duplicates"] and name != "reorg_in_flight")
            or observation["relay_duplicate_ok"] or observation["relay_generation_resets"]):
        raise ValueError("workload observation contains duplicate or reset terminals")
    expected_rejects = expected["warm"] if victim_notices and name in {"rbf_pairs", "rbf_pairs_windowed"} else 0
    if observation["relay_rejects"] != expected_rejects:
        raise ValueError("workload observation contains an unexpected reject terminal set")
    if (observation["reorg_overlap_callbacks"] > 0) != (name == "reorg_in_flight"):
        raise ValueError("workload observation reorg overlap differs from its scenario")

    unknown = observation["relay_unknown_parent_observations"]
    if not isinstance(unknown, list):
        raise ValueError("workload observation unknown-parent evidence is invalid")
    identities = []
    count = 0
    for row in unknown:
        if not isinstance(row, dict) or set(row) != {"peer", "parents", "count"}:
            raise ValueError("workload observation unknown-parent evidence is invalid")
        peer, parents, occurrences = row["peer"], row["parents"], row["count"]
        if (type(peer) is not int or peer < 0 or type(occurrences) is not int or occurrences <= 0
                or not isinstance(parents, list) or not parents
                or any(not isinstance(parent, str) or HEX_32.fullmatch(parent) is None for parent in parents)
                or parents != sorted(set(parents))):
            raise ValueError("workload observation unknown-parent evidence is invalid")
        identities.append((peer, tuple(parents)))
        count += occurrences
    if identities != sorted(set(identities)):
        raise ValueError("workload observation unknown-parent multiset is not canonical")
    if count != observation["relay_unknown_parents"]:
        raise ValueError("workload observation unknown-parent count does not match evidence")
    if count and not name.endswith("_reverse"):
        raise ValueError("workload observation contains unknown-parent terminals")
    return observation
