import copy
import json
import os
import signal
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from external_benchmark import (capture, configure_seed, external_build, finish_capture, stop_node,
                                validate_config, validate_cpu_affinity, validate_node_logs,
                                validate_shutdown, validate_usage)


def capture_config(directory):
    config = {name: str(directory / name) for name in
              ("root", "bench", "bench_build", "node", "node_build", "miner", "miner_build", "seed", "corpus")}
    return config | {"workers": 8, "concurrency": 100, "poll_ms": 5, "mine_ms": 200,
                     "timeout_s": 60, "rpc_port": 35110}


class CaptureQualificationTests(unittest.TestCase):
    def test_optional_arrival_rate_is_positive_integral_and_bounded(self):
        config = capture_config(Path("/tmp"))
        validate_config(config)
        for rate in (1, 1500, 65536):
            validate_config(config | {"arrival_rate": rate})
        for rate in (None, True, 0, -1, 100.0, "100", 65537):
            with self.subTest(rate=rate), self.assertRaises(ValueError):
                validate_config(config | {"arrival_rate": rate})

    def test_cpu_isolation_requires_disjoint_complete_explicit_budgets(self):
        config = capture_config(Path("/tmp"))
        cpus = {"node_cpus": [0, 1, 2, 3], "control_cpus": [4, 5]}
        validate_config(config | cpus)
        for value in (None, [], [True], [0, 0], [1, 0], [-1], [0.0], "0,1"):
            for name in cpus:
                with self.subTest(name=name, value=value), self.assertRaises(ValueError):
                    validate_config(config | cpus | {name: value})
        for invalid in ({"node_cpus": [0]}, {"control_cpus": [1]},
                        {"node_cpus": [0, 1], "control_cpus": [1, 2]}):
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                validate_config(config | invalid)

    def test_seed_cannot_redirect_node_data_outside_its_owned_copy(self):
        with tempfile.TemporaryDirectory() as temporary:
            node = Path(temporary)
            for value in ("/tmp/shared-node", "../other-node", "alternate"):
                (node / "ckb.toml").write_text(f'data_dir = "{value}"\n')
                with self.assertRaisesRegex(ValueError, "owned relative data_dir"):
                    configure_seed(node, {})

    def test_capture_cannot_write_into_its_frozen_inputs(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary).resolve()
            config = capture_config(directory)
            for name in ("root", "seed"):
                output = Path(config[name]) / "capture"
                with self.assertRaises(ValueError):
                    capture(config, output)
                self.assertFalse(output.exists())

    def test_cpu_replay_rejects_wrong_ownership_or_budget_without_reading_host_affinity(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary)
            config = {"node_cpus": [0, 1], "control_cpus": [2]}
            records = {"runner": {"pid": 100, "ppid": 99, "cpus": [2], "available": [0, 1, 2]},
                       "node": {"pid": 101, "ppid": 100, "cpus": [0, 1]},
                       "executor": {"pid": 102, "ppid": 100, "cpus": [2]},
                       "miner": {"pid": 103, "ppid": 100, "cpus": [2]}}
            (output / "processes.json").write_text(json.dumps([
                {"name": name, "pid": record["pid"]} for name, record in records.items() if name != "runner"]))
            for name, record in records.items():
                (output / f"{name}-affinity.json").write_text(json.dumps(record))
            with mock.patch("external_benchmark.os.sched_getaffinity", create=True,
                            side_effect=AssertionError("replay must not read this host")):
                validate_cpu_affinity(config, output)
                for name, field, value in (("node", "cpus", [2]), ("node", "pid", 999),
                                           ("executor", "cpus", [0, 1]), ("miner", "ppid", 99),
                                           ("runner", "cpus", [0]), ("runner", "available", [2])):
                    path = output / f"{name}-affinity.json"
                    path.write_text(json.dumps(records[name] | {field: value}))
                    with self.subTest(name=name, field=field), self.assertRaises(ValueError):
                        validate_cpu_affinity(config, output)
                    path.write_text(json.dumps(records[name]))
                (output / "miner-affinity.json").unlink()
                with self.assertRaises(FileNotFoundError):
                    validate_cpu_affinity(config, output)
            # Older captures and the default mode have no affinity observations.
            validate_cpu_affinity({}, output)

    def test_external_receipts_bind_clean_production_sources_and_binary(self):
        binary = {"sha256": "a" * 64}
        node = {"sha256": binary["sha256"], "base": "b" * 40, "status": "", "patch": "",
                "command": ["cargo", "build", "--locked", "--profile", "prod"],
                "artifact": {"target": {"name": "ckb"}, "profile": {"debug_assertions": False}}}
        self.assertEqual(external_build(node, binary, node=True), node)
        for field, value in (("sha256", "c" * 64), ("status", " M lib.rs"),
                             ("patch", "changed"), ("command", ["cargo", "build"]), ("base", "unknown")):
            changed = copy.deepcopy(node)
            changed[field] = value
            with self.assertRaises(ValueError):
                external_build(changed, binary, node=True)
        miner = {"sha256": binary["sha256"], "source_sha": "b" * 40, "patch": "documented miner change",
                 "command": ["cargo", "build", "--locked"]}
        self.assertEqual(external_build(miner, binary, node=False), miner)
        del miner["patch"]
        with self.assertRaises(ValueError):
            external_build(miner, binary, node=False)

    def test_abnormal_exit_and_lost_usage_cannot_qualify_a_capture(self):
        usage = {"returncode": 0, "forced_kill": False, "user_cpu_ns": 1, "system_cpu_ns": 0,
                 "peak_rss_bytes": 1024, "scope": "whole_node_lifetime_including_startup_drain_and_shutdown"}
        validate_usage(usage)
        for field, value in (("returncode", 1), ("forced_kill", True), ("user_cpu_ns", -1),
                             ("system_cpu_ns", None), ("peak_rss_bytes", 0), ("scope", "submission_only")):
            with self.assertRaises(ValueError):
                validate_usage({**usage, field: value})
        stopped = {"executor": {"returncode": 0, "forced_kill": False},
                   "miner": {"returncode": -signal.SIGINT, "forced_kill": False}}
        validate_shutdown(stopped)
        for name, field, value in (("executor", "returncode", 1), ("miner", "returncode", -signal.SIGKILL),
                                   ("miner", "forced_kill", True)):
            changed = copy.deepcopy(stopped)
            changed[name][field] = value
            with self.assertRaises(ValueError):
                validate_shutdown(changed)


class CaptureLogRetentionTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.output = Path(temporary.name)
        (self.output / "ckb.toml").write_text('[logger]\nlog_to_file = true\n')
        self.node = self.output / "node-data"
        logs = self.node / "data/logs"
        logs.mkdir(parents=True)
        (logs / "run.log").write_text("complete node diagnostics\n")

    def test_archives_actual_data_logs_before_disposal_and_binds_their_hashes(self):
        receipt = {"state": "complete"}
        finish_capture(self.output, receipt)
        self.assertFalse(self.node.exists())
        self.assertEqual((self.output / "node-logs/run.log").read_text(), "complete node diagnostics\n")
        self.assertIn("node-logs/run.log", receipt["artifacts"])
        self.assertEqual(json.loads((self.output / "receipt.json").read_text()), receipt)
        validate_node_logs(self.output)
        (self.output / "node-logs/run.log").unlink()
        with self.assertRaisesRegex(ValueError, "file log was not retained"):
            validate_node_logs(self.output)

    def test_failed_workload_still_archives_diagnostics_and_retains_original_error(self):
        receipt = {"state": "failed", "error": "original executor deadline"}
        finish_capture(self.output, receipt)
        self.assertEqual(receipt["error"], "original executor deadline")
        self.assertIn("node-logs/run.log", receipt["artifacts"])
        self.assertFalse(self.node.exists())

    def test_missing_file_log_fails_capture_and_keeps_the_source_directory(self):
        (self.node / "data/logs/run.log").unlink()
        receipt = {"state": "complete"}
        with self.assertRaisesRegex(ValueError, "file log was not retained"):
            finish_capture(self.output, receipt)
        self.assertTrue(self.node.exists())
        self.assertEqual(receipt["state"], "failed")
        self.assertIn("log_retention_error", receipt)

    def test_archive_failure_or_cancellation_keeps_the_complete_node_copy(self):
        for failure in (OSError("copy failed"), KeyboardInterrupt()):
            with self.subTest(failure=type(failure).__name__):
                receipt = {"state": "failed", "error": "original workload failure"}
                with mock.patch("external_benchmark.shutil.copytree", side_effect=failure):
                    with self.assertRaises(type(failure)):
                        finish_capture(self.output, receipt)
                self.assertEqual(receipt["state"], "failed")
                self.assertEqual(receipt["error"], "original workload failure")
                self.assertTrue((self.node / "data/logs/run.log").is_file())
                self.assertIn("log_retention_error", receipt)

    def test_stdout_only_configuration_does_not_require_a_file_logger(self):
        (self.output / "ckb.toml").write_text('[logger]\nlog_to_stdout = true\n')
        validate_node_logs(self.output)


