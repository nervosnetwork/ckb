"""The executable and its declared source must come from one Cargo observation."""

import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import benchmark_build as build


def rocksdb_observation(flags=("-std=c++17",)):
    package = "registry+https://github.com/rust-lang/crates.io-index#ckb-librocksdb-sys@8.5.4"
    return dict(package_id=package, detected_cxxflags=None if flags is None else list(flags),
                build_script=dict(reason="build-script-executed", package_id=package,
                                  out_dir="/removed-target/build/rocksdb/out"))


def build_receipt(source, binary):
    return dict(schema=2, kind="tx_pool_benchmark_build", source=source, binary=binary,
                bench="profile_one_shot", features="", profile="prod", toolchain=dict(rustc="fixed", cargo="fixed"),
                command=build.build_command("profile_one_shot", ""), rocksdb_build=rocksdb_observation(),
                cargo_artifact=dict(reason="compiler-artifact", executable=binary["path"],
                                    target=dict(name="profile_one_shot", kind=["bench"])))


def rocksdb_messages(root, *, fresh=False):
    observation = rocksdb_observation()
    script = observation["build_script"] | {"out_dir": str(root / "selected/out")}
    output = Path(script["out_dir"]).parent / "output"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text("PLATFORM_CXXFLAGS: " + json.dumps(observation["detected_cxxflags"]) + "\n")
    library = dict(reason="compiler-artifact", package_id=observation["package_id"], fresh=fresh,
                   target=dict(name="ckb_librocksdb_sys", kind=["lib"]))
    return [library, script], output


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
            messages, native_output = rocksdb_messages(root)
            completed = subprocess.CompletedProcess([], 0, "\n".join(map(json.dumps, [artifact, *messages])), "")
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
                native_output.unlink()  # Replay uses the receipt, never the mutable Cargo cache.
                binary, loaded = build.load_build(path, root, "profile_one_shot", "", copy_path)
                self.assertEqual(loaded, receipt)
                self.assertEqual(binary["path"], str(copy_path.resolve()))
                build.validate_rocksdb_builds(receipt, loaded)
                legacy = receipt | {"schema": 1}
                legacy.pop("rocksdb_build")
                build.validate_build(legacy, source, "profile_one_shot", "", binary)
                with self.assertRaisesRegex(RuntimeError, "requires recorded"):
                    build.validate_rocksdb_builds(legacy, legacy)
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

    def test_rocksdb_observation_uses_cargos_selected_fresh_or_cached_output(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for fresh in (False, True):
                messages, selected = rocksdb_messages(root, fresh=fresh)
                flags = ["-std=c++17", "-DORDER=1", "-DORDER=2", "-DHAVE_FULLFSYNC"]
                selected.write_text("cargo:rerun-if-changed=build.rs\nPLATFORM_CXXFLAGS:  " + json.dumps(flags) + "\n")
                decoy = root / "other-cached-build/output"
                decoy.parent.mkdir(exist_ok=True)
                decoy.write_text('PLATFORM_CXXFLAGS: ["wrong cache"]\n')
                later = selected.stat().st_mtime_ns + 1_000_000_000
                os.utime(decoy, ns=(later, later))
                observation = build.rocksdb_build_observation("\n".join(map(json.dumps, messages)))
                self.assertEqual(observation["detected_cxxflags"], flags)
                self.assertEqual(observation["build_script"], messages[1])
                for malformed in ("null", "{}", '["flag", 1]', '["unterminated]',
                                  '[]\nPLATFORM_CXXFLAGS: []'):
                    selected.write_text("PLATFORM_CXXFLAGS: " + malformed)
                    with self.subTest(malformed=malformed), self.assertRaisesRegex(RuntimeError, "cannot observe"):
                        build.rocksdb_build_observation("\n".join(map(json.dumps, messages)))
                selected.write_text("no detected flags\n")
                unknown = build.rocksdb_build_observation("\n".join(map(json.dumps, messages)))
                self.assertIsNone(unknown["detected_cxxflags"])
                for invalid in ([], messages[:1], messages[1:], [*messages, messages[0]], [*messages, messages[1]]):
                    with self.assertRaises(RuntimeError):
                        build.rocksdb_build_observation("\n".join(map(json.dumps, invalid)))
                selected.unlink()
                with self.assertRaisesRegex(RuntimeError, "cannot observe"):
                    build.rocksdb_build_observation("\n".join(map(json.dumps, messages)))

    def test_comparison_requires_known_ordered_flags_without_assuming_host_capabilities(self):
        def receipt(flags):
            return dict(schema=2, rocksdb_build=rocksdb_observation(flags))
        for flags in ([], ["-std=c++17"], ["-DFLAG", "-DFLAG", "-UFLAG"]):
            original = receipt(flags)
            relocated = receipt(flags)
            relocated["rocksdb_build"]["build_script"]["out_dir"] = "/other-target/out"
            build.validate_rocksdb_builds(original, relocated)
        baseline = receipt(["-DFLAG", "-DFLAG", "-UFLAG"])
        for flags in (["-DFLAG", "-UFLAG"], ["-UFLAG", "-DFLAG", "-DFLAG"],
                      ["-DFLAG", "-DFLAG", "-UFLAG", "-DHAVE_FULLFSYNC"]):
            with self.assertRaisesRegex(RuntimeError, "flags differ"):
                build.validate_rocksdb_builds(baseline, receipt(flags))
        for unknown in (dict(schema=1), receipt(None)):
            with self.assertRaisesRegex(RuntimeError, "requires recorded"):
                build.validate_rocksdb_builds(unknown, unknown)
        for change in (dict(package_id="another package"), dict(build_script={}),
                       dict(detected_cxxflags=True)):
            invalid = receipt([])
            invalid["rocksdb_build"].update(change)
            with self.assertRaisesRegex(RuntimeError, "invalid RocksDB"):
                build.rocksdb_configuration(invalid)

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
