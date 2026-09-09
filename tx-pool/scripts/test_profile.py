#!/usr/bin/env python3
"""Artifact-integrity and window-cropping canaries for ``profile.py``."""

from __future__ import annotations

import importlib.util
import argparse
import json
import shutil
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock


SCRIPT = Path(__file__).resolve().with_name("profile.py")
sys.path.insert(0, str(SCRIPT.parent))
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
            "elapsed_nanos": 1_000_000_000,
            "throughput_tps": float(scenario["target"]),
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
        observation = self.observation(scenario, elapsed_nanos=window["elapsed_nanos"],
                                       throughput_tps=scenario["target"] * 1e9 / window["elapsed_nanos"])
        output = self.output(window, observation)
        spans = {
            "schema_version": 3,
            "instrumentation": "authority_v1",
            "measurement": "span_lifetimes_started_during_target_work",
            "window": window,
            "spans": [
                {
                    "name": "tx_pool.authority.apply",
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
        spans["spans"].extend(
            {"name": name, "start_count": 1, "elapsed_nanos": 0}
            for name in ("tx_pool.authority.capture", "tx_pool.effects.publish",
                         "tx_pool.membership.admission", "tx_pool.stage.resolve", "tx_pool.stage.verify")
        )
        spans["spans"].sort(key=lambda span: span["name"])
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
        sources = [PROFILE.ONE_SHOT_SOURCE, PROFILE.SPAN_SOURCE, PROFILE.SCRIPT_SOURCE, PROFILE.PROCESS_SOURCE]
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
        with mock.patch.object(PROFILE, "run_process", return_value=completed):
            binary, command, _ = PROFILE.build_binary(
                self.root / "target", "profile_one_shot", ("profiling",)
            )
        self.assertEqual(binary, executable.resolve())
        self.assertEqual(command[command.index("--profile") + 1], "prod")

    def test_source_drift_during_build_stops_before_capture(self) -> None:
        args = argparse.Namespace(**self.scenario(), output_prefix=self.root / "source-drift",
                                  force=False, binary=None, target_dir=self.root / "target")
        with mock.patch.object(PROFILE, "capture_source_identity", side_effect=[{"git": "before"}, {"git": "after"}]), mock.patch.object(
            PROFILE, "build_binary", return_value=(self.root / "binary", [], {})
        ), mock.patch.object(PROFILE, "run") as run:
            with self.assertRaisesRegex(PROFILE.ProfileError, "source changed during build"):
                PROFILE.capture(args)
        run.assert_not_called()

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
            [{"symbol": "synthetic_leaf", "estimated_cpu_weight_micros": 50.0}],
        )
        self.assertEqual(absolute["span_capture"]["total_elapsed_nanos"], 1_000)
        self.assertEqual(absolute["sampling"]["complete_intervals"], 2)
        self.assertEqual(absolute["sampling"]["positive_cpu_intervals"], 2)
        self.assertEqual(absolute["sampling"]["unattributed_cpu_weight_micros"], 0)

    def test_missing_stacks_preserve_unattributed_cpu_weight(self) -> None:
        manifest_path = self.bundle("missing-stack", absolute_time=True)
        profile = PROFILE.read_json(manifest_path.parent / "profile.json")
        profile["threads"][0]["samples"]["stack"] = [None, None, None]
        manifest = PROFILE.read_json(manifest_path)
        resolver = PROFILE.SymbolResolver(profile, PROFILE.read_json(manifest_path.parent / "symbols.json"))
        result = PROFILE.analyze_samples(profile, resolver, manifest["window"])
        self.assertEqual(result["leaf_hotspots"], [])
        self.assertEqual(result["sampling"]["complete_interval_cpu_micros"], 50)
        self.assertEqual(result["sampling"]["unattributed_cpu_weight_micros"], 50)

    def test_null_cpu_reads_are_missing_and_nanoseconds_are_normalized(self) -> None:
        manifest_path = self.bundle("missing-cpu", absolute_time=True)
        profile = PROFILE.read_json(manifest_path.parent / "profile.json")
        profile["meta"]["sampleUnits"]["threadCPUDelta"] = "ns"
        profile["threads"][0]["samples"]["threadCPUDelta"] = [None, None, 30_000]
        resolver = PROFILE.SymbolResolver(profile, PROFILE.read_json(manifest_path.parent / "symbols.json"))
        result = PROFILE.analyze_samples(profile, resolver, PROFILE.read_json(manifest_path)["window"])
        self.assertEqual(result["sampling"]["complete_intervals"], 2)
        self.assertEqual(result["sampling"]["missing_cpu_intervals"], 1)
        self.assertEqual(result["sampling"]["observed_cpu_interval_fraction"], 0.5)
        self.assertEqual(result["sampling"]["complete_interval_cpu_micros"], 30)
        self.assertEqual(result["leaf_hotspots"][0]["estimated_cpu_weight_micros"], 30)

    def test_monotonic_and_profile_windows_must_agree(self) -> None:
        observation = self.observation(self.scenario())
        PROFILE.validate_window_observation(dict(scenario="always_success", elapsed_nanos=1_000_000_100), observation)
        with self.assertRaisesRegex(PROFILE.ProfileError, "wall window differs"):
            PROFILE.validate_window_observation(dict(scenario="always_success", elapsed_nanos=1_010_000_000), observation)
        invalid = self.observation(self.scenario(), throughput_tps=2.0)
        with self.assertRaisesRegex(PROFILE.ProfileError, "throughput differs"):
            PROFILE.parse_observation(f"{PROFILE.OBSERVATION_PREFIX}{json.dumps(invalid)}\n", self.scenario())

    def test_timeout_preserves_failed_capture_logs(self) -> None:
        paths = self.root / "stdout.log", self.root / "stderr.log"
        error = PROFILE.subprocess.TimeoutExpired(["owned-fixture"], 1, output=b"partial stdout", stderr=b"partial stderr")
        with mock.patch.object(PROFILE, "run_process", side_effect=error):
            with self.assertRaisesRegex(PROFILE.ProfileError, "exceeded"):
                PROFILE.run(["owned-fixture"], timeout=1, log_paths=paths)
        self.assertEqual([path.read_text() for path in paths], ["partial stdout", "partial stderr"])

    def test_bundle_moves_without_its_capture_binary(self) -> None:
        manifest = self.bundle("portable", absolute_time=True)
        moved = self.root / "moved"
        shutil.move(str(manifest.parent), moved)
        self.assertTrue(PROFILE.analyze_manifest(moved / "manifest.json").is_file())

    def test_nonfinite_sample_inputs_and_boolean_metadata_are_rejected(self) -> None:
        manifest_path = self.bundle("numeric-guards", absolute_time=True)
        manifest = PROFILE.read_json(manifest_path)
        for section, field, value in (("meta", "interval", True), ("meta", "startTime", float("nan")),
                                      ("samples", "time", float("inf")),
                                      ("samples", "threadCPUDelta", float("nan"))):
            with self.subTest(section=section, field=field):
                profile = PROFILE.read_json(manifest_path.parent / "profile.json")
                if section == "meta":
                    profile["meta"][field] = value
                else:
                    profile["threads"][0]["samples"][field][1] = value
                resolver = PROFILE.SymbolResolver(profile, PROFILE.read_json(manifest_path.parent / "symbols.json"))
                with self.assertRaises(PROFILE.ProfileError):
                    PROFILE.analyze_samples(profile, resolver, manifest["window"])

    def test_zero_start_spans_are_reported_as_unobserved(self) -> None:
        manifest_path = self.bundle("unobserved-span", absolute_time=True)
        spans_path = manifest_path.parent / "spans.json"
        spans = PROFILE.read_json(spans_path)
        next(span for span in spans["spans"] if span["name"] == "tx_pool.authority.capture").update(start_count=0, elapsed_nanos=0)
        self.write_json(spans_path, spans)
        result = PROFILE.analyze_spans(PROFILE.read_json(manifest_path), spans_path)
        self.assertEqual(result["unobserved_span_names"], ["tx_pool.authority.capture"])

    def test_current_authority_coverage_and_instrumentation_are_required(self) -> None:
        manifest_path = self.bundle("current-spans", absolute_time=True)
        spans_path = manifest_path.parent / "spans.json"
        original = PROFILE.read_json(spans_path)
        for field, value in (("schema_version", 2), ("instrumentation", "retired")):
            with self.subTest(field=field):
                self.write_json(spans_path, {**original, field: value})
                with self.assertRaisesRegex(PROFILE.ProfileError, "schema"):
                    PROFILE.analyze_spans(PROFILE.read_json(manifest_path), spans_path)
        for name in ("tx_pool.authority.apply", "tx_pool.effects.publish",
                     "tx_pool.membership.admission", "tx_pool.stage.resolve", "tx_pool.stage.verify"):
            with self.subTest(name=name):
                modified = json.loads(json.dumps(original))
                next(span for span in modified["spans"] if span["name"] == name)["start_count"] = 0
                self.write_json(spans_path, modified)
                with self.assertRaisesRegex(PROFILE.ProfileError, "current-authority coverage"):
                    PROFILE.analyze_spans(PROFILE.read_json(manifest_path), spans_path)

    def test_entered_time_and_historical_lifetimes_remain_distinct(self) -> None:
        manifest_path = self.bundle("entered-time", absolute_time=True)
        manifest = PROFILE.read_json(manifest_path)
        path = manifest_path.parent / "spans.json"
        legacy = PROFILE.read_json(path)
        original = PROFILE.analyze_spans(manifest, path)
        self.assertIn("Historical", original["interpretation"])
        current = {**legacy, "schema_version": 4, "instrumentation": "authority_v2",
                   "measurement": "entered_scope_wall_time_within_capture_window",
                   "capture_window": {key: manifest["window"][key] for key in
                                      ("start_unix_nanos", "end_unix_nanos", "elapsed_nanos")},
                   "spans": [{**span, "enter_count": 2, "active_at_start": 0, "active_at_end": 0}
                             for span in legacy["spans"]]}
        # A span created before capture can be observed entirely through its
        # already-entered interval, without a creation or new enter in capture.
        current["spans"][0].update(start_count=0, enter_count=0, active_at_start=1, active_at_end=1)
        self.write_json(path, current)
        result = PROFILE.analyze_spans(manifest, path)
        self.assertEqual(result["unobserved_span_names"], [])
        self.assertIn("preexisting", result["interpretation"])
        self.assertEqual(result["capture_window"], current["capture_window"])
        for field, value in (("schema_version", 3), ("instrumentation", "authority_v1"),
                             ("measurement", legacy["measurement"])):
            with self.subTest(field=field):
                self.write_json(path, {**current, field: value})
                with self.assertRaisesRegex(PROFILE.ProfileError, "schema"):
                    PROFILE.analyze_spans(manifest, path)
        invalid = json.loads(json.dumps(current))
        invalid["spans"][0]["active_at_end"] = 2
        self.write_json(path, invalid)
        with self.assertRaisesRegex(PROFILE.ProfileError, "depth"):
            PROFILE.analyze_spans(manifest, path)
        invalid = json.loads(json.dumps(current))
        invalid["capture_window"]["elapsed_nanos"] += 1
        self.write_json(path, invalid)
        with self.assertRaisesRegex(PROFILE.ProfileError, "timestamps"):
            PROFILE.analyze_spans(manifest, path)

    def test_interrupt_retains_partial_logs_without_becoming_success(self) -> None:
        paths = self.root / "stdout.log", self.root / "stderr.log"
        error = KeyboardInterrupt()
        error.output, error.stderr = "partial", "interrupted"
        with mock.patch.object(PROFILE, "run_process", side_effect=error):
            with self.assertRaises(KeyboardInterrupt):
                PROFILE.run(["fixture"], log_paths=paths)
        self.assertEqual([path.read_text() for path in paths], ["partial", "interrupted"])

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

    def test_observation_binds_exact_terminals(self) -> None:
        rbf = self.scenario(scenario="rbf_pairs", target=4, warm=2, workers=2, peers=2)
        observation = self.observation(rbf)
        stdout = f"{PROFILE.OBSERVATION_PREFIX}{json.dumps(observation)}\n"
        self.assertEqual(PROFILE.parse_observation(stdout, rbf), observation)
        broken = {**observation, "relay_rejects": 0}
        with self.assertRaisesRegex(PROFILE.ProfileError, "unexpected reject"):
            PROFILE.parse_observation(
                f"{PROFILE.OBSERVATION_PREFIX}{json.dumps(broken)}\n", rbf
            )

    def test_reorg_reaccept_callbacks_preserve_other_terminal_guards(self) -> None:
        scenario = self.scenario(scenario="reorg_in_flight", target=2, warm=1)
        observation = self.observation(
            scenario, throughput_tps=2.0, callback_duplicates=1, reorg_overlap_callbacks=1
        )
        def parse(value, expected=scenario):
            return PROFILE.parse_observation(
                f"{PROFILE.OBSERVATION_PREFIX}{json.dumps(value)}\n", expected
            )
        self.assertEqual(parse(observation), observation)
        for field, value in (("relay_duplicate_ok", 1), ("relay_generation_resets", 1),
                             ("relay_rejects", 1), ("accepted", 2), ("relay_ok", 2),
                             ("reorg_overlap_callbacks", 0)):
            with self.subTest(field=field), self.assertRaises(PROFILE.ProfileError):
                parse({**observation, field: value})
        ordinary = {**scenario, "scenario": "always_success"}
        with self.assertRaises(PROFILE.ProfileError):
            parse({**observation, "scenario": "always_success", "reorg_overlap_callbacks": 0}, ordinary)

    def test_observation_binds_reverse_evidence(self) -> None:

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
