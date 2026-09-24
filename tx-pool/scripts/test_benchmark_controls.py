"""A/B ranking depends on applicable controls reconstructed from raw evidence."""

import argparse
import copy
import io
import json
import tempfile
import unittest
from pathlib import Path
from unittest import mock

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
        self.context = dict(source={"commit": "candidate", "root": "/fixed/source"}, consensus={},
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
                     "measurement_window_sha256", "rejection_diagnostics_sha256", "scenario_parser_sha256"):
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
        for change in (dict(outcome="failure"), dict(output="invalid raw evidence"), dict(output=None),
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

    def resume(self, record, run_attempt=None):
        """Exercise the checkpoint path while keeping native measurement mocked."""
        with (
            tempfile.TemporaryDirectory() as temporary,
            mock.patch.object(benchmark, "run_attempt", side_effect=run_attempt) as run,
            mock.patch.object(benchmark, "cool") as cool,
            mock.patch.object(benchmark, "environment_snapshot", return_value={}),
            mock.patch("sys.stdout", new=io.StringIO()),
        ):
            path = Path(temporary) / "result.json"
            benchmark.run_scenario(record, benchmark.attempt_index(record), path,
                                   record["sides"], self.scenario, argparse.Namespace(**self.config))
            self.assertEqual(benchmark.read_checkpoint(path), record)
        return run, cool

    def assert_same_evidence(self, actual, expected):
        for field in ("status", "corpus", "metrics", "metric_quality", "aa_equivalence"):
            self.assertEqual(actual[field], expected[field], field)
        self.assertEqual(len(actual["paired_samples"]), len(expected["paired_samples"]))
        for actual_pair, expected_pair in zip(actual["paired_samples"], expected["paired_samples"]):
            for side in ("baseline", "candidate"):
                self.assertEqual(actual_pair[side], expected_pair[side])

    def test_resume_rebuilds_saved_fields_from_the_same_raw_evidence_as_aa(self):
        expected = self.summary()
        for attempt in self.control["attempts"]:
            attempt.update(corpus={"forged": "corpus"}, build={"forged": "build"},
                           window={}, readiness={"forged": "readiness"},
                           command=["frozen", attempt["id"]], started_unix_ns=123,
                           ended_unix_ns=456, environment_before={"cpu": "original"})
        metadata = [{key: attempt[key] for key in (
            "id", "command", "output", "started_unix_ns", "ended_unix_ns", "environment_before")}
            for attempt in self.control["attempts"]]
        with mock.patch.object(benchmark, "write_checkpoint", wraps=benchmark.write_checkpoint) as write:
            run, cool = self.resume(self.control)
        write.assert_called_once()  # One scenario summary, never one rewrite per cached sample.
        run.assert_not_called()
        cool.assert_not_called()
        self.assert_same_evidence(self.control["summary"][self.key], expected)
        for attempt, original in zip(self.control["attempts"], metadata):
            self.assertEqual({key: attempt[key] for key in original}, original)
            self.assertEqual(attempt["metrics"]["elapsed_ns"], 1_000_000_000)
            self.assertEqual(attempt["metrics"]["throughput_tps"], 8)
            self.assertEqual(attempt["corpus"], expected["corpus"])
            self.assertIsNone(attempt["readiness"])

    def test_unchanged_cached_outcomes_do_not_write_the_ledger(self):
        for outcome in ("success", "failure"):
            with self.subTest(outcome=outcome), tempfile.TemporaryDirectory() as temporary:
                record = copy.deepcopy(self.control)
                attempt = record["attempts"][0]
                attempt.update(outcome=outcome, category="runner_timeout")
                before = copy.deepcopy(record)
                path = Path(temporary) / "result.json"
                benchmark.write_checkpoint(path, record)
                with (
                    mock.patch.object(benchmark, "write_checkpoint", wraps=benchmark.write_checkpoint) as write,
                    mock.patch.object(benchmark, "run_attempt") as run,
                    mock.patch.object(benchmark, "cool") as cool,
                ):
                    result = benchmark.obtain_attempt(
                        record, benchmark.attempt_index(record), path, {}, self.scenario,
                        attempt["side"], attempt["id"], argparse.Namespace(**self.config))
                self.assertIs(result, attempt)
                write.assert_not_called()
                run.assert_not_called()
                cool.assert_not_called()
                self.assertEqual(benchmark.read_checkpoint(path), before)
                if outcome == "success":
                    self.assertEqual(attempt["metrics"]["throughput_tps"], 8)

    def test_new_cached_failures_are_saved_immediately_without_sampling(self):
        corpus = benchmark.parse_attempt(output(), None, self.scenario, "disabled")["corpus"]
        cases = (
            ({"outcome": "running"}, "interrupted_attempt"),
            ({"output": "invalid raw evidence"}, "invalid_evidence"),
            ({"output": output().replace("00" * 32, "44" * 32)}, "corpus_drift"),
        )
        for changes, category in cases:
            with self.subTest(category=category), tempfile.TemporaryDirectory() as temporary:
                record = copy.deepcopy(self.control)
                attempt = record["attempts"][0]
                attempt.update(changes)
                path = Path(temporary) / "result.json"
                with (
                    mock.patch.object(benchmark, "write_checkpoint", wraps=benchmark.write_checkpoint) as write,
                    mock.patch.object(benchmark, "run_attempt") as run,
                    mock.patch.object(benchmark, "cool") as cool,
                ):
                    benchmark.obtain_attempt(
                        record, benchmark.attempt_index(record), path, {}, self.scenario,
                        attempt["side"], attempt["id"], argparse.Namespace(**self.config), corpus)
                write.assert_called_once()
                run.assert_not_called()
                cool.assert_not_called()
                self.assertEqual(attempt["outcome"], "failure")
                self.assertEqual(attempt["category"], category)
                self.assertEqual(benchmark.read_checkpoint(path), record)

    def test_new_attempt_saves_running_then_result_before_cooldown(self):
        for outcome in ("success", "failure"):
            with self.subTest(outcome=outcome), tempfile.TemporaryDirectory() as temporary:
                path = Path(temporary) / "result.json"
                record = {"attempts": []}
                result = dict(outcome=outcome, output=output(), category="runner_timeout")
                if outcome == "success":
                    result.update(benchmark.parse_attempt(output(), None, self.scenario, "disabled"))

                def sample(*_):
                    self.assertEqual(benchmark.read_checkpoint(path)["attempts"][0]["outcome"], "running")
                    return result

                def cooldown(_):
                    self.assertEqual(benchmark.read_checkpoint(path), record)
                    self.assertEqual(record["attempts"][0]["outcome"], outcome)

                with (
                    mock.patch.object(benchmark, "write_checkpoint", wraps=benchmark.write_checkpoint) as write,
                    mock.patch.object(benchmark, "run_attempt", side_effect=sample) as run,
                    mock.patch.object(benchmark, "cool", side_effect=cooldown) as cool,
                    mock.patch.object(benchmark, "environment_snapshot", return_value={}),
                    mock.patch("sys.stdout", new=io.StringIO()),
                ):
                    benchmark.obtain_attempt(
                        record, {}, path, self.context, self.scenario, "candidate", "new/attempt",
                        argparse.Namespace(**self.config))
                self.assertEqual(write.call_count, 2)
                run.assert_called_once()
                cool.assert_called_once_with(self.config["cooldown_seconds"])

    def test_ab_resume_cannot_reverse_the_raw_result_with_plausible_saved_metrics(self):
        self.config["comparison"] = "ab"
        for attempt in self.control["attempts"]:
            candidate = attempt["side"] == "candidate"
            saved = output(elapsed=800_000_000) if candidate else output()
            attempt.update(benchmark.parse_attempt(saved, None, self.scenario, "disabled"))
            attempt["output"] = output(elapsed=1_200_000_000) if candidate else output()
        run, _ = self.resume(self.control)
        run.assert_not_called()
        summary = self.control["summary"][self.key]
        self.assertEqual(summary["status"], "comparable")
        throughput = summary["metrics"]["throughput_tps"]
        self.assertEqual(throughput["ratio_direction"], "lower")
        self.assertAlmostEqual(throughput["median_candidate_over_baseline"], 1 / 1.2)

    def test_resume_bad_raw_evidence_cannot_be_repaired_by_saved_metrics_or_rerun(self):
        original = copy.deepcopy(self.control)
        for raw in ("invalid raw evidence", None, output().replace("accepted=8", "accepted=7")):
            with self.subTest(raw=raw):
                record = copy.deepcopy(original)
                attempt = record["attempts"][2]
                attempt.update(benchmark.parse_attempt(output(), None, self.scenario, "disabled"))
                attempt["output"] = raw
                run, _ = self.resume(record)
                run.assert_not_called()
                self.assertEqual(record["summary"][self.key]["status"], "non_comparable")
                self.assertEqual(attempt["category"], "invalid_evidence")
                self.assertEqual(attempt["output"], raw)
                # Even restoring parseable output cannot turn an already
                # recorded failure into a fresh success on the next resume.
                attempt["output"] = output()
                run, _ = self.resume(record)
                run.assert_not_called()
                self.assertEqual(attempt["outcome"], "failure")

    def test_resume_uses_the_frozen_allocation_observation_mode(self):
        self.config.update(comparison="ab", allocation_observation="enabled")
        for attempt in self.control["attempts"]:
            attempt["output"] = output().replace("allocation_observation=false", "allocation_observation=true").replace(
                "allocation_calls=0 allocated_bytes=0", "allocation_calls=7 allocated_bytes=64")
        run, _ = self.resume(self.control)
        run.assert_not_called()
        self.assertEqual(self.control["summary"][self.key]["status"], "allocation_observation")
        for attempt in self.control["attempts"]:
            self.assertEqual(attempt["metrics"]["allocation_calls"], 7)
            self.assertEqual(attempt["metrics"]["allocated_bytes"], 64)
        self.config["allocation_observation"] = "disabled"
        run, _ = self.resume(self.control)
        run.assert_not_called()
        self.assertEqual(self.control["summary"][self.key]["reason"], "pilot_failure")
        self.assertEqual(self.control["attempts"][0]["category"], "invalid_evidence")

    def test_resume_checks_raw_corpus_against_the_pilot(self):
        original = copy.deepcopy(self.control)
        for index, reason in ((1, "pilot_corpus_mismatch"), (2, "measurement_failure")):
            with self.subTest(index=index):
                record = copy.deepcopy(original)
                attempt = record["attempts"][index]
                attempt["corpus"] = benchmark.parse_attempt(output(), None, self.scenario, "disabled")["corpus"]
                attempt["output"] = output().replace("00" * 32, "44" * 32)
                run, _ = self.resume(record)
                run.assert_not_called()
                self.assertEqual(record["summary"][self.key]["reason"], reason)
                if index == 2:
                    self.assertEqual(attempt["category"], "corpus_drift")
                self.assertEqual(benchmark.replay_aa_row(record, self.scenario)["status"], "non_comparable")

    def test_resume_retains_failed_and_interrupted_attempts_without_sampling(self):
        original = copy.deepcopy(self.control)
        for outcome, category in (("failure", "runner_timeout"), ("running", "interrupted_attempt")):
            with self.subTest(outcome=outcome):
                record = copy.deepcopy(original)
                attempt = record["attempts"][2]
                attempt.update(outcome=outcome, category="runner_timeout", detail="original timeout")
                run, cool = self.resume(record)
                run.assert_not_called()
                cool.assert_not_called()
                self.assertEqual(attempt["outcome"], "failure")
                self.assertEqual(attempt["category"], category)
                self.assertEqual(attempt["output"], output())
                if outcome == "failure":
                    self.assertEqual(attempt["detail"], "original timeout")
                self.assertEqual(record["summary"][self.key]["reason"], "measurement_failure")

    def test_resume_completes_a_prefix_while_aa_requires_all_scheduled_attempts(self):
        expected = self.summary()
        complete = copy.deepcopy(self.control["attempts"])
        self.control["attempts"] = self.control["attempts"][:3]
        self.assertEqual(self.summary()["status"], "non_comparable")

        def sample(binary, root, scenario, side, attempt_id, timeout, allocation):
            self.assertEqual(scenario, self.scenario)
            return dict(id=attempt_id, side=side, scenario=scenario, outcome="success", output=output(),
                        **benchmark.parse_attempt(output(), None, scenario, allocation))

        run, cool = self.resume(self.control, sample)
        self.assertEqual([call.args[4] for call in run.call_args_list], [item["id"] for item in complete[3:]])
        self.assertEqual(cool.call_count, len(complete) - 3)
        self.assertEqual([attempt["id"] for attempt in self.control["attempts"]],
                         [attempt["id"] for attempt in complete])
        self.assert_same_evidence(self.control["summary"][self.key], expected)
        self.assertEqual(self.summary(), expected)

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

    def test_early_exit_does_not_misreport_control_corpus_mismatch(self):
        control = self.summary()
        evidence = {side: {self.key: control} for side in ("baseline", "candidate")}
        summaries = [dict(status=status, ranking_permitted=False)
                     for status in ("short_target_window", "calibrated")]
        summaries += [benchmark.failure_summary(reason, []) for reason in
                      ("pilot_failure", "pilot_corpus_mismatch", "measurement_failure")]
        for summary in summaries:
            with self.subTest(summary=summary):
                original = copy.deepcopy(summary)
                benchmark.qualify_ranking(summary, evidence, self.scenario, "disabled")
                self.assertEqual(summary["status"], original["status"])
                self.assertEqual(summary.get("reason"), original.get("reason"))
                self.assertFalse(summary["production_ranking_permitted"])
                self.assertEqual(summary["aa_controls"], {})

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
