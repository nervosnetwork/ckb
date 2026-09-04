#!/usr/bin/env python3
"""Core evidence and resume canaries for ``cross_version_benchmark.py``."""

from __future__ import annotations

import importlib.util
import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

SCRIPT = Path(__file__).resolve().with_name("cross_version_benchmark.py")
SPEC = importlib.util.spec_from_file_location("txpool_cross_version_benchmark", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("cannot import cross_version_benchmark.py")
BENCHMARK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BENCHMARK)


class BuildProfileContractTest(unittest.TestCase):
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
            "debug_assertions=false\n"
        )
        build, error = BENCHMARK.timing_build_observation(output, None, "disabled")
        self.assertIsNone(error)
        self.assertEqual(build["adapter"], "bounded_remote_batch")
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
            with mock.patch.object(BENCHMARK.subprocess, "run", return_value=completed) as run:
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


if __name__ == "__main__":
    unittest.main()
