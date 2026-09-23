"""The executable and its declared source must come from one Cargo observation."""

import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import benchmark_build as build


class BenchmarkBuildTests(unittest.TestCase):
    def test_observation_mode_has_one_effective_feature_set(self):
        self.assertEqual(build.effective_features("legacy, allocation-observation ckb-tx-pool/legacy", True,
                                                 "profile_one_shot"),
                         "ckb-tx-pool/allocation-observation,ckb-tx-pool/legacy")
        self.assertEqual(build.effective_features("", True, "packing_one_shot"),
                         "ckb-tx-pool/allocation-observation,ckb-tx-pool/packing-bench")
        for name in ("allocation-observation", "ckb-tx-pool/allocation-observation"):
            with self.assertRaisesRegex(ValueError, "requires allocation"):
                build.effective_features(name, False, "profile_one_shot")

    def test_cargo_artifact_binds_source_and_copied_executable(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            executable = root / "bench"
            executable.write_bytes(b"Cargo-produced binary")
            source = {"root": str(root), "commit": "frozen"}
            artifact = {"reason": "compiler-artifact", "target": {"name": "profile_one_shot", "kind": ["bench"]},
                        "executable": str(executable)}
            completed = subprocess.CompletedProcess([], 0, json.dumps(artifact), "")
            with mock.patch.object(build, "git_record", return_value=source), mock.patch.object(
                    build, "command_output", return_value="fixed toolchain"), mock.patch.object(
                    build, "run_process", return_value=completed) as run:
                receipt = build.build_binary(root, root / "target", "", "profile_one_shot")
                command = run.call_args.args[0]
                self.assertEqual(command[command.index("--profile") + 1], "prod")
                self.assertEqual(receipt["cargo_artifact"], artifact | {"executable": str(executable.resolve())})
                self.assertEqual(receipt["source"], source)
                path = root / "build.json"
                path.write_text(json.dumps(receipt))
                copy_path = root / "copied"
                copy_path.write_bytes(executable.read_bytes())
                binary, loaded = build.load_build(path, root, "profile_one_shot", "", copy_path)
                self.assertEqual(loaded, receipt)
                self.assertEqual(binary["path"], str(copy_path.resolve()))
                for field in ("sha256", "size"):
                    incomplete = receipt | {"binary": receipt["binary"] | {field: None}}
                    with self.subTest(missing=field), self.assertRaises(RuntimeError):
                        build.validate_build(incomplete, source, "profile_one_shot", "", binary | {field: None})
                for changed in ({"source": {"commit": "other"}}, {"bench": "packing_one_shot"},
                                {"profile": "dev"}, {"features": "ckb-tx-pool/profiling"}, {"schema": 0},
                                {"cargo_artifact": {}}, {"command": []}, {"toolchain": {}}):
                    with self.subTest(changed=changed):
                        path.write_text(json.dumps(receipt | changed))
                        with self.assertRaises(RuntimeError):
                            build.load_build(path, root, "profile_one_shot", "", copy_path)
                path.write_text(json.dumps(receipt))
                copy_path.write_bytes(b"different binary")
                with self.assertRaisesRegex(RuntimeError, "binary differs"):
                    build.load_build(path, root, "profile_one_shot", "", copy_path)

    def test_failed_ambiguous_or_drifting_build_has_no_receipt(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            artifact = {"reason": "compiler-artifact", "target": {"name": "profile_one_shot", "kind": ["bench"]},
                        "executable": str(root / "binary")}
            for status, output in ((1, "error"), (0, ""), (0, "\n".join([json.dumps(artifact)] * 2))):
                with self.subTest(status=status, output=output), mock.patch.object(
                        build, "git_record", return_value={}), mock.patch.object(
                        build, "command_output", return_value="fixed"), mock.patch.object(
                        build, "run_process", return_value=subprocess.CompletedProcess([], status, output, "")):
                    with self.assertRaises(RuntimeError):
                        build.build_binary(root, root / "target", "", "profile_one_shot")
            with mock.patch.object(build, "git_record", side_effect=[{"commit": "before"}, {"commit": "after"}]), mock.patch.object(
                    build, "command_output", return_value="fixed"), mock.patch.object(
                    build, "run_process", return_value=subprocess.CompletedProcess([], 0, json.dumps(artifact), "")):
                with self.assertRaisesRegex(RuntimeError, "source changed"):
                    build.build_binary(root, root / "target", "", "profile_one_shot")

    def test_host_identity_distinguishes_node_and_cpu(self):
        with mock.patch.object(build.platform, "system", return_value="Darwin"), mock.patch.object(
                build.platform, "node", return_value="measurement-host"), mock.patch.object(
                build, "command_output", side_effect=["test CPU", "rustc version", "cargo version"]):
            identity = build.host_identity()
        self.assertEqual(identity["node"], "measurement-host")
        self.assertEqual(identity["cpu_model"], "test CPU")


if __name__ == "__main__":
    unittest.main()
