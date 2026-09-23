"""A/B ranking depends on applicable controls reconstructed from raw evidence."""

import copy
import json
import tempfile
import unittest
from pathlib import Path

import cross_version_benchmark as benchmark
from benchmark_build import build_command


def output(elapsed=1_000_000_000, cpu=1_000_000_000, rss=1_000_000):
    corpus = dict(consensus_blake2b="00" * 32, cycles_blake2b="11" * 32,
                  transaction_bytes_blake2b="22" * 32, transaction_hashes_blake2b="33" * 32,
                  cycle_assignment_count=8, cycles_sum=80, script_preflight_count=1, transaction_count=8)
    terminals = dict(callback_duplicates=0, relay_duplicate_ok=0, relay_generation_resets=0,
                     relay_ok=8, relay_rejects=0, relay_unknown_parent_observations=[])
    window = dict(schema_version=3, scenario="always_success", start_unix_nanos=1_000_000_000,
                  end_unix_nanos=1_000_000_000 + elapsed, elapsed_nanos=elapsed,
                  start_clock_uncertainty_nanos=0, end_clock_uncertainty_nanos=0,
                  observed_end_unix_nanos=1_000_000_000 + elapsed)
    return ("BENCH_BUILD profiling=false allocation_observation=false "
            "callback_observer=preallocated_atomic_slots_sharded_completion adapter=bounded_remote_batch "
            "debug_assertions=false measurement_window=terminal_completion_v3\n"
            + "BENCH_CORPUS " + json.dumps(corpus) + "\n"
            + "BENCH_TERMINALS " + json.dumps(terminals) + "\n"
            + "TX_POOL_PROFILE_WINDOW " + json.dumps(window) + "\n"
            + "BENCH_REJECTION_CAPTURE " + json.dumps(dict(schema=1, logger="rejections_and_warnings_v1",
                records=0, service_records=0, write_failed=False)) + "\n"
            + f"BENCH_RESULT scenario=always_success target=8 warm=0 workers=1 peers=1 elapsed_ns={elapsed} "
            + f"throughput_tps={8e9 / elapsed:.6f} accepted=8 callback_duplicates=0 relay_ok=8 relay_duplicate_ok=0 "
            + f"relay_rejects=0 relay_unknown_parents=0 relay_generation_resets=0 p99_latency_ns=1 target_cpu_ns={cpu} "
            + "allocation_calls=0 allocated_bytes=0 reorg_latency_ns=1 reorg_overlap_callbacks=0 shutdown_latency_ns=1\n"
            + f"RESOURCE_RESULT max_rss_bytes={rss} voluntary_context_switches=0 involuntary_context_switches=0\n")


class ControlEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.scenario = benchmark.parse_scenario("always_success,8,0,1,1")
        self.key = benchmark.scenario_key(self.scenario)
        self.config = dict(comparison="aa", calibration_only=False, allocation_observation="disabled",
            scenarios=[self.scenario], runs=6, replicates_per_sample=1, order_seed=0,
            initial_cooldown_seconds=0, cooldown_seconds=0, max_paired_mad_percent=1.5,
            confidence_level=0.95, max_ratio_interval_width_percent=4, min_target_seconds=0.25,
            timeout_seconds=180, aa_equivalence_margin_percent=2)
        self.config["schedule"] = {self.key: benchmark.balanced_schedule(6, 1, 0, self.key)}
        self.context = dict(source={"commit": "candidate"}, consensus={},
            build=dict(bench="profile_one_shot", features="", profile="prod", toolchain={"rustc": "fixed"}),
            binary=dict(path="/fixed/candidate", sha256="candidate binary", size=123))
        self.context["build"].update(schema=1, kind="tx_pool_benchmark_build",
            source=copy.deepcopy(self.context["source"]), binary=copy.deepcopy(self.context["binary"]),
            command=build_command("profile_one_shot", ""),
            cargo_artifact=dict(reason="compiler-artifact", executable="/fixed/candidate",
                                target=dict(name="profile_one_shot", kind=["bench"])))
        self.context["build"]["toolchain"]["cargo"] = "fixed"
        self.control = dict(schema=benchmark.SCHEMA_VERSION, complete=True, configuration=self.config,
            host={"node": "test", "cpu_model": "CPU"}, metric_scopes=benchmark.METRIC_SCOPES,
            sides={side: copy.deepcopy(self.context) for side in ("baseline", "candidate")},
            summary={self.key: {"status": "forged summary must be ignored"}})
        for name in ("runner_sha256", "build_runner_sha256", "process_runner_sha256", "harness_sha256",
                     "measurement_window_sha256", "rejection_diagnostics_sha256"):
            self.control[name] = "same"
        attempts = [(side, f"{self.key}/pilot/{side}") for side in ("candidate", "baseline")]
        for pair, block in enumerate(self.config["schedule"][self.key], 1):
            for replicate, order in enumerate(block, 1):
                attempts += [(side, f"{self.key}/pair-{pair}/replicate-{replicate}/{side}") for side in order]
        self.control["attempts"] = [dict(id=identity, side=side, scenario=self.scenario, outcome="success",
            output=output(), metrics={"deliberately": "not authoritative"}) for side, identity in attempts]

    def summary(self):
        return benchmark.replay_aa_row(self.control, self.scenario)

    def test_replay_uses_raw_observations_and_retains_failed_controls(self):
        summary = self.summary()
        self.assertEqual(summary["status"], "aa_equivalent", summary)
        original = copy.deepcopy(self.control)
        for change in (dict(outcome="failure"), dict(output="invalid raw evidence"),
                       dict(side="wrong"), dict(scenario={})):
            self.control = copy.deepcopy(original)
            self.control["attempts"][2].update(change)
            self.assertEqual(self.summary()["status"], "non_comparable")
        self.control = copy.deepcopy(original)
        self.control["attempts"].pop()
        self.assertEqual(self.summary()["status"], "non_comparable")
        self.control = copy.deepcopy(original)
        self.control["attempts"][2:4] = reversed(self.control["attempts"][2:4])
        self.assertEqual(self.summary()["status"], "non_comparable")

    def test_two_controls_are_required_and_rss_does_not_erase_throughput(self):
        control = self.summary()
        summary = benchmark.summarize_pairs(control["paired_samples"], control["corpus"], "disabled", 1.5)
        benchmark.qualify_ranking(summary, {}, self.scenario, "disabled")
        self.assertEqual(summary["status"], "comparable")
        self.assertFalse(summary["production_ranking_permitted"])
        evidence = {"candidate": {self.key: control}}
        benchmark.qualify_ranking(summary, evidence, self.scenario, "disabled")
        self.assertFalse(summary["production_ranking_permitted"])
        evidence["baseline"] = {self.key: copy.deepcopy(control)}
        benchmark.qualify_ranking(summary, evidence, self.scenario, "disabled")
        self.assertTrue(summary["production_ranking_permitted"])
        evidence["baseline"][self.key]["metric_quality"]["mean_peak_rss_bytes"]["aa_disposition"] = "unresolved"
        benchmark.qualify_ranking(summary, evidence, self.scenario, "disabled")
        self.assertFalse(summary["production_ranking_permitted"])
        self.assertTrue(summary["metric_quality"]["throughput_tps"]["ranking_permitted"])
        evidence["baseline"][self.key]["corpus"] = {"different": "corpus"}
        benchmark.qualify_ranking(summary, evidence, self.scenario, "disabled")
        self.assertEqual(summary["aa_controls"]["baseline"]["status"], "corpus_mismatch")
        self.assertFalse(summary["metric_quality"]["throughput_tps"]["ranking_permitted"])

    def test_favorable_summary_cannot_hide_raw_rss_or_corpus_failure(self):
        for attempt in self.control["attempts"]:
            if attempt["side"] == "candidate":
                attempt["output"] = output(rss=1_050_000)
        self.control["summary"][self.key] = {"status": "aa_equivalent", "production_ranking_permitted": True}
        summary = self.summary()
        self.assertEqual(summary["status"], "aa_equivalence_unresolved")
        self.assertEqual(summary["metric_quality"]["mean_peak_rss_bytes"]["aa_disposition"], "unresolved")
        self.assertEqual(summary["metric_quality"]["throughput_tps"]["aa_disposition"], "equivalent")
        self.control["attempts"][2]["output"] = output().replace("00" * 32, "44" * 32)
        self.assertEqual(self.summary()["status"], "non_comparable")

    def test_allocation_does_not_inherit_timing_ranking(self):
        control = self.summary()
        for sample in control["paired_samples"]:
            for arm in ("baseline", "candidate"):
                sample[arm]["metrics"].update(allocation_calls=10, allocated_bytes=100)
        summary = benchmark.summarize_pairs(control["paired_samples"], control["corpus"], "enabled", 1.5)
        benchmark.qualify_ranking(summary, {}, self.scenario, "enabled")
        self.assertFalse(summary["production_ranking_permitted"])
        for name, quality in summary["metric_quality"].items():
            self.assertEqual(quality["ranking_permitted"], name in ("allocation_calls", "allocated_bytes"))

    def test_control_is_bound_to_its_arm_not_the_other_production_version(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "aa.json"
            path.write_text(json.dumps(self.control))
            record = copy.deepcopy(self.control)
            record["configuration"].update(comparison="ab", aa_evidence={"baseline": None,
                "candidate": benchmark.binary_record(path)})
            record["sides"]["baseline"]["source"] = {"commit": "a different production version"}
            evidence = benchmark.load_aa_evidence(record)
            self.assertEqual(evidence["candidate"][self.key]["status"], "aa_equivalent")
            relocated = copy.deepcopy(self.control)
            for side, arm in relocated["sides"].items():
                arm["source"]["root"] = f"/another/{side}"
                arm["build"]["source"] = copy.deepcopy(arm["source"])
            path.write_text(json.dumps(relocated))
            record["configuration"]["aa_evidence"]["candidate"] = benchmark.binary_record(path)
            self.assertEqual(benchmark.load_aa_evidence(record)["candidate"][self.key]["status"], "aa_equivalent")
            original = copy.deepcopy(self.control)
            for field in ("source", "binary", "consensus", "build"):
                self.control = copy.deepcopy(original)
                self.control["sides"]["baseline"][field] = {"wrong": "identity"}
                path.write_text(json.dumps(self.control))
                record["configuration"]["aa_evidence"]["candidate"] = benchmark.binary_record(path)
                with self.subTest(field=field), self.assertRaises(RuntimeError):
                    benchmark.load_aa_evidence(record)
            for field, value in (("host", {}), ("complete", False), ("harness_sha256", "wrong")):
                self.control = original | {field: value}
                path.write_text(json.dumps(self.control))
                record["configuration"]["aa_evidence"]["candidate"] = benchmark.binary_record(path)
                with self.subTest(field=field), self.assertRaises(RuntimeError):
                    benchmark.load_aa_evidence(record)
            for field in ("source", "cargo_artifact"):
                self.control = copy.deepcopy(original)
                self.control["sides"]["baseline"]["build"][field] = {"wrong": "producer observation"}
                path.write_text(json.dumps(self.control))
                record["configuration"]["aa_evidence"]["candidate"] = benchmark.binary_record(path)
                with self.subTest(field=field), self.assertRaises(RuntimeError):
                    benchmark.load_aa_evidence(record)
            self.control = copy.deepcopy(original)
            self.control["configuration"]["confidence_level"] = 0.9
            path.write_text(json.dumps(self.control))
            record["configuration"]["aa_evidence"]["candidate"] = benchmark.binary_record(path)
            with self.assertRaisesRegex(RuntimeError, "setting differs"):
                benchmark.load_aa_evidence(record)
            path.write_text(json.dumps(original))
            with self.assertRaisesRegex(RuntimeError, "evidence changed"):
                benchmark.load_aa_evidence(record)


if __name__ == "__main__":
    unittest.main()
