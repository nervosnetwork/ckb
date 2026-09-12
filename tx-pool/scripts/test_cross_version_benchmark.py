#!/usr/bin/env python3
"""Core evidence and resume canaries for ``cross_version_benchmark.py``."""

from __future__ import annotations

import importlib.util
import argparse
import io
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

SCRIPT = Path(__file__).resolve().with_name("cross_version_benchmark.py")
sys.path.insert(0, str(SCRIPT.parent))
SPEC = importlib.util.spec_from_file_location("txpool_cross_version_benchmark", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("cannot import cross_version_benchmark.py")
BENCHMARK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BENCHMARK)


EMPTY_CAPTURE = "BENCH_REJECTION_CAPTURE " + json.dumps(dict(
    schema=1, logger="rejections_and_warnings_v1", records=0, service_records=0, write_failed=False)) + "\n"


def window_record(scenario, elapsed, observed_offset=0):
    return "TX_POOL_PROFILE_WINDOW " + json.dumps(dict(
        schema_version=3, scenario=scenario, start_unix_nanos=1_000_000_000,
        end_unix_nanos=1_000_000_000 + elapsed, elapsed_nanos=elapsed,
        start_clock_uncertainty_nanos=0, end_clock_uncertainty_nanos=0,
        observed_end_unix_nanos=1_000_000_000 + elapsed + observed_offset)) + "\n"


