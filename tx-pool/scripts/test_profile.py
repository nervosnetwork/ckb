#!/usr/bin/env python3
"""Artifact-integrity and window-cropping canaries for ``profile.py``."""

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

    @staticmethod
    def write_json(path: Path, value: object) -> None:
        path.write_text(json.dumps(value, sort_keys=True) + "\n")

    @staticmethod
    def scenario(**changes: object) -> dict[str, object]:
        value: dict[str, object] = {
            "scenario": "always_success",
            "target": 1,
            "warm": 0,
            "workers": 1,
            "peers": 1,
        }
        value.update(changes)
        return value

    @staticmethod
    def observation(
        scenario: dict[str, object], **changes: object
    ) -> dict[str, object]:
        accepted = int(scenario["target"]) + int(scenario["warm"])
        value: dict[str, object] = {
            "schema_version": PROFILE.OBSERVATION_SCHEMA_VERSION,
            **scenario,
            "elapsed_nanos": 1,
            "throughput_tps": 1.0,
            "accepted": accepted,
            "callback_duplicates": 0,
            "p99_latency_nanos": 1,
            "target_cpu_nanos": 1,
            "target_user_cpu_nanos": 1,
            "target_system_cpu_nanos": 0,
            "allocation_calls": 0,
            "allocated_bytes": 0,
            "reorg_latency_nanos": 1,
            "reorg_overlap_callbacks": 0,
            "relay_ok": accepted,
            "relay_duplicate_ok": 0,
            "relay_rejects": int(scenario["warm"])
            if scenario["scenario"] == "rbf_pairs"
            else 0,
            "relay_unknown_parents": 0,
            "relay_unknown_parent_observations": [],
            "relay_generation_resets": 0,
            "shutdown_latency_nanos": 1,
        }
        value.update(changes)
        return value

    @staticmethod
    def output(window: dict[str, object], observation: dict[str, object]) -> str:
        return (
            f"{PROFILE.MARKER_PREFIX}{json.dumps(window, sort_keys=True)}\n"
            f"{PROFILE.OBSERVATION_PREFIX}{json.dumps(observation, sort_keys=True)}\n"
        )

    def bundle(
        self,
        name: str,
        *,
        absolute_time: bool,
        coordinates: list[float] | None = None,
    ) -> Path:
        bundle = self.root / name
        bundle.mkdir()
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
                    "samples": {
                        "length": 3,
                        time_field: coordinates
                        or ([1.0, 2.0, 3.0] if absolute_time else [1.0, 1.0, 1.0]),
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
        window = {
            "schema_version": PROFILE.PROFILE_SCHEMA_VERSION,
            "scenario": "always_success",
            "start_unix_nanos": 1_001_000_000,
            "end_unix_nanos": 1_003_000_000,
            "elapsed_nanos": 2_000_000,
        }
        scenario = self.scenario()
        observation = self.observation(scenario)
        output = self.output(window, observation)
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
        values = {
            "profile.json": profile,
            "symbols.json": {"string_table": [], "data": []},
            "stdout.log": output,
            "stderr.log": "",
            "spans.json": spans,
            "span.stdout.log": output,
            "span.stderr.log": "",
        }
        for filename, value in values.items():
            path = bundle / filename
            path.write_text(value) if isinstance(value, str) else self.write_json(path, value)
        files = {
            "profile": "profile.json",
            "symbols": "symbols.json",
            "stdout": "stdout.log",
            "stderr": "stderr.log",
            "spans": "spans.json",
            "span_stdout": "span.stdout.log",
            "span_stderr": "span.stderr.log",
        }
        sources = [PROFILE.ONE_SHOT_SOURCE, PROFILE.SCRIPT_SOURCE]
        manifest = {
            "schema_version": PROFILE.MANIFEST_SCHEMA_VERSION,
            "harness": "profile_one_shot",
            "features": list(PROFILE.ONE_SHOT_FEATURES),
            "scenario": scenario,
            "observation": observation,
            "window": window,
            "capture": {"sample_rate_hz": 1_000},
            "span_capture": {"window": window},
            "inputs": {
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
            "artifacts": {
                label: PROFILE.artifact(bundle / filename, bundle)
                for label, filename in files.items()
            },
            "summary_path": "summary.json",
        }
        manifest_path = bundle / "manifest.json"
        self.write_json(manifest_path, manifest)
        return manifest_path

    def test_prod_binary_contract_and_build_command(self) -> None:
        self.assertEqual(PROFILE.require_final_build_profile("prod"), "prod")
        with self.assertRaisesRegex(ValueError, "explicit prod"):
            PROFILE.require_final_build_profile("bench")
        executable = self.root / "target" / "prod" / "profile_one_shot"
        executable.parent.mkdir(parents=True)
        executable.write_bytes(b"binary")
        message = {
            "reason": "compiler-artifact",
            "target": {"name": "profile_one_shot", "kind": ["bench"]},
            "executable": str(executable),
        }
        completed = mock.Mock(stdout=json.dumps(message), stderr="")
        with mock.patch.object(PROFILE.subprocess, "run", return_value=completed):
            binary, command, _ = PROFILE.build_binary(
                self.root / "target", "profile_one_shot", ("profiling",)
            )
        self.assertEqual(binary, executable.resolve())
        self.assertEqual(command[command.index("--profile") + 1], "prod")

    def test_absolute_and_delta_coordinates_have_identical_hotspots(self) -> None:
        absolute = PROFILE.read_json(
            PROFILE.analyze_manifest(self.bundle("absolute", absolute_time=True))
        )
        delta = PROFILE.read_json(
            PROFILE.analyze_manifest(self.bundle("delta", absolute_time=False))
        )
        for key in ("sampling", "leaf_hotspots", "inclusive_hotspots"):
            self.assertEqual(absolute[key], delta[key])
        self.assertEqual(
            absolute["leaf_hotspots"],
            [{"symbol": "synthetic_leaf", "thread_cpu_delta_micros": 50.0}],
        )
        self.assertEqual(absolute["span_capture"]["total_elapsed_nanos"], 1_000)

    def test_bundle_moves_without_its_capture_binary(self) -> None:
        manifest = self.bundle("portable", absolute_time=True)
        moved = self.root / "moved"
        shutil.move(str(manifest.parent), moved)
        self.assertTrue(PROFILE.analyze_manifest(moved / "manifest.json").is_file())

    def test_artifact_tampering_is_rejected(self) -> None:
        manifest = self.bundle("tamper", absolute_time=True)
        (manifest.parent / "profile.json").write_text("{}\n")
        with self.assertRaisesRegex(PROFILE.ProfileError, "(size|hash) changed"):
            PROFILE.analyze_manifest(manifest)

    def test_invalid_window_coordinates_are_rejected(self) -> None:
        manifest = self.bundle(
            "nonmonotonic", absolute_time=True, coordinates=[1.0, 0.5, 3.0]
        )
        with self.assertRaisesRegex(PROFILE.ProfileError, "not monotonic"):
            PROFILE.analyze_manifest(manifest)

    def test_remote_batch_span_is_required(self) -> None:
        manifest_path = self.bundle("sequential", absolute_time=True)
        spans_path = manifest_path.parent / "spans.json"
        spans = PROFILE.read_json(spans_path)
        spans["spans"] = spans["spans"][:1]
        self.write_json(spans_path, spans)
        manifest = PROFILE.read_json(manifest_path)
        manifest["artifacts"]["spans"] = PROFILE.artifact(
            spans_path, manifest_path.parent
        )
        self.write_json(manifest_path, manifest)
        with self.assertRaisesRegex(PROFILE.ProfileError, "remote-batch ingress"):
            PROFILE.analyze_manifest(manifest_path)

    def test_observation_binds_exact_terminals_and_reverse_evidence(self) -> None:
        rbf = self.scenario(scenario="rbf_pairs", target=4, warm=2, workers=2, peers=2)
        observation = self.observation(rbf)
        stdout = f"{PROFILE.OBSERVATION_PREFIX}{json.dumps(observation)}\n"
        self.assertEqual(PROFILE.parse_observation(stdout, rbf), observation)
        broken = {**observation, "relay_rejects": 0}
        with self.assertRaisesRegex(PROFILE.ProfileError, "unexpected reject"):
            PROFILE.parse_observation(
                f"{PROFILE.OBSERVATION_PREFIX}{json.dumps(broken)}\n", rbf
            )

        reverse = self.scenario(
            scenario="dependent_forest_8_reverse", target=4, workers=2, peers=2
        )
        observation = self.observation(
            reverse,
            relay_unknown_parents=2,
            relay_unknown_parent_observations=[
                {"peer": 1, "parents": ["00" * 32], "count": 2}
            ],
        )
        stdout = f"{PROFILE.OBSERVATION_PREFIX}{json.dumps(observation)}\n"
        self.assertEqual(PROFILE.parse_observation(stdout, reverse), observation)
        broken = {**observation, "relay_unknown_parents": 1}
        with self.assertRaisesRegex(PROFILE.ProfileError, "does not match"):
            PROFILE.parse_observation(
                f"{PROFILE.OBSERVATION_PREFIX}{json.dumps(broken)}\n", reverse
            )

    def test_sidecar_resolves_address_frame(self) -> None:
        profile = {"libs": [{"name": "synthetic", "codeId": "code"}]}
        sidecar = {
            "string_table": ["resolved_symbol"],
            "data": [
                {
                    "code_id": "code",
                    "known_addresses": [[256, 0]],
                    "symbol_table": [{"rva": 256, "size": 16, "symbol": 0}],
                }
            ],
        }
        thread = {
            "frameTable": {"func": [0], "address": [256]},
            "funcTable": {"name": [0], "resource": [0]},
            "resourceTable": {"lib": [0]},
            "stringArray": ["0x100"],
        }
        self.assertEqual(
            PROFILE.SymbolResolver(profile, sidecar).frame_name(thread, 0),
            "resolved_symbol",
        )


if __name__ == "__main__":
    unittest.main()
