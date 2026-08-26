#!/usr/bin/env python3
"""Focused build-profile contract tests for ``cross_version_benchmark.py``."""

from __future__ import annotations

import importlib.util
import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

SCRIPT = Path(__file__).resolve().with_name("cross_version_benchmark.py")
HARNESS = SCRIPT.parents[1] / "benches" / "profile_one_shot.rs"
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

    def test_legacy_adapter_uses_native_unbounded_relay_with_active_consumer(self) -> None:
        source = HARNESS.read_text()
        self.assertIn("ckb_channel::unbounded()", source)
        self.assertNotIn("ckb_channel::bounded(1024)", source)
        self.assertIn("RelayDrainGuard::start(relay_receiver)", source)

    def test_final_timing_requires_profiling_disabled_build_identity(self) -> None:
        output = (
            "BENCH_BUILD profiling=false adapter=bounded_remote_batch "
            "debug_assertions=false\n"
        )
        build, error = BENCHMARK.timing_build_observation(output, None)
        self.assertIsNone(error)
        self.assertEqual(build["adapter"], "bounded_remote_batch")

        for invalid_output, spans in (
            ("", None),
            (output.replace("profiling=false", "profiling=true"), None),
            (output.replace("debug_assertions=false", "debug_assertions=true"), None),
            (output, {}),
        ):
            with self.subTest(output=invalid_output, spans=spans):
                _, error = BENCHMARK.timing_build_observation(invalid_output, spans)
                self.assertIsNotNone(error)

    def test_long_rbf_preserves_the_precommitted_total_population_bound(self) -> None:
        scenario = BENCHMARK.parse_scenario("rbf_pairs,32768,32768,13,18")
        self.assertEqual(scenario["target"] + scenario["warm"], 65_536)
        with self.assertRaisesRegex(ValueError, "invalid scenario"):
            BENCHMARK.parse_scenario("rbf_pairs,32769,32768,13,18")

    def test_fixed_binary_requires_explicit_prod_profile(self) -> None:
        self.assertEqual(BENCHMARK.require_final_build_profile("prod"), "prod")
        for invalid in (None, "bench", "release"):
            with self.subTest(invalid=invalid):
                with self.assertRaisesRegex(ValueError, "explicit prod"):
                    BENCHMARK.require_final_build_profile(invalid)

    def test_runner_builds_and_records_prod_profile(self) -> None:
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


if __name__ == "__main__":
    unittest.main()