@unittest.skipUnless(os.name == "posix", "node resource ownership uses POSIX wait4")
class NodeUsageTests(unittest.TestCase):
    def test_reaps_early_exit_without_losing_resource_usage(self):
        process = subprocess.Popen([sys.executable, "-c", "sum(i*i for i in range(100000)); print('complete')"],
                                   stdout=subprocess.PIPE)
        # EOF is an observable exit boundary; Popen.poll/wait would consume the
        # usage before the production collector can bind it to this node.
        self.assertEqual(process.stdout.read(), b"complete\n")
        usage = stop_node(process)
        process.stdout.close()
        self.assertEqual(usage["returncode"], 0)
        self.assertFalse(usage["forced_kill"])
        self.assertGreater(usage["user_cpu_ns"], 0)

    def test_graceful_stop_retains_the_same_scope(self):
        code = "import signal,sys,time; signal.signal(signal.SIGINT, lambda *_: sys.exit(0)); print('ready',flush=True); time.sleep(60)"
        process = subprocess.Popen([sys.executable, "-c", code], stdout=subprocess.PIPE)
        self.assertEqual(process.stdout.readline(), b"ready\n")
        usage = stop_node(process)
        process.stdout.close()
        self.assertEqual(usage["returncode"], 0)
        self.assertFalse(usage["forced_kill"])
        self.assertGreater(usage["peak_rss_bytes"], 0)
        self.assertEqual(usage["scope"], "whole_node_lifetime_including_startup_drain_and_shutdown")


@unittest.skipUnless(hasattr(os, "sched_getaffinity"), "requires Linux process affinity")
class CpuIsolationTests(unittest.TestCase):
    def test_exec_preserves_pid_and_budgets_before_threads_and_children_start(self):
        cpus = sorted(os.sched_getaffinity(0))
        if len(cpus) < 2:
            self.skipTest("requires two available CPUs")
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary)
            config = {"node_cpus": cpus[:1], "control_cpus": cpus[1:2]}
            (output / "config.json").write_text(json.dumps(config))
            code = """import json,os,subprocess,sys,threading
record = {'pid': os.getpid(), 'cpus': sorted(os.sched_getaffinity(0))}
thread = threading.Thread(target=lambda: record.update(thread_cpus=sorted(os.sched_getaffinity(0))))
thread.start()
thread.join()
record['child_cpus'] = json.loads(subprocess.check_output([sys.executable, '-c',
    'import os,json; print(json.dumps(sorted(os.sched_getaffinity(0))))']))
print(json.dumps(record))
"""
            script = str(Path(__file__).with_name("external_benchmark.py"))
            # The node must be able to take its own budget after the owned runner
            # has restricted itself to the disjoint control CPUs.
            launcher = "import os,sys; os.sched_setaffinity(0," + repr(config["control_cpus"]) + "); os.execv(sys.executable,sys.argv[1:])"
            for name in ("node", "executor", "miner"):
                command = [sys.executable, "-c", launcher, sys.executable, script,
                           "__exec__", str(output), name, sys.executable, "-c", code]
                with subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE) as process:
                    stdout, stderr = process.communicate(timeout=10)
                    self.assertEqual(process.returncode, 0, stderr.decode())
                    actual = json.loads(stdout)
                    recorded = json.loads((output / f"{name}-affinity.json").read_text())
                    expected = config["node_cpus" if name == "node" else "control_cpus"]
                    self.assertEqual(actual, {"pid": process.pid, "cpus": expected,
                                              "thread_cpus": expected, "child_cpus": expected})
                    self.assertEqual(recorded, {"pid": process.pid, "ppid": os.getpid(), "cpus": expected})

    def test_unavailable_cpu_fails_before_starting_children(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary)
            cpus = sorted(os.sched_getaffinity(0))
            config = capture_config(output) | {"node_cpus": [max(cpus) + 1], "control_cpus": cpus[:1]}
            (output / "config.json").write_text(json.dumps(config))
            result = subprocess.run([sys.executable, str(Path(__file__).with_name("external_benchmark.py")),
                                     "__capture__", str(output)], start_new_session=True,
                                    capture_output=True, timeout=10)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(b"outside the runner's available affinity", result.stderr)
            self.assertFalse((output / "processes.json").exists())
            self.assertFalse((output / "runner-affinity.json").exists())


if __name__ == "__main__":
    unittest.main()
