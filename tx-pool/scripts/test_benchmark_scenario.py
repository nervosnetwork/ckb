"""One boundary corpus exercises the native parser and both Python entry points."""

import importlib.util
import io
import json
import sys
import unittest
from pathlib import Path
from unittest import mock

SCRIPTS = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPTS))
import cross_version_benchmark as benchmark
from benchmark_scenario import validate_scenario

spec = importlib.util.spec_from_file_location("scenario_profile_cli", SCRIPTS / "profile.py")
profile = importlib.util.module_from_spec(spec)
spec.loader.exec_module(profile)
CASES = json.loads((SCRIPTS.parent / "benches/scenario/cases.json").read_text())


class ScenarioBoundaries(unittest.TestCase):
    def test_both_python_entry_points_match_native_boundary_corpus(self):
        for case in CASES:
            args, valid = case["args"], case["valid"]
            with self.subTest(args=args):
                arguments = [args[0], *map(int, args[1:])]
                if valid:
                    validate_scenario(*arguments)
                    self.assertEqual(benchmark.parse_scenario(",".join(args))["name"], args[0])
                else:
                    with self.assertRaises(ValueError):
                        validate_scenario(*arguments)
                    with self.assertRaises(ValueError):
                        benchmark.parse_scenario(",".join(args))
                command = ["profile.py", "capture", "--output-prefix", "/tmp/profile-fixture"]
                for key, value in zip(("scenario", "target", "warm", "workers", "peers"), args):
                    command.extend([f"--{key}", value])
                with mock.patch.object(sys, "argv", command), mock.patch("sys.stderr", new=io.StringIO()):
                    if valid:
                        self.assertEqual(profile.parse_args().scenario, args[0])
                    else:
                        with self.assertRaises(SystemExit) as error:
                            profile.parse_args()
                        self.assertEqual(error.exception.code, 2)

    def test_parser_identity_is_frozen_for_resume_and_profile(self):
        record = dict(metric_scopes=benchmark.METRIC_SCOPES,
                      runner_sha256="same", build_runner_sha256="same", process_runner_sha256="same",
                      measurement_window_sha256="same", rejection_diagnostics_sha256="same",
                      scenario_parser_sha256="old")
        with mock.patch.object(benchmark, "sha256", return_value="same"):
            with self.assertRaisesRegex(RuntimeError, "scenario parser changed"):
                benchmark.validate_frozen(record, {}, "same", {}, {})
        self.assertIn(SCRIPTS / "benchmark_scenario.py", profile.harness_sources())


if __name__ == "__main__":
    unittest.main()
