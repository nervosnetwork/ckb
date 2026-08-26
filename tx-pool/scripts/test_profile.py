#!/usr/bin/env python3
"""Executable negative and portability tests for ``profile.py``."""

from __future__ import annotations

import importlib.util
import json
import shutil
import tempfile
import unittest
from pathlib import Path
from unittest import mock


SCRIPT = Path(__file__).resolve().with_name("profile.py")
SPEC = importlib.util.spec_from_file_location("txpool_profile", SCRIPT)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("cannot import profile.py")
PROFILE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PROFILE)


class ProfileAnalyzerTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="txpool-profile-test-")
        self.root = Path(self.temporary.name)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def test_reused_binary_requires_explicit_prod_profile(self) -> None:
        self.assertEqual(PROFILE.require_final_build_profile("prod"), "prod")
        for invalid in (None, "bench", "release"):
            with self.subTest(invalid=invalid):
                with self.assertRaisesRegex(ValueError, "explicit prod"):
                    PROFILE.require_final_build_profile(invalid)

    def test_capture_build_uses_prod_profile(self) -> None:
        executable = self.root / "target" / "prod" / "deps" / "profile_one_shot"
        executable.parent.mkdir(parents=True)
        executable.write_bytes(b"profile-binary")
        message = {
            "reason": "compiler-artifact",
            "target": {"name": "profile_one_shot", "kind": ["bench"]},
            "executable": str(executable),
        }
        completed = mock.Mock(
            returncode=0,
            stdout=json.dumps(message),
            stderr="",
        )
        with mock.patch.object(PROFILE.subprocess, "run", return_value=completed) as run:
            binary, command, _ = PROFILE.build_binary(
                self.root / "target", "profile_one_shot", ("profiling",)
            )

        self.assertEqual(binary, executable.resolve())
        self.assertIn("--profile", command)
        self.assertEqual(command[command.index("--profile") + 1], "prod")
        self.assertEqual(run.call_args.args[0], command)

    @staticmethod
    def write_json(path: Path, value: object) -> None:
        path.write_text(json.dumps(value, sort_keys=True) + "\n")

    def bundle(
        self,
        name: str,
        *,
        absolute_time: bool,
        coordinates: list[float] | None = None,
    ) -> Path:
        bundle = self.root / name
        bundle.mkdir()
        sample_coordinates = coordinates or ([1.0, 2.0, 3.0] if absolute_time else [1.0, 1.0, 1.0])
        time_field = "time" if absolute_time else "timeDeltas"
        profile = {
            "libs": [{"name": "synthetic", "codeId": "synthetic-code"}],
            "meta": {
                "startTime": 1_000.0,
                "interval": 1.0,
                "sampleUnits": {"threadCPUDelta": "µs"},
            },
            "threads": [
                {
                    "name": "worker",
                    "pid": 1,
                    "tid": 2,
                    "samples": {
                        "length": 3,
                        time_field: sample_coordinates,
                        "stack": [0, 0, 0],
                        "threadCPUDelta": [10.0, 20.0, 30.0],
                    },
                    "stackTable": {"length": 1, "frame": [0], "prefix": [None]},
                    "frameTable": {"length": 1, "func": [0], "address": [0]},
                    "funcTable": {"length": 1, "name": [0], "resource": [0]},
                    "resourceTable": {"length": 1, "lib": [0]},
                    "stringArray": ["synthetic_leaf"],
                }
            ],
        }
        sidecar = {"string_table": [], "data": []}
        window = {
            "schema_version": 1,
            "scenario": "synthetic",
            "start_unix_nanos": 1_001_000_000,
            "end_unix_nanos": 1_003_000_000,
            "elapsed_nanos": 2_000_000,
        }
        spans = {
            "schema_version": 2,
            "measurement": "span_lifetimes_started_during_target_work",
            "window": window,
            "spans": [
                {
                    "name": "tx_pool.authority.write_hold",
                    "start_count": 3,
                    "elapsed_nanos": 900,
                },
                {
                    "name": "tx_pool.ingress.remote_batch",
                    "start_count": 1,
                    "elapsed_nanos": 100,
                },
            ],
        }
        marker = f"{PROFILE.MARKER_PREFIX}{json.dumps(window, sort_keys=True)}\n"
        values = {
            "profile.json": profile,
            "symbols.json": sidecar,
            "stdout.log": marker,
            "stderr.log": "",
            "spans.json": spans,
            "span.stdout.log": marker,
            "span.stderr.log": "",
        }
        for filename, value in values.items():
            path = bundle / filename
            if isinstance(value, str):
                path.write_text(value)
            else:
                self.write_json(path, value)
        sources = [PROFILE.BENCHMARK_SOURCE, PROFILE.SCRIPT_SOURCE]
        artifacts = {
            label: PROFILE.artifact(bundle / filename, bundle)
            for label, filename in {
                "profile": "profile.json",
                "symbols": "symbols.json",
                "stdout": "stdout.log",
                "stderr": "stderr.log",
                "spans": "spans.json",
                "span_stdout": "span.stdout.log",
                "span_stderr": "span.stderr.log",
            }.items()
        }
        manifest = {
            "schema_version": PROFILE.MANIFEST_SCHEMA_VERSION,
            "git": {},
            "harness": "pipeline",
            "features": list(PROFILE.PIPELINE_FEATURES),
            "scenario": {"scenario": "synthetic"},
            "observation": None,
            "window": window,
            "capture": {"sample_rate_hz": 1_000},
            "span_capture": {"window": window},
            "environment": {},
            "inputs": {
                "workspace_manifest_sha256": "0" * 64,
                "cargo_lock_sha256": "0" * 64,
                "tx_pool_manifest_sha256": "0" * 64,
                "harness_sources": [
                    source.relative_to(PROFILE.WORKSPACE_ROOT).as_posix()
                    for source in sources
                ],
                "harness_sha256": PROFILE.files_sha256(sources),
                "binary": {
                    "path_at_capture": "/discarded/synthetic-binary",
                    "size_bytes": 1,
                    "sha256": "1" * 64,
                },
            },
            "artifacts": artifacts,
            "summary_path": "summary.json",
        }
        manifest_path = bundle / "manifest.json"
        self.write_json(manifest_path, manifest)
        return manifest_path

    def test_absolute_and_delta_coordinates_produce_the_same_window(self) -> None:
        absolute = PROFILE.analyze_manifest(
            self.bundle("absolute", absolute_time=True)
        )
        delta = PROFILE.analyze_manifest(self.bundle("delta", absolute_time=False))
        absolute_summary = PROFILE.read_json(absolute)
        delta_summary = PROFILE.read_json(delta)
        self.assertEqual(absolute_summary["sampling"], delta_summary["sampling"])
        self.assertEqual(
            absolute_summary["top_leaf_symbols_by_thread_cpu_delta"],
            delta_summary["top_leaf_symbols_by_thread_cpu_delta"],
        )
        self.assertEqual(
            absolute_summary["top_leaf_symbols_by_window_samples"],
            delta_summary["top_leaf_symbols_by_window_samples"],
        )
        self.assertEqual(
            absolute_summary["span_capture"]["selected_span_elapsed_nanos"], 1_000
        )

    def test_bundle_remains_analyzable_after_move_and_binary_loss(self) -> None:
        manifest = self.bundle("portable", absolute_time=True)
        moved = self.root / "moved"
        shutil.move(str(manifest.parent), moved)
        summary = PROFILE.analyze_manifest(moved / "manifest.json")
        self.assertTrue(summary.is_file())

    def test_artifact_tampering_is_rejected(self) -> None:
        manifest = self.bundle("tamper", absolute_time=True)
        (manifest.parent / "profile.json").write_text("{}\n")
        with self.assertRaisesRegex(PROFILE.ProfileError, "(size|hash) changed"):
            PROFILE.analyze_manifest(manifest)

    def test_nonmonotonic_absolute_coordinates_are_rejected(self) -> None:
        manifest = self.bundle(
            "nonmonotonic", absolute_time=True, coordinates=[1.0, 0.5, 3.0]
        )
        with self.assertRaisesRegex(PROFILE.ProfileError, "not monotonic"):
            PROFILE.analyze_manifest(manifest)

    def test_sequential_ingress_shape_is_rejected(self) -> None:
        manifest_path = self.bundle("sequential", absolute_time=True)
        spans_path = manifest_path.parent / "spans.json"
        spans = PROFILE.read_json(spans_path)
        spans["spans"] = [
            row
            for row in spans["spans"]
            if row["name"] != "tx_pool.ingress.remote_batch"
        ]
        self.write_json(spans_path, spans)
        manifest = PROFILE.read_json(manifest_path)
        manifest["artifacts"]["spans"] = PROFILE.artifact(
            spans_path, manifest_path.parent
        )
        self.write_json(manifest_path, manifest)
        with self.assertRaisesRegex(PROFILE.ProfileError, "remote-batch ingress"):
            PROFILE.analyze_manifest(manifest_path)

    def test_one_shot_observation_identity_is_checked(self) -> None:
        expected = {
            "scenario": "always_success",
            "target": 4,
            "warm": 1,
            "workers": 2,
            "peers": 2,
        }
        observation = {
            "schema_version": 1,
            **expected,
            "elapsed_nanos": 1,
            "throughput_tps": 1.0,
            "accepted": 5,
            "p99_latency_nanos": 1,
            "target_cpu_nanos": 1,
            "target_user_cpu_nanos": 1,
            "target_system_cpu_nanos": 0,
            "allocation_calls": 1,
            "allocated_bytes": 1,
            "reorg_latency_nanos": 1,
            "reorg_overlap_callbacks": 0,
            "shutdown_latency_nanos": 1,
        }
        stdout = f"{PROFILE.OBSERVATION_PREFIX}{json.dumps(observation)}\n"
        self.assertEqual(PROFILE.parse_observation(stdout, expected), observation)
        with self.assertRaisesRegex(PROFILE.ProfileError, "drifted"):
            PROFILE.parse_observation(stdout, {**expected, "peers": 1})

    def test_one_shot_observation_v2_terminal_identity_is_checked(self) -> None:
        expected = {
            "scenario": "rbf_pairs",
            "target": 4,
            "warm": 2,
            "workers": 2,
            "peers": 2,
        }
        observation = {
            "schema_version": 2,
            **expected,
            "elapsed_nanos": 1,
            "throughput_tps": 1.0,
            "accepted": 6,
            "callback_duplicates": 0,
            "p99_latency_nanos": 1,
            "target_cpu_nanos": 1,
            "target_user_cpu_nanos": 1,
            "target_system_cpu_nanos": 0,
            "allocation_calls": 1,
            "allocated_bytes": 1,
            "reorg_latency_nanos": 1,
            "reorg_overlap_callbacks": 0,
            "relay_ok": 6,
            "relay_duplicate_ok": 0,
            "relay_rejects": 2,
            "relay_unknown_parents": 0,
            "relay_generation_resets": 0,
            "shutdown_latency_nanos": 1,
        }
        stdout = f"{PROFILE.OBSERVATION_PREFIX}{json.dumps(observation)}\n"
        self.assertEqual(PROFILE.parse_observation(stdout, expected), observation)
        with self.assertRaisesRegex(PROFILE.ProfileError, "unexpected reject"):
            PROFILE.parse_observation(
                f"{PROFILE.OBSERVATION_PREFIX}{json.dumps({**observation, 'relay_rejects': 0})}\n",
                expected,
            )

    def test_presymbolication_sidecar_resolves_an_address_frame(self) -> None:
        profile = {"libs": [{"name": "synthetic", "codeId": "code"}]}
        sidecar = {
            "string_table": ["resolved_symbol"],
            "data": [
                {
                    "code_id": "code",
                    "known_addresses": [[256, 0]],
                    "symbol_table": [
                        {"rva": 256, "size": 16, "symbol": 0}
                    ],
                }
            ],
        }
        thread = {
            "frameTable": {"func": [0], "address": [256]},
            "funcTable": {"name": [0], "resource": [0]},
            "resourceTable": {"lib": [0]},
            "stringArray": ["0x100"],
        }
        resolver = PROFILE.SymbolResolver(profile, sidecar)
        self.assertEqual(resolver.frame_name(thread, 0), "resolved_symbol")


if __name__ == "__main__":
    unittest.main()