class BuildProfileContractTest(unittest.TestCase):
    def test_readiness_requires_exact_policy_and_complete_timed_barriers(self) -> None:
        record = dict(schema_version=1, policy="public_orphan_size_0_then_64_yield_v1",
                      query_count=7, completed_barriers=4, elapsed_nanos=100)
        def output(value):
            return "BENCH_READINESS " + json.dumps(value) + "\n"
        parsed = BENCHMARK.parse_readiness(output(record), "fanout_ready_64_reverse", 130, 1000)
        self.assertEqual(parsed["target_wall_fraction"], 0.1)
        for changes in (dict(completed_barriers=3), dict(query_count=3), dict(elapsed_nanos=1001),
                        dict(elapsed_nanos=0), dict(query_count=True), dict(policy="ungated")):
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                BENCHMARK.parse_readiness(output(record | changes), "fanout_ready_64_reverse", 130, 1000)
        for text in ("", output(record) * 2):
            with self.assertRaises(ValueError):
                BENCHMARK.parse_readiness(text, "fanout_ready_64_reverse", 130, 1000)
        with self.assertRaises(ValueError):
            BENCHMARK.parse_readiness(output(record), "always_success", 130, 1000)
        self.assertIsNone(BENCHMARK.parse_readiness("", "always_success", 130, 1000))

    def test_cargo_metadata_diagnostics_do_not_corrupt_dependency_identity(self) -> None:
        metadata = {
            "packages": [{"id": name, "name": name} for name in BENCHMARK.CONSENSUS_LOCK_PACKAGES],
            "resolve": {"nodes": [{"id": name, "features": []} for name in BENCHMARK.CONSENSUS_LOCK_PACKAGES]},
        }
        owned_process = BENCHMARK.run_process
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            (root / "Cargo.lock").write_text("\n".join(
                f'[[package]]\nname = "{name}"\nversion = "0.24.15"'
                for name in BENCHMARK.CONSENSUS_LOCK_PACKAGES
            ))

            def emit_metadata(command, **kwargs):
                self.assertEqual(command[:2], ["cargo", "metadata"])
                return owned_process([
                    sys.executable, "-c",
                    "import sys; print('Blocking waiting for Cargo lock', file=sys.stderr); "
                    f"print({json.dumps(metadata)!r})",
                ], **kwargs)

            with mock.patch.object(BENCHMARK, "run_process", side_effect=emit_metadata):
                identity = BENCHMARK.consensus_dependency_identity(root, "")
            self.assertEqual(identity["enabled_features"], {
                name: [] for name in BENCHMARK.CONSENSUS_LOCK_PACKAGES
            })
            for output, exit_code in (("not JSON", 0), (json.dumps(metadata), 7)):
                def reject_metadata(command, **kwargs):
                    return owned_process([
                        sys.executable, "-c", f"print({output!r}); raise SystemExit({exit_code})",
                    ], **kwargs)

                with mock.patch.object(BENCHMARK, "run_process", side_effect=reject_metadata):
                    with self.assertRaisesRegex(RuntimeError, "cannot bind CKB-VM Cargo features"):
                        BENCHMARK.consensus_dependency_identity(root, "")

    def test_real_process_pipeline_detects_known_metric_changes_and_rejects_drift(self) -> None:
        # An executable fixture checks the complete command/resource/parser path.
        # Its declared metrics are a tool control, never production performance.
        scenario = BENCHMARK.parse_scenario("always_success,8,0,1,1")
        corpus = dict(consensus_blake2b="00" * 32, cycles_blake2b="11" * 32,
                      transaction_bytes_blake2b="22" * 32, transaction_hashes_blake2b="33" * 32,
                      cycle_assignment_count=8, cycles_sum=80, script_preflight_count=1,
                      transaction_count=8)
        terminals = dict(callback_duplicates=0, relay_duplicate_ok=0, relay_generation_resets=0,
                         relay_ok=8, relay_rejects=0, relay_unknown_parent_observations=[])
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            attempts = {}
            for elapsed in (800_000_000, 1_000_000_000, 1_200_000_000):
                output = (
                    "BENCH_BUILD profiling=false allocation_observation=false "
                    "callback_observer=preallocated_atomic_slots_sharded_completion "
                    "adapter=bounded_remote_batch debug_assertions=false measurement_window=terminal_completion_v3\n"
                    f"{window_record('always_success', elapsed)}"
                    f"BENCH_CORPUS {json.dumps(corpus)}\nBENCH_TERMINALS {json.dumps(terminals)}\n{EMPTY_CAPTURE}"
                    "BENCH_RESULT scenario=always_success target=8 warm=0 workers=1 peers=1 "
                    f"elapsed_ns={elapsed} throughput_tps={8e9 / elapsed:.3f} accepted=8 callback_duplicates=0 "
                    "relay_ok=8 relay_duplicate_ok=0 relay_rejects=0 relay_unknown_parents=0 "
                    f"relay_generation_resets=0 p99_latency_ns=1 target_cpu_ns={elapsed} "
                    "allocation_calls=0 allocated_bytes=0 reorg_latency_ns=1 "
                    "reorg_overlap_callbacks=0 shutdown_latency_ns=1\n"
                )
                binary = root / f"fixture-{elapsed}"
                binary.write_text(f"#!{sys.executable}\nprint({output!r})\n")
                binary.chmod(0o700)
                attempt = BENCHMARK.run_attempt(BENCHMARK.binary_record(binary), root, scenario,
                                                "baseline", str(elapsed), 5, "disabled")
                self.assertEqual(attempt["outcome"], "success", attempt)
                attempts[elapsed] = attempt
            base = BENCHMARK.aggregate_side([attempts[1_000_000_000]], 8)
            for elapsed, direction in ((800_000_000, "higher"), (1_200_000_000, "lower")):
                candidate = BENCHMARK.aggregate_side([attempts[elapsed]], 8)
                summary = BENCHMARK.summarize_pairs([dict(baseline=base, candidate=candidate)] * 10, corpus, "disabled", 1.5)
                self.assertEqual(summary["metrics"]["throughput_tps"]["ratio_direction"], direction)
                BENCHMARK.classify_aa_equivalence(summary, 2)
                self.assertEqual(summary["status"], "aa_equivalence_unresolved")
                self.assertFalse(summary["aa_equivalence"]["passed"])
            adjusted = output.replace('"observed_end_unix_nanos": 2200000000',
                                      '"observed_end_unix_nanos": 2400000000')
            binary.write_text(f"#!{sys.executable}\nprint({adjusted!r})\n")
            result = BENCHMARK.run_attempt(BENCHMARK.binary_record(binary), root, scenario,
                                           "baseline", "wall-adjustment", 5, "disabled")
            self.assertEqual(result["outcome"], "success", result)
            self.assertFalse(result["wall_alignment"]["profile_alignment_valid"])
            self.assertEqual(result["metrics"]["elapsed_ns"], 1_200_000_000)
            for invalid in (output.replace("throughput_tps=6.667", "throughput_tps=8.000"),
                            output.replace('"end_unix_nanos": 2200000000', '"end_unix_nanos": 2400000000'),
                            output.replace("callback_duplicates=0", "callback_duplicates=1")):
                binary.write_text(f"#!{sys.executable}\nprint({invalid!r})\n")
                result = BENCHMARK.run_attempt(BENCHMARK.binary_record(binary), root, scenario,
                                               "baseline", "rejection", 5, "disabled")
                self.assertEqual(result["category"], "invalid_evidence", result)

    def test_completion_rechecks_source_instead_of_cached_identity(self) -> None:
        source = {"root": "/fixed-source", "commit": "before"}
        contexts = {"baseline": {"source": source, "consensus": {}}}
        record = {"runner_sha256": "same", "process_runner_sha256": "same", "measurement_window_sha256": "same",
                  "rejection_diagnostics_sha256": "same",
                  "harness_sha256": "same", "host": {}, "sides": contexts,
                  "metric_scopes": BENCHMARK.METRIC_SCOPES}
        with mock.patch.object(BENCHMARK, "sha256", return_value="same"), mock.patch.object(
            BENCHMARK, "git_record", return_value={**source, "commit": "after"}
        ):
            with self.assertRaisesRegex(RuntimeError, "source changed during measurement"):
                BENCHMARK.validate_frozen(record, contexts, "same", {}, {"baseline": None})

    def test_result_record_requires_exact_callback_and_relay_terminal_fields(self) -> None:
        output = (
            "BENCH_RESULT scenario=always_success target=8 warm=1 workers=2 peers=2 "
            "elapsed_ns=100 throughput_tps=1.000 accepted=9 callback_duplicates=0 "
            "relay_ok=9 relay_duplicate_ok=0 relay_rejects=0 relay_unknown_parents=0 "
            "relay_generation_resets=0 p99_latency_ns=90 target_cpu_ns=80 "
            "allocation_calls=7 allocated_bytes=6 reorg_latency_ns=5 "
            "reorg_overlap_callbacks=0 shutdown_latency_ns=4"
        )
        match = BENCHMARK.RESULT.fullmatch(output)
        self.assertIsNotNone(match)
        self.assertEqual(match.group("relay_ok"), "9")
        self.assertNotRegex(output.replace(" relay_ok=9", ""), BENCHMARK.RESULT)

    def test_terminal_contract_rejects_loss_and_allows_only_scoped_duplicates(self) -> None:
        valid = {
            "scenario_name": "always_success",
            "expected_accepted": 9,
            "accepted": 9,
            "callback_duplicates": 0,
            "relay_ok": 9,
            "relay_duplicate_ok": 0,
            "relay_rejects": 0,
            "relay_unknown_parents": 0,
            "relay_generation_resets": 0,
            "expected_relay_rejects": 0,
        }
        self.assertIsNone(BENCHMARK.terminal_observation_error(**valid))
        for field in (
            "relay_duplicate_ok",
            "relay_rejects",
            "relay_generation_resets",
        ):
            invalid = dict(valid)
            invalid[field] = 1
            with self.subTest(field=field):
                self.assertIsNotNone(BENCHMARK.terminal_observation_error(**invalid))

        reorg = dict(valid, scenario_name="reorg_in_flight", callback_duplicates=2)
        self.assertIsNone(BENCHMARK.terminal_observation_error(**reorg))
        reverse = dict(valid, scenario_name="dependent_forest_8_reverse", relay_unknown_parents=8)
        self.assertIsNone(BENCHMARK.terminal_observation_error(**reverse))
        rbf = dict(
            valid,
            scenario_name="rbf_pairs",
            relay_rejects=8,
            expected_relay_rejects=8,
        )
        self.assertIsNone(BENCHMARK.terminal_observation_error(**rbf))
        rbf_missing = dict(rbf, relay_rejects=7)
        self.assertIsNotNone(BENCHMARK.terminal_observation_error(**rbf_missing))

    def test_final_timing_requires_profiling_disabled_build_identity(self) -> None:
        output = (
            "BENCH_BUILD profiling=false allocation_observation=false "
            "callback_observer=preallocated_atomic_slots_sharded_completion "
            "adapter=bounded_remote_batch "
            "debug_assertions=false measurement_window=terminal_completion_v3\n"
        )
        build, error = BENCHMARK.timing_build_observation(output, None, "disabled")
        self.assertIsNone(error)
        self.assertEqual(build["adapter"], "bounded_remote_batch")
        _, diagnostic_error = BENCHMARK.timing_build_observation(
            output + "BENCH_DIAGNOSTICS resource_phases=true\n", None, "disabled")
        self.assertIn("diagnostic instrumentation", diagnostic_error)
        allocation_output = output.replace(
            "allocation_observation=false", "allocation_observation=true"
        )
        allocation_build, error = BENCHMARK.timing_build_observation(
            allocation_output, None, "enabled"
        )
        self.assertIsNone(error)
        self.assertEqual(allocation_build["allocation_observation"], "true")

        for invalid_output, spans in (
            ("", None),
            (output.replace("profiling=false", "profiling=true"), None),
            (
                output.replace(
                    "allocation_observation=false", "allocation_observation=true"
                ),
                None,
            ),
            (
                output.replace(
                    "callback_observer=preallocated_atomic_slots_sharded_completion",
                    "callback_observer=locked_hash_set",
                ),
                None,
            ),
            (output.replace("debug_assertions=false", "debug_assertions=true"), None),
            (output.replace("terminal_completion_v3", "legacy_validation_and_latency_sort"), None),
            (output, {}),
        ):
            with self.subTest(output=invalid_output, spans=spans):
                _, error = BENCHMARK.timing_build_observation(
                    invalid_output, spans, "disabled"
                )
                self.assertIsNotNone(error)

    def test_corpus_identity_is_exact_and_pairing_rejects_drift(self) -> None:
        corpus = {
            "consensus_blake2b": "00" * 32,
            "cycle_assignment_count": 8,
            "cycles_blake2b": "11" * 32,
            "cycles_sum": 80,
            "script_preflight_count": 1,
            "transaction_bytes_blake2b": "22" * 32,
            "transaction_count": 8,
            "transaction_hashes_blake2b": "33" * 32,
        }
        self.assertIsNone(BENCHMARK.corpus_observation_error(corpus, 8))
        self.assertIsNone(BENCHMARK.paired_corpus_error(corpus, dict(corpus)))
        drift = dict(corpus, cycles_sum=81)
        self.assertIsNotNone(BENCHMARK.paired_corpus_error(corpus, drift))
        invalid = dict(corpus, script_preflight_count=9)
        self.assertIsNotNone(BENCHMARK.corpus_observation_error(invalid, 8))

    def test_terminal_multiset_requires_canonical_exact_records(self) -> None:
        terminals = {
            "callback_duplicates": 0,
            "relay_duplicate_ok": 0,
            "relay_generation_resets": 0,
            "relay_ok": 8,
            "relay_rejects": 0,
            "relay_unknown_parent_observations": [
                {"peer": 1, "parents": ["11" * 32], "count": 2}
            ],
        }
        arguments = {
            "callback_duplicates": 0,
            "relay_ok": 8,
            "relay_duplicate_ok": 0,
            "relay_rejects": 0,
            "relay_unknown_parents": 2,
            "relay_generation_resets": 0,
        }
        self.assertIsNone(BENCHMARK.terminal_record_error(terminals, **arguments))
        malformed = dict(terminals)
        malformed["relay_unknown_parent_observations"] = ["not-an-object"]
        self.assertIsNotNone(
            BENCHMARK.terminal_record_error(malformed, **arguments)
        )
        mismatched = dict(arguments, relay_unknown_parents=1)
        self.assertIsNotNone(
            BENCHMARK.terminal_record_error(terminals, **mismatched)
        )

    def test_long_rbf_preserves_the_precommitted_total_population_bound(self) -> None:
        scenario = BENCHMARK.parse_scenario("rbf_pairs,32768,32768,13,18")
        self.assertEqual(scenario["target"] + scenario["warm"], 65_536)
        with self.assertRaisesRegex(ValueError, "invalid scenario"):
            BENCHMARK.parse_scenario("rbf_pairs,32769,32768,13,18")

    def test_runner_builds_and_records_prod_profile(self) -> None:
        self.assertEqual(BENCHMARK.require_final_build_profile("prod"), "prod")
        with self.assertRaisesRegex(ValueError, "explicit prod"):
            BENCHMARK.require_final_build_profile("bench")
        with tempfile.TemporaryDirectory(prefix="txpool-cross-build-profile-") as raw:
            temporary = Path(raw)
            root = temporary / "source"
            target = temporary / "target"
            executable = target / "prod" / "deps" / "profile_one_shot-fixed"
            root.mkdir()
            executable.parent.mkdir(parents=True)
            executable.write_bytes(b"fixed-binary")
            message = {
                "reason": "compiler-artifact",
                "target": {"name": "profile_one_shot", "kind": ["bench"]},
                "executable": str(executable),
            }
            completed = subprocess.CompletedProcess(
                args=[], returncode=0, stdout=json.dumps(message), stderr=""
            )
            with mock.patch.object(BENCHMARK, "run_process", return_value=completed) as run:
                binary, build = BENCHMARK.build_binary(root, target, "profiling")

            command = run.call_args.args[0]
            self.assertIn("--profile", command)
            self.assertEqual(command[command.index("--profile") + 1], "prod")
            self.assertEqual(build["profile"], "prod")
            self.assertEqual(binary["sha256"], BENCHMARK.sha256(executable))

    def test_checkpoint_round_trip_and_attempt_ids_are_resume_authority(self) -> None:
        with tempfile.TemporaryDirectory(prefix="txpool-cross-checkpoint-") as raw:
            path = Path(raw) / "result.json"
            record = {"attempts": [{"id": "scenario/pilot/baseline"}]}
            BENCHMARK.write_checkpoint(path, record)
            self.assertEqual(BENCHMARK.read_checkpoint(path), record)
            self.assertEqual(
                list(BENCHMARK.attempt_index(record)), ["scenario/pilot/baseline"]
            )
            record["attempts"].append({"id": "scenario/pilot/baseline"})
            with self.assertRaisesRegex(RuntimeError, "duplicate attempt"):
                BENCHMARK.attempt_index(record)

    def test_resume_reuses_a_completed_attempt_id(self) -> None:
        scenario = BENCHMARK.parse_scenario("always_success,8,0,2,2")
        cached = {
            "id": "case/pilot/baseline",
            "outcome": "success",
            "side": "baseline",
            "scenario": scenario,
        }
        record = {"attempts": [cached]}
        with mock.patch.object(BENCHMARK, "run_attempt") as run:
            result = BENCHMARK.obtain_attempt(
                record,
                BENCHMARK.attempt_index(record),
                Path("unused"),
                {},
                scenario,
                "baseline",
                cached["id"],
                mock.Mock(),
            )
        self.assertIs(result, cached)
        run.assert_not_called()

    def test_metric_summary_derives_median_and_mad_from_pairs(self) -> None:
        samples = [
            {
                "baseline": {"metrics": {"elapsed_ns": 10}},
                "candidate": {"metrics": {"elapsed_ns": value}},
            }
            for value in (10, 20, 30)
        ]
        summary = BENCHMARK.metric_summary(samples, "elapsed_ns")
        self.assertEqual(summary["median_candidate_over_baseline"], 2.0)
        self.assertEqual(summary["ratio_relative_mad_percent"], 50.0)

    def test_exact_median_interval_uses_declared_coverage(self) -> None:
        six = BENCHMARK.median_interval([1, 2, 3, 4, 5, 6], 0.95)
        self.assertEqual((six["lower"], six["upper"]), (1, 6))
        self.assertEqual(six["coverage_under_independent_sampling"], 0.96875)
        ten = BENCHMARK.median_interval(list(range(1, 11)), 0.95)
        self.assertEqual((ten["lower_rank"], ten["upper_rank"]), (2, 9))
        self.assertEqual(ten["coverage_under_independent_sampling"], 0.978515625)
        self.assertIsNone(BENCHMARK.median_interval([1] * 5, 0.95))
        self.assertIsNone(BENCHMARK.median_interval([1] * 6, 0.99))
        for invalid in (0, float("nan"), float("inf")):
            with self.assertRaises(ValueError):
                BENCHMARK.median_interval([1] * 9 + [invalid], 0.95)

    @staticmethod
    def samples(ratios, duration=1_000_000_000):
        return [
            {side: {"metrics": {name: value for name in BENCHMARK.SUMMARY_METRICS},
                    "minimum_target_elapsed_ns": duration}
             for side, value in (("baseline", 1), ("candidate", ratio))}
            for ratio in ratios
        ]

    def test_low_mad_is_insufficient_when_interval_is_wide(self) -> None:
        samples = self.samples([0.9, 0.9] + [1] * 6 + [1.1, 1.1])
        summary = BENCHMARK.summarize_pairs(samples, {}, "disabled", 1.5)
        self.assertEqual(summary["metrics"]["throughput_tps"]["ratio_relative_mad_percent"], 0)
        self.assertEqual(summary["status"], "imprecise")
        self.assertEqual(summary["uncertainty_rule"]["imprecise_metrics"], list(BENCHMARK.PRECISION_METRICS))

    def test_direction_and_quality_are_separate_decisions(self) -> None:
        for ratio, direction in ((0.9, "lower"), (1, "unresolved"), (1.1, "higher")):
            summary = BENCHMARK.summarize_pairs(self.samples([ratio] * 10), {}, "disabled", 1.5)
            self.assertEqual(summary["status"], "comparable")
            self.assertEqual(summary["metrics"]["throughput_tps"]["ratio_direction"], direction)
        noisy = BENCHMARK.summarize_pairs(self.samples([0.95, 1.05] * 5), {}, "disabled", 1.5)
        self.assertEqual(noisy["status"], "noisy")

    def test_replicates_do_not_hide_short_individual_windows(self) -> None:
        summary = BENCHMARK.summarize_pairs(self.samples([1] * 10, duration=40_000_000), {}, "disabled", 1.5)
        self.assertEqual(summary["status"], "short_target_window")
        self.assertEqual(summary["duration_rule"]["observed_minimum_target_seconds"], 0.04)

    def test_aa_requires_whole_intervals_and_quality_not_only_direction(self) -> None:
        for ratio in (0.98, 0.99, 1, 1.01, 1.02):
            summary = BENCHMARK.summarize_pairs(self.samples([ratio] * 10), {}, "disabled", 1.5)
            BENCHMARK.classify_aa_equivalence(summary, 2)
            self.assertEqual(summary["status"], "aa_equivalent")
            self.assertTrue(summary["aa_equivalence"]["passed"])
            self.assertFalse(summary["production_ranking_permitted"])
        summary = BENCHMARK.summarize_pairs(self.samples([0.985] * 2 + [1] * 6 + [1.024] * 2), {}, "disabled", 1.5)
        self.assertEqual(summary["status"], "comparable")
        self.assertEqual(summary["metrics"]["throughput_tps"]["ratio_direction"], "unresolved")
        BENCHMARK.classify_aa_equivalence(summary, 2)
        self.assertEqual(summary["status"], "aa_equivalence_unresolved")
        self.assertFalse(summary["aa_equivalence"]["passed"])
        noisy = BENCHMARK.summarize_pairs(self.samples([0.99, 1.01] * 5), {}, "disabled", 0.5)
        BENCHMARK.classify_aa_equivalence(noisy, 2)
        self.assertTrue(all(noisy["aa_equivalence"]["whole_interval_within_margin"].values()))
        self.assertEqual(noisy["status"], "noisy")
        self.assertFalse(noisy["aa_equivalence"]["passed"])
        failed = BENCHMARK.failure_summary("measurement_failure", [])
        BENCHMARK.classify_aa_equivalence(failed, 2)
        self.assertEqual(failed["status"], "non_comparable")
        self.assertFalse(failed["aa_equivalence"]["passed"])

    @staticmethod
    def aa_command(root, output):
        return [str(SCRIPT), "--baseline-root", str(root), "--candidate-root", str(root),
                "--baseline-binary", str(root / "binary"), "--candidate-binary", str(root / "binary"),
                "--baseline-binary-profile", "prod", "--candidate-binary-profile", "prod",
                "--output", str(output), "--comparison", "aa", "--runs", "6",
                "--initial-cooldown-seconds", "0", "--cooldown-seconds", "0",
                "--scenario", "always_success,8,0,1,1"]

    def test_aa_cli_requires_an_explicit_valid_applicable_margin(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            command = self.aa_command(root, root / "result.json")
            invalid = [[], ["--allocation-observation", "enabled", "--aa-equivalence-margin-percent", "2"],
                       ["--comparison", "ab", "--aa-equivalence-margin-percent", "2"]]
            invalid += [["--aa-equivalence-margin-percent", value] for value in ("0", "-1", "100", "nan", "inf")]
            for extra in invalid:
                with self.subTest(extra=extra), mock.patch.object(sys, "argv", command + extra), mock.patch.object(sys, "stderr", io.StringIO()):
                    with self.assertRaises(SystemExit) as caught:
                        BENCHMARK.arguments()
                    self.assertEqual(caught.exception.code, 2)
            with mock.patch.object(sys, "argv", command + ["--calibration-only"]):
                self.assertIsNone(BENCHMARK.arguments().aa_equivalence_margin_percent)

    def test_aa_main_persists_its_decision_and_rejects_margin_change_on_resume(self) -> None:
        # Exercise orchestration and durable decisions with controlled observations;
        # the separate executable control covers resource collection and parsing.
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            (root / "binary").write_bytes(b"one fixed identity")
            harness = root / "tx-pool/benches/profile_one_shot.rs"
            harness.parent.mkdir(parents=True)
            harness.write_text("fixed harness")
            for name in BENCHMARK.HARNESS_FILES[1:]:
                helper = root / name
                helper.parent.mkdir(parents=True, exist_ok=True)
                helper.write_text("fixed helper")
            for ratio in (1, 1.03):
                output = root / f"result-{ratio}.json"
                command = self.aa_command(root, output) + ["--aa-equivalence-margin-percent", "2"]
                def observation(_binary, _root, scenario, side, attempt_id, *_):
                    value = round(1_000_000_000 * (ratio if side == "candidate" else 1))
                    metrics = {name: value for name in BENCHMARK.SUMMARY_METRICS}
                    metrics["reorg_overlap_callbacks"] = 0
                    return dict(id=attempt_id, side=side, scenario=scenario,
                                outcome="success", corpus={}, metrics=metrics)
                with mock.patch.object(BENCHMARK, "git_record", return_value={"root": str(root), "commit": "fixed"}), mock.patch.object(
                    BENCHMARK, "consensus_dependency_identity", return_value={"locked_packages": [], "enabled_features": []}
                ), mock.patch.object(BENCHMARK, "host_identity", return_value={}), mock.patch.object(
                    BENCHMARK, "run_attempt", side_effect=observation
                ) as run, mock.patch.object(sys, "stdout", io.StringIO()):
                    with mock.patch.object(sys, "argv", command):
                        if ratio == 1:
                            BENCHMARK.main()
                        else:
                            with self.assertRaises(SystemExit) as caught:
                                BENCHMARK.main()
                            self.assertEqual(caught.exception.code, 2)
                    record = BENCHMARK.read_checkpoint(output)
                    self.assertTrue(record["complete"])
                    summary = next(iter(record["summary"].values()))
                    self.assertEqual(summary["aa_equivalence"]["passed"], ratio == 1)
                    self.assertEqual(record["configuration"]["aa_equivalence_margin_percent"], 2)
                    self.assertEqual(run.call_count, 14)
                    self.assertEqual(len(record["attempts"]), 14)
                    self.assertTrue(all("environment_before" in a and "environment_after" in a for a in record["attempts"]))
                    before = output.read_bytes()
                    with mock.patch.object(sys, "argv", command + ["--resume", "--aa-equivalence-margin-percent", "3"]):
                        with self.assertRaisesRegex(RuntimeError, "configuration differs"):
                            BENCHMARK.main()
                    with mock.patch.object(sys, "argv", command + ["--resume", "--order-seed", "42"]):
                        with self.assertRaisesRegex(RuntimeError, "configuration differs"):
                            BENCHMARK.main()
                    self.assertEqual(run.call_count, 14)
                    self.assertEqual(output.read_bytes(), before)
                    archived = dict(record, schema=10)
                    BENCHMARK.write_checkpoint(output, archived)
                    with mock.patch.object(sys, "argv", command + ["--resume"]):
                        with self.assertRaisesRegex(RuntimeError, "schema or configuration differs"):
                            BENCHMARK.main()
                    self.assertEqual(BENCHMARK.read_checkpoint(output)["schema"], 10)
                    self.assertEqual(run.call_count, 14)


    def test_short_pilot_stops_before_comparative_measurement(self) -> None:
        scenario = BENCHMARK.parse_scenario("rbf_pairs,2000,2000,8,4")
        record = {"summary": {}}
        pilot = {"id": "pilot", "outcome": "success", "corpus": {}, "metrics": {"elapsed_ns": 40_000_000}}
        args = argparse.Namespace(calibration_only=False, min_target_seconds=0.25, allocation_observation="disabled")
        with mock.patch.object(BENCHMARK, "obtain_attempt", return_value=pilot) as run, mock.patch.object(BENCHMARK, "write_checkpoint"):
            BENCHMARK.run_scenario(record, {}, Path("unused"), {"baseline": {}, "candidate": {}}, scenario, args)
        self.assertEqual(run.call_count, 2)
        self.assertEqual(record["summary"][BENCHMARK.scenario_key(scenario)]["status"], "short_target_window")

    def test_calibration_never_creates_a_ranking(self) -> None:
        scenario = BENCHMARK.parse_scenario("always_success,16000,1000,8,4")
        record = {"summary": {}}
        pilot = {"id": "pilot", "outcome": "success", "corpus": {}, "metrics": {"elapsed_ns": 500_000_000}}
        args = argparse.Namespace(calibration_only=True, min_target_seconds=0.25)
        with mock.patch.object(BENCHMARK, "obtain_attempt", return_value=pilot) as run, mock.patch.object(BENCHMARK, "write_checkpoint"):
            BENCHMARK.run_scenario(record, {}, Path("unused"), {"baseline": {}, "candidate": {}}, scenario, args)
        self.assertEqual(run.call_count, 2)
        summary = record["summary"][BENCHMARK.scenario_key(scenario)]
        self.assertEqual(summary["status"], "calibrated")
        self.assertFalse(summary["ranking_permitted"])

    def test_interrupt_is_preserved_and_resume_cannot_replace_it(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            path = Path(raw) / "result.json"
            scenario = BENCHMARK.parse_scenario("always_success,8,0,1,1")
            args = argparse.Namespace(timeout_seconds=1, allocation_observation="disabled", comparison_contract="protocol")
            record = {"attempts": []}
            indexed = {}
            def interrupt(*_):
                self.assertEqual(BENCHMARK.read_checkpoint(path)["attempts"][0]["outcome"], "running")
                raise KeyboardInterrupt()
            with mock.patch.object(BENCHMARK, "run_attempt", side_effect=interrupt):
                with self.assertRaises(KeyboardInterrupt):
                    BENCHMARK.obtain_attempt(record, indexed, path, {"binary": {}, "source": {"root": raw}}, scenario, "baseline", "case", args)
            self.assertEqual(BENCHMARK.read_checkpoint(path)["attempts"][0]["category"], "runner_interrupted")
            with mock.patch.object(BENCHMARK, "run_attempt") as run:
                result = BENCHMARK.obtain_attempt(record, indexed, path, {}, scenario, "baseline", "case", args)
            self.assertEqual(result["outcome"], "failure")
            run.assert_not_called()

    def test_resume_marks_an_abandoned_running_attempt_as_failed(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            path = Path(raw) / "result.json"
            scenario = BENCHMARK.parse_scenario("always_success,8,0,1,1")
            record = {"attempts": [{"id": "case", "side": "baseline", "scenario": scenario, "outcome": "running"}]}
            with mock.patch.object(BENCHMARK, "run_attempt") as run:
                result = BENCHMARK.obtain_attempt(record, BENCHMARK.attempt_index(record), path, {}, scenario, "baseline", "case", argparse.Namespace())
            self.assertEqual(result["category"], "interrupted_attempt")
            self.assertEqual(BENCHMARK.read_checkpoint(path)["attempts"][0]["outcome"], "failure")
            run.assert_not_called()


    def test_omitted_rbf_notices_require_explicit_diagnostic_contract(self) -> None:
        scenario = BENCHMARK.parse_scenario("rbf_pairs,8,8,1,1")
        corpus = dict(consensus_blake2b="00" * 32, cycles_blake2b="11" * 32,
                      transaction_bytes_blake2b="22" * 32, transaction_hashes_blake2b="33" * 32,
                      cycle_assignment_count=16, cycles_sum=160, script_preflight_count=1,
                      transaction_count=16)
        terminals = dict(callback_duplicates=0, relay_duplicate_ok=0, relay_generation_resets=0,
                         relay_ok=16, relay_rejects=0, relay_unknown_parent_observations=[])
        output = (
            "BENCH_BUILD profiling=false allocation_observation=false "
            "callback_observer=preallocated_atomic_slots_sharded_completion "
            "adapter=bounded_remote_batch debug_assertions=false "
            "measurement_window=terminal_completion_v3 comparison_contract=CONTRACT\n"
            f"{window_record('rbf_pairs', 1_000_000_000)}"
            f"BENCH_CORPUS {json.dumps(corpus)}\nBENCH_TERMINALS {json.dumps(terminals)}\n{EMPTY_CAPTURE}"
            "BENCH_RESULT scenario=rbf_pairs target=8 warm=8 workers=1 peers=1 "
            "elapsed_ns=1000000000 throughput_tps=8.000 accepted=16 callback_duplicates=0 "
            "relay_ok=16 relay_duplicate_ok=0 relay_rejects=0 relay_unknown_parents=0 "
            "relay_generation_resets=0 p99_latency_ns=1 target_cpu_ns=1000000000 "
            "allocation_calls=0 allocated_bytes=0 reorg_latency_ns=1 "
            "reorg_overlap_callbacks=0 shutdown_latency_ns=1\n"
        )
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            binary = root / "omitted-notice-fixture"
            binary.write_text(f"#!{sys.executable}\nimport os\nprint({output!r}.replace('CONTRACT', os.environ['TX_POOL_BENCH_COMPARISON_CONTRACT']))\n")
            binary.chmod(0o700)
            identity = BENCHMARK.binary_record(binary)
            normal = BENCHMARK.run_attempt(identity, root, scenario, "candidate", "normal", 5, "disabled")
            self.assertEqual(normal["category"], "invalid_evidence")
            diagnostic = BENCHMARK.run_attempt(identity, root, scenario, "candidate", "diagnostic", 5,
                                               "disabled", BENCHMARK.RBF_ABLATION_CONTRACT)
            self.assertEqual(diagnostic["outcome"], "success", diagnostic)
            baseline = BENCHMARK.run_attempt(identity, root, scenario, "baseline", "baseline", 5,
                                             "disabled", BENCHMARK.RBF_ABLATION_CONTRACT)
            self.assertEqual(baseline["category"], "invalid_evidence")
            self.assertIn("comparison_contract=protocol", baseline["output"])
            legacy = output.replace("adapter=bounded_remote_batch", "adapter=legacy_peer_local_sequential")
            binary.write_text(f"#!{sys.executable}\nimport os\nprint({legacy!r}.replace('CONTRACT', os.environ['TX_POOL_BENCH_COMPARISON_CONTRACT']))\n")
            identity = BENCHMARK.binary_record(binary)
            baseline = BENCHMARK.run_attempt(identity, root, scenario, "baseline", "legacy-baseline", 5,
                                             "disabled", BENCHMARK.RBF_ABLATION_CONTRACT)
            self.assertEqual(baseline["outcome"], "success", baseline)
            candidate = BENCHMARK.run_attempt(identity, root, scenario, "candidate", "legacy-candidate", 5,
                                              "disabled", BENCHMARK.RBF_ABLATION_CONTRACT)
            self.assertEqual(candidate["category"], "invalid_evidence")

    def test_contract_diagnostic_cannot_become_a_production_ranking(self) -> None:
        scenario = BENCHMARK.parse_scenario("rbf_pairs,8,8,1,1")
        record = {"summary": {}}
        attempt = {"id": "fixture", "outcome": "success", "corpus": {},
                   "metrics": {name: 1 for name in BENCHMARK.SUMMARY_METRICS}}
        attempt["metrics"]["elapsed_ns"] = 1_000_000_000
        attempt["metrics"]["reorg_overlap_callbacks"] = 0
        args = argparse.Namespace(calibration_only=False, allocation_observation="disabled",
                                  comparison="ab",
                                  comparison_contract=BENCHMARK.RBF_ABLATION_CONTRACT,
                                  runs=6, replicates_per_sample=1, order_seed=0, min_target_seconds=0.25,
                                  max_paired_mad_percent=1.5, confidence_level=0.95,
                                  max_ratio_interval_width_percent=4)
        with mock.patch.object(BENCHMARK, "obtain_attempt", return_value=attempt), mock.patch.object(BENCHMARK, "write_checkpoint"):
            BENCHMARK.run_scenario(record, {}, Path("unused"), {"baseline": {}, "candidate": {}}, scenario, args)
        summary = record["summary"][BENCHMARK.scenario_key(scenario)]
        self.assertEqual(summary["status"], "contract_diagnostic")
        self.assertFalse(summary["production_ranking_permitted"])


class MeasurementRepairTest(unittest.TestCase):
    def test_outlier_is_preserved_in_mean_and_maximum(self):
        attempts = [{"id": str(i), "metrics": {name: 1 for name in
                     set(BENCHMARK.SUM_METRICS + BENCHMARK.MAX_METRICS)}} for i in range(4)]
        for attempt, rss in zip(attempts, [100, 100, 100, 500]):
            attempt["metrics"]["peak_rss_bytes"] = rss
        summary = BENCHMARK.aggregate_side(attempts, 1)
        self.assertEqual(summary["metrics"]["mean_peak_rss_bytes"], 200)
        self.assertEqual(summary["metrics"]["peak_rss_bytes"], 500)
        self.assertEqual(summary["attempt_ids"], ["0", "1", "2", "3"])
        self.assertEqual(attempts[-1]["metrics"]["peak_rss_bytes"], 500)

    def test_memory_uncertainty_does_not_erase_throughput_quality(self):
        samples = [{side: {"minimum_target_elapsed_ns": 1_000_000_000,
                    "metrics": {name: 1 for name in BENCHMARK.SUMMARY_METRICS}}
                    for side in ("baseline", "candidate")} for _ in range(10)]
        for sample, ratio in zip(samples, [.9, .9] + [1] * 6 + [1.1, 1.1]):
            sample["candidate"]["metrics"]["mean_peak_rss_bytes"] = ratio
        summary = BENCHMARK.summarize_pairs(samples, {}, "disabled", 1.5)
        BENCHMARK.classify_aa_equivalence(summary, 2)
        self.assertEqual(summary["status"], "imprecise")
        self.assertFalse(summary["aa_equivalence"]["passed"])
        self.assertEqual(summary["metric_quality"]["throughput_tps"]["aa_disposition"], "equivalent")
        self.assertEqual(summary["metric_quality"]["mean_peak_rss_bytes"]["aa_disposition"], "unresolved")

    def test_seeded_schedule_is_balanced_and_reproducible(self):
        for replicates in (1, 2, 4, 8):
            schedule = BENCHMARK.balanced_schedule(24, replicates, 17, "scenario")
            self.assertEqual(schedule, BENCHMARK.balanced_schedule(24, replicates, 17, "scenario"))
            self.assertNotEqual(schedule, BENCHMARK.balanced_schedule(24, replicates, 18, "scenario"))
            starts = [order[0] for block in schedule for order in block]
            self.assertEqual(starts.count("baseline"), len(starts) // 2)
            if replicates > 1:
                self.assertTrue(all(sum(order[0] == "baseline" for order in block) == replicates // 2 for block in schedule))

    def test_every_helper_and_added_file_changes_bundle_identity(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            for name in BENCHMARK.HARNESS_FILES:
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("original")
            original = BENCHMARK.bundle_hash(root)
            for name in BENCHMARK.HARNESS_FILES:
                path = root / name
                path.write_text("changed")
                self.assertNotEqual(BENCHMARK.bundle_hash(root), original)
                path.write_text("original")
            (root / "tx-pool/benches/future_helper").mkdir()
            (root / "tx-pool/benches/future_helper/extra.rs").write_text("extra")
            self.assertNotEqual(BENCHMARK.bundle_hash(root), original)

    def test_window_parser_change_rejects_frozen_record(self):
        record = dict(metric_scopes=BENCHMARK.METRIC_SCOPES,
                      runner_sha256="same", process_runner_sha256="same", measurement_window_sha256="old")
        with mock.patch.object(BENCHMARK, "sha256", return_value="same"):
            with self.assertRaisesRegex(RuntimeError, "window parser changed"):
                BENCHMARK.validate_frozen(record, {}, "same", {}, {})

    def test_rejection_verifier_change_rejects_frozen_record(self):
        record = dict(metric_scopes=BENCHMARK.METRIC_SCOPES,
                      runner_sha256="same", process_runner_sha256="same", measurement_window_sha256="same",
                      rejection_diagnostics_sha256="old")
        with mock.patch.object(BENCHMARK, "sha256", return_value="same"):
            with self.assertRaisesRegex(RuntimeError, "rejection diagnostic verifier changed"):
                BENCHMARK.validate_frozen(record, {}, "same", {}, {})


if __name__ == "__main__":
    unittest.main()
