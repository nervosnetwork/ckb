"""The shared success record keeps strict workload facts for both frontends."""

import json
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import measurement_observation as observation


def completed_observation(*, scenario="always_success", target=8, warm=0, workers=1, peers=1,
                          elapsed=1_000_000_000, cpu=1_000_000_000, **changes):
    record = dict(schema_version=observation.SCHEMA_VERSION, scenario=scenario, target=target,
        warm=warm, workers=workers, peers=peers, elapsed_nanos=elapsed,
        throughput_tps=target * 1e9 / elapsed, accepted=target + warm, callback_duplicates=0,
        p99_latency_nanos=1, target_cpu_nanos=cpu, target_user_cpu_nanos=cpu,
        target_system_cpu_nanos=0, allocation_calls=0, allocated_bytes=0,
        reorg_latency_nanos=1, reorg_overlap_callbacks=0, relay_ok=target + warm,
        relay_duplicate_ok=0, relay_rejects=0, relay_unknown_parents=0,
        relay_unknown_parent_observations=[], relay_generation_resets=0, shutdown_latency_nanos=1)
    return record | changes


def record_output(record):
    return observation.PREFIX + json.dumps(record) + "\n"


class ObservationContractTests(unittest.TestCase):
    def setUp(self):
        self.record = completed_observation()
        self.expected = {name: self.record[name] for name in observation.SCENARIO_FIELDS}

    def parse(self, record, *, victim_notices=True, expected=None):
        return observation.parse_observation(record_output(record), expected or self.expected,
                                             victim_notices=victim_notices)

    def test_one_exact_record_and_supported_schema_are_required(self):
        self.assertEqual(self.parse(self.record), self.record)
        for output in ("", record_output(self.record) * 2, observation.PREFIX + "not JSON",
                       observation.PREFIX + "[]"):
            with self.subTest(output=output), self.assertRaises(ValueError):
                observation.parse_observation(output, self.expected, victim_notices=True)
        for change in (dict(schema_version=2), dict(schema_version=True), dict(extra=1)):
            with self.subTest(change=change), self.assertRaises(ValueError):
                self.parse(self.record | change)
        for field in self.record:
            with self.subTest(missing=field), self.assertRaises(ValueError):
                self.parse({key: value for key, value in self.record.items() if key != field})

    def test_identity_and_integer_types_are_exact(self):
        for field in observation.INTEGER_FIELDS:
            for value in (True, -1, "1", float(self.record[field])):
                with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                    self.parse(self.record | {field: value})
        for field, value in (("scenario", "other"), ("target", 9), ("warm", 1), ("workers", 2), ("peers", 2)):
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, "scenario differs"):
                self.parse(self.record | {field: value})
        with self.assertRaisesRegex(ValueError, "scenario differs"):
            self.parse(self.record, expected=self.expected | {"workers": True})

    def test_throughput_cpu_and_duration_are_cross_checked(self):
        for value in (True, "8", 0, -1, float("nan"), float("inf"), 10**400, 8.001):
            with self.subTest(value=value), self.assertRaises(ValueError):
                self.parse(self.record | {"throughput_tps": value})
        for change in (dict(elapsed_nanos=0), dict(elapsed_nanos=10**400), dict(target_system_cpu_nanos=1)):
            with self.subTest(change=change), self.assertRaises(ValueError):
                self.parse(self.record | change)
        # Full precision replaces the old three-decimal output tolerance.
        with self.assertRaisesRegex(ValueError, "throughput differs"):
            self.parse(self.record | {"throughput_tps": 8.0001})

    def test_terminal_loss_duplicates_and_reorg_have_distinct_scope(self):
        for field, value in (("accepted", 7), ("relay_ok", 7), ("callback_duplicates", 1),
                             ("relay_duplicate_ok", 1), ("relay_generation_resets", 1),
                             ("relay_rejects", 1), ("reorg_overlap_callbacks", 1)):
            with self.subTest(field=field), self.assertRaises(ValueError):
                self.parse(self.record | {field: value})
        reorg = self.record | dict(scenario="reorg_in_flight", callback_duplicates=2, reorg_overlap_callbacks=1)
        expected = self.expected | {"scenario": "reorg_in_flight"}
        self.assertEqual(self.parse(reorg, expected=expected), reorg)
        with self.assertRaisesRegex(ValueError, "overlap"):
            self.parse(reorg | {"reorg_overlap_callbacks": 0}, expected=expected)

    def test_both_rbf_workloads_require_the_validated_adapters_victim_semantics(self):
        for name in ("rbf_pairs", "rbf_pairs_windowed"):
            expected = self.expected | dict(scenario=name, warm=8)
            native = completed_observation(scenario=name, warm=8, relay_rejects=8)
            self.assertEqual(self.parse(native, expected=expected), native)
            legacy = native | {"relay_rejects": 0}
            self.assertEqual(self.parse(legacy, expected=expected, victim_notices=False), legacy)
            for record, enabled in ((legacy, True), (native, False)):
                with self.assertRaisesRegex(ValueError, "unexpected reject"):
                    self.parse(record, expected=expected, victim_notices=enabled)

    def test_reverse_multiset_is_validated_without_normalizing_bad_evidence(self):
        expected = self.expected | {"scenario": "dependent_forest_8_reverse"}
        rows = [{"peer": 1, "parents": ["00" * 32, "11" * 32], "count": 2},
                {"peer": 2, "parents": ["22" * 32], "count": 1}]
        reverse = self.record | dict(scenario=expected["scenario"], relay_unknown_parents=3,
                                     relay_unknown_parent_observations=rows)
        self.assertEqual(self.parse(reverse, expected=expected), reverse)
        bad_rows = [None, ["bad"], [rows[0] | {"extra": 1}],
            [rows[0] | {"peer": True}], [rows[0] | {"count": False}],
            [rows[0] | {"count": 0}], [rows[0] | {"parents": []}],
            [rows[0] | {"parents": [None]}], [rows[0] | {"parents": [["00" * 32]]}],
            [rows[0] | {"parents": ["nonempty but not a hash"]}],
            [rows[0] | {"parents": ["AA" * 32]}],
            [rows[0] | {"parents": ["11" * 32, "00" * 32]}],
            [rows[0] | {"parents": ["00" * 32, "00" * 32]}],
            list(reversed(rows)), [rows[0], rows[0]],
            [rows[0], rows[0] | {"count": 1}]]
        for malformed in bad_rows:
            with self.subTest(rows=malformed), self.assertRaises(ValueError):
                self.parse(reverse | {"relay_unknown_parent_observations": malformed}, expected=expected)
        with self.assertRaisesRegex(ValueError, "does not match"):
            self.parse(reverse | {"relay_unknown_parents": 2}, expected=expected)
        with self.assertRaisesRegex(ValueError, "unknown-parent terminals"):
            self.parse(reverse | {"scenario": "always_success"})


if __name__ == "__main__":
    unittest.main()
