"""Fail-closed packing result contract tests; native fixtures have Rust checks."""
import copy
import json
import tempfile
import unittest
from pathlib import Path

from packing_benchmark import explicit_limits, parse, replay, sha256


class PackingContractTests(unittest.TestCase):
    def setUp(self):
        self.expected = {"shape": "independent", "fees": "equal", "count": 8, "limit": "all", "repeats": 2, "warm": 1}
        scenario = "packing_independent_equal_8_all"
        self.build = {"schema_version": 1, "contract": "template_selection_v2", "adapter": "authority_selection_v1",
                      "debug_assertions": False, "packing_bench": True, "profiling": False,
                      "allocation_observation": False, "tokio_trace": False}
        self.corpus = self.expected | {
            "schema_version": 1, "scenario": scenario, "fixture_digest": "a" * 64,
            "fixture_ns": 100, "setup_ns": 50, "retained_source_entries": 8, "causal_edges": 0,
            "cell_dep_edges": 0, "max_ancestors": 64, "total_bytes": 800, "total_cycles": 240_000,
            "total_fees": 800_000, "bytes_limit": 800, "cycles_limit": 240_000,
            "optimal_fee_small_fixture": 800_000, "scope": "accepted transactions only",
            "fixture_allocation": None, "setup_allocation": None,
        }
        self.samples = [{
            "schema_version": 1, "repeat": index,
            "window": {"schema_version": 3, "scenario": scenario, "start_unix_nanos": 1_000_000 + 5_000 * index,
                       "end_unix_nanos": 1_001_000 + 5_000 * index, "elapsed_nanos": 1_000,
                       "start_clock_uncertainty_nanos": 10, "end_clock_uncertainty_nanos": 10,
                       "observed_end_unix_nanos": 1_001_000 + 5_000 * index},
            "result": {"selected_tx": 8, "selected_bytes": 800, "selected_cycles": 240_000,
                       "selected_fees": 800_000, "ordered_digest": "b" * 64, "set_digest": "c" * 64,
                       "byte_utilization": 1.0, "cycle_utilization": 1.0},
            "ordered_deterministic": True, "set_deterministic": True,
            "allocation": None,
        } for index in range(2)]
        self.corpus["reference_result"] = copy.deepcopy(self.samples[0]["result"])

    def output(self):
        return "\n".join(prefix + " " + json.dumps(record) for prefix, record in
            [("PACKING_BUILD", self.build), ("PACKING_CORPUS", self.corpus)]
            + [("PACKING_SAMPLE", sample) for sample in self.samples]
            + [("PACKING_COMPLETE", {"schema_version": 1, "samples": 2})])

    def test_valid_windows_use_templates_per_second_and_quality(self):
        result = parse(self.output(), self.expected, minimum_selection_ns=500)
        self.assertEqual(result["summary"]["median_templates_per_second"], 1_000_000)
        self.assertEqual(result["summary"]["fee_quality_ratio"], 1)
        self.assertNotIn("throughput_tps", result["summary"])

    def test_short_and_zero_selection_are_functional_only(self):
        result = parse(self.output(), self.expected)
        self.assertEqual(result["summary"]["timing_qualification"], "functional_only")
        self.assertIsNone(result["summary"]["median_templates_per_second"])
        self.expected["limit"] = self.corpus["limit"] = "zero"
        self.corpus["scenario"] = "packing_independent_equal_8_zero"
        self.corpus["bytes_limit"] = self.corpus["cycles_limit"] = self.corpus["optimal_fee_small_fixture"] = 0
        for sample in self.samples:
            sample["window"]["scenario"] = self.corpus["scenario"]
            for field in ("selected_tx", "selected_bytes", "selected_cycles", "selected_fees", "byte_utilization", "cycle_utilization"):
                sample["result"][field] = 0
        self.corpus["reference_result"] = copy.deepcopy(self.samples[0]["result"])
        result = parse(self.output(), self.expected, minimum_selection_ns=500)
        self.assertEqual(result["summary"]["timing_qualification"], "functional_only")
        self.assertIsNone(result["summary"]["median_templates_per_second"])

    def test_clock_adjustment_only_invalidates_profile_alignment(self):
        self.samples[1]["window"]["observed_end_unix_nanos"] += 5_000_000
        result = parse(self.output(), self.expected)
        self.assertFalse(result["samples"][1]["wall_alignment"]["profile_alignment_valid"])
        self.assertEqual(result["summary"]["median_selection_ns"], 1000)

    def test_missing_or_duplicate_records_are_rejected(self):
        original = self.output()
        for value in ("\n".join(original.splitlines()[:-1]), original + "\n" + original.splitlines()[2]):
            with self.subTest(value=value), self.assertRaises(ValueError):
                parse(value, self.expected)

    def test_instrumented_or_debug_build_is_rejected(self):
        for field in ("debug_assertions", "profiling", "allocation_observation", "tokio_trace"):
            self.build[field] = True
            with self.subTest(field=field), self.assertRaises(ValueError):
                parse(self.output(), self.expected)
            self.build[field] = False

    def test_fake_packing_strategy_and_unknown_adapter_are_rejected(self):
        self.corpus["fees"] = "arrival_time"
        with self.assertRaises(ValueError):
            parse(self.output(), self.expected)
        self.corpus["fees"] = "equal"
        self.build["adapter"] = "fixture_only_sort"
        with self.assertRaises(ValueError):
            parse(self.output(), self.expected)

    def test_allocation_requires_explicit_mode_and_complete_counters(self):
        with self.assertRaises(ValueError):
            parse(self.output(), self.expected, observation_mode="allocation")
        self.build["allocation_observation"] = True
        self.corpus["fixture_allocation"] = {"calls": 100, "requested_bytes": 8000}
        self.corpus["setup_allocation"] = {"calls": 50, "requested_bytes": 4000}
        for sample in self.samples:
            sample["allocation"] = {"calls": 10, "requested_bytes": 800}
        result = parse(self.output(), self.expected, minimum_selection_ns=500, observation_mode="allocation")
        self.assertEqual(result["summary"]["timing_qualification"], "allocation_only")
        self.assertIsNone(result["summary"]["median_templates_per_second"])
        self.assertEqual(result["summary"]["median_allocation"], {"calls": 10, "requested_bytes": 800})
        with self.assertRaises(ValueError):
            parse(self.output(), self.expected)
        for value in (None, {"calls": True, "requested_bytes": 800}, {"calls": 10, "requested_bytes": -1}):
            self.samples[0]["allocation"] = value
            with self.subTest(value=value), self.assertRaises(ValueError):
                parse(self.output(), self.expected, observation_mode="allocation")
    def test_boolean_integer_and_bad_digest_are_rejected(self):
        self.samples[0]["result"]["selected_tx"] = True
        with self.assertRaises(ValueError):
            parse(self.output(), self.expected)
        self.samples[0]["result"]["selected_tx"] = 8
        self.corpus["fixture_digest"] = "fake"
        with self.assertRaises(ValueError):
            parse(self.output(), self.expected)

    def test_capacity_fee_and_utilization_tampering_is_rejected(self):
        original = copy.deepcopy(self.samples)
        for field, value in (("selected_bytes", 801), ("selected_cycles", 240_001),
                             ("selected_fees", 800_001), ("byte_utilization", 0.5)):
            self.samples = copy.deepcopy(original)
            self.samples[0]["result"][field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                parse(self.output(), self.expected)

    def test_current_order_and_repeated_set_must_be_deterministic(self):
        self.samples[1]["ordered_deterministic"] = False
        with self.assertRaises(ValueError):
            parse(self.output(), self.expected)
        self.samples[1]["ordered_deterministic"] = True
        self.samples[1]["result"]["set_digest"] = "d" * 64
        with self.assertRaises(ValueError):
            parse(self.output(), self.expected)

    def test_legacy_sibling_order_is_observed_without_changing_its_path(self):
        self.build["adapter"] = "develop_tx_selector_v1"
        self.samples[1]["ordered_deterministic"] = False
        self.samples[1]["result"]["ordered_digest"] = "d" * 64
        result = parse(self.output(), self.expected)
        self.assertFalse(result["samples"][1]["ordered_deterministic"])

    def test_legacy_partial_selection_records_set_and_quality_variation(self):
        self.build["adapter"] = "develop_tx_selector_v1"
        self.expected["limit"] = self.corpus["limit"] = "both"
        self.corpus["scenario"] = "packing_independent_equal_8_both"
        self.corpus["bytes_limit"] = 800 * 2 // 3
        self.corpus["cycles_limit"] = 160_000
        self.corpus["optimal_fee_small_fixture"] = 500_000
        for index, sample in enumerate(self.samples):
            sample["window"]["scenario"] = self.corpus["scenario"]
            count = 4 - index
            sample["result"].update(selected_tx=count, selected_bytes=count * 100, selected_cycles=count * 30_000,
                                    selected_fees=count * 100_000, byte_utilization=count * 100 / self.corpus["bytes_limit"],
                                    cycle_utilization=count * 30_000 / self.corpus["cycles_limit"])
        self.corpus["reference_result"] = copy.deepcopy(self.samples[0]["result"])
        self.samples[1]["result"].update(set_digest="d" * 64, ordered_digest="e" * 64)
        self.samples[1].update(ordered_deterministic=False, set_deterministic=False)
        summary = parse(self.output(), self.expected, minimum_selection_ns=500)["summary"]
        self.assertIsNone(summary["result"])
        self.assertEqual(summary["distinct_selected_sets"], 2)
        self.assertEqual(summary["result_range"]["selected_fees"], {"min": 300_000, "median": 350_000, "max": 400_000})
        self.build["adapter"] = "authority_selection_v1"
        with self.assertRaises(ValueError):
            parse(self.output(), self.expected)
        self.build["adapter"] = "develop_tx_selector_v1"
        self.samples[1]["set_deterministic"] = True
        with self.assertRaises(ValueError):
            parse(self.output(), self.expected)

    def test_window_duration_and_sample_sequence_tampering_are_rejected(self):
        self.samples[0]["window"]["elapsed_nanos"] = 999
        with self.assertRaises(ValueError):
            parse(self.output(), self.expected)
        self.samples[0]["window"]["elapsed_nanos"] = 1000
        self.samples[1]["repeat"] = 0
        with self.assertRaises(ValueError):
            parse(self.output(), self.expected)

    def test_source_prepare_and_quality_contracts_are_required(self):
        self.corpus["retained_source_entries"] = 7
        with self.assertRaises(ValueError):
            parse(self.output(), self.expected)
        self.corpus["retained_source_entries"] = 8
        self.corpus["optimal_fee_small_fixture"] = None
        with self.assertRaises(ValueError):
            parse(self.output(), self.expected)

    def test_explicit_budget_is_bounded_and_not_a_whole_block_limit(self):
        self.assertEqual(explicit_limits("budget:595000:3500000000"), (595_000, 3_500_000_000))
        for value in ("budget:1", "budget:1:2:3", "budget:-1:2", "budget:1:18446744073709551616"):
            with self.subTest(value=value), self.assertRaises(ValueError):
                explicit_limits(value)

    def test_replay_rejects_tampered_logs_and_failed_capture(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            (directory / "stdout.log").write_text(self.output())
            (directory / "stderr.log").write_text("")
            receipt = {"schema_version": 1, "contract": "template_selection_v2", "state": "complete",
                       "observation_mode": "timing",
                       "expected": self.expected, "minimum_selection_ns": 1_000_000, "observation": parse(self.output(), self.expected),
                       "artifacts": {name: {"size_bytes": (directory / name).stat().st_size,
                                            "sha256": sha256(directory / name)} for name in ("stdout.log", "stderr.log")}}
            (directory / "receipt.json").write_text(json.dumps(receipt))
            self.assertEqual(replay(directory), receipt["observation"])
            (directory / "stdout.log").write_text(self.output() + "tampered")
            with self.assertRaises(ValueError):
                replay(directory)
            (directory / "stdout.log").write_text(self.output())
            receipt["state"] = "failed"
            (directory / "receipt.json").write_text(json.dumps(receipt))
            with self.assertRaises(ValueError):
                replay(directory)


if __name__ == "__main__":
    unittest.main()
