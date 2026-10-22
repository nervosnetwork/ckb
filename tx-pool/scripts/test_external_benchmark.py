import copy
import os
import signal
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from external_benchmark import capture, external_build, stop_node, validate_shutdown, validate_usage


class CaptureQualificationTests(unittest.TestCase):
    def test_capture_cannot_write_into_its_frozen_inputs(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary).resolve()
            config = {name: str(directory / name) for name in
                      ("root", "bench", "bench_build", "node", "node_build", "miner", "miner_build", "seed", "corpus")}
            config.update(workers=8, concurrency=100, poll_ms=5, mine_ms=200, timeout_s=60, rpc_port=35110)
            for name in ("root", "seed"):
                output = Path(config[name]) / "capture"
                with self.assertRaises(ValueError):
                    capture(config, output)
                self.assertFalse(output.exists())

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


if __name__ == "__main__":
    unittest.main()
