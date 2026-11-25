"""Validate a monotonic target window and its independent wall-clock anchors.

Wall alignment belongs to profile attribution. A wall-clock correction or a
preempted anchor read does not invalidate elapsed time measured by Instant.
"""

import json

PREFIX = "TX_POOL_PROFILE_WINDOW "
SCHEMA_VERSION = 3
FIELDS = {
    "schema_version", "scenario", "start_unix_nanos", "end_unix_nanos",
    "elapsed_nanos", "start_clock_uncertainty_nanos", "end_clock_uncertainty_nanos",
    "observed_end_unix_nanos",
}


def validate_measurement_window(window: object) -> dict[str, object]:
    if not isinstance(window, dict) or set(window) != FIELDS or window["schema_version"] != SCHEMA_VERSION:
        raise ValueError("measurement window schema is unsupported")
    integers = FIELDS - {"scenario"}
    if any(type(window[name]) is not int or window[name] < 0 for name in integers):
        raise ValueError("measurement window contains an invalid integer")
    if not isinstance(window["scenario"], str) or not window["scenario"]:
        raise ValueError("measurement window scenario is empty")
    start, end, elapsed = (window[name] for name in ("start_unix_nanos", "end_unix_nanos", "elapsed_nanos"))
    if elapsed <= 0 or end <= start or end - start != elapsed:
        raise ValueError("measurement window differs from its monotonic duration")
    return window


def parse_measurement_window(output: str, scenario_name: str, elapsed_ns: int) -> dict[str, object]:
    records = [line[len(PREFIX):] for line in output.splitlines() if line.startswith(PREFIX)]
    if len(records) != 1:
        raise ValueError(f"expected one measurement window, observed {len(records)}")
    window = validate_measurement_window(json.loads(records[0]))
    if window["scenario"] != scenario_name or window["elapsed_nanos"] != elapsed_ns:
        raise ValueError("measurement window and benchmark observation differ")
    return window


def wall_alignment(window: dict[str, object]) -> dict[str, object]:
    validate_measurement_window(window)
    error = window["observed_end_unix_nanos"] - window["end_unix_nanos"]
    uncertainty = window["start_clock_uncertainty_nanos"] + window["end_clock_uncertainty_nanos"]
    tolerance = max(1_000_000, window["elapsed_nanos"] // 10_000)
    maximum_error = abs(error) + uncertainty
    return {
        "observed_end_error_nanos": error,
        "anchor_uncertainty_nanos": uncertainty,
        "maximum_alignment_error_nanos": maximum_error,
        "tolerance_nanos": tolerance,
        "profile_alignment_valid": maximum_error <= tolerance,
        "scope": "wall mapping for profile attribution; monotonic throughput is independent",
    }


def parse_readiness(output: str, scenario: str, target: int, elapsed_ns: int) -> dict[str, object] | None:
    """Qualify the public-query barrier cost included in reverse-cohort timing."""
    prefix = "BENCH_READINESS "
    records = [line[len(prefix):] for line in output.splitlines() if line.startswith(prefix)]
    if scenario != "fanout_ready_64_reverse":
        if records:
            raise ValueError("unexpected workload readiness observation")
        return None
    if len(records) != 1 or target <= 0 or target % 65:
        raise ValueError("reverse cohort requires one complete readiness observation")
    record = json.loads(records[0])
    fields = {"schema_version", "policy", "query_count", "completed_barriers", "elapsed_nanos"}
    if not isinstance(record, dict) or set(record) != fields:
        raise ValueError("readiness observation fields differ")
    if any(type(record[k]) is not int or record[k] < 0 for k in fields - {"policy"}):
        raise ValueError("readiness observation integer is invalid")
    if record["schema_version"] != 1 or record["policy"] != "public_orphan_size_0_then_64_yield_v1":
        raise ValueError("readiness policy is unsupported")
    if (record["completed_barriers"] != 2 * (target // 65)
            or record["query_count"] < record["completed_barriers"]
            or not 0 < record["elapsed_nanos"] <= elapsed_ns):
        raise ValueError("readiness observation did not complete the timed barriers")
    return record | {"target_wall_fraction": record["elapsed_nanos"] / elapsed_ns}
