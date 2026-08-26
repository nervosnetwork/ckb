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
SPEC = importlib.util.spec_from_file_location("txpool_cross_version_benchmark", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("cannot import cross_version_benchmark.py")
BENCHMARK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BENCHMARK)


class BuildProfileContractTest(unittest.TestCase):
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
