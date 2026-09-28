#!/usr/bin/env python3
"""Owned-process tests use a descendant-held socket as the lifetime witness."""

import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import unittest

SCRIPT_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPT_DIR))
from measurement_process import run_process


DESCENDANT = """
import os, signal, sys
fd = int(sys.argv[1])
signal.signal(signal.SIGTERM, signal.SIG_IGN)
os.write(fd, b'ready')
while True:
    signal.pause()
"""
PARENT = """
import os, signal, subprocess, sys
fd = int(sys.argv[1])
subprocess.Popen([sys.executable, '-c', sys.argv[2], str(fd)], pass_fds=(fd,),
                 stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
os.close(fd)
print('parent started', flush=True)
if sys.argv[3] == 'hang':
    while True:
        signal.pause()
"""
SUPERVISOR = """
import json, subprocess, sys
sys.path.insert(0, sys.argv[1])
from measurement_process import run_process
run_process(json.loads(sys.argv[2]), timeout=30, pass_fds=(int(sys.argv[3]),),
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
"""


@unittest.skipUnless(os.name == "posix", "POSIX process ownership")
class MeasurementProcessTest(unittest.TestCase):
    def test_output_and_exit_status_are_preserved(self):
        command = [sys.executable, "-c", "print('finished'); raise SystemExit(7)"]
        result = run_process(command, timeout=5, stdout=subprocess.PIPE, text=True)
        self.assertEqual((result.returncode, result.stdout), (7, "finished\n"))
        with self.assertRaises(subprocess.CalledProcessError):
            run_process(command, timeout=5, stdout=subprocess.PIPE, check=True)

    def test_timeout_kills_descendants_and_preserves_output(self):
        with self.socket_witness() as witness:
            observer, child = witness
            command = self.parent_command(child, "hang")
            with self.assertRaises(subprocess.TimeoutExpired) as raised:
                run_process(command, timeout=2, pass_fds=(child.fileno(),),
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            child.close()
            self.assertEqual(observer.recv(5), b"ready")
            self.assertEqual(observer.recv(1), b"")
            self.assertIn("parent started", raised.exception.stdout)

    def test_success_also_cleans_a_descendant_after_leader_exit(self):
        with self.socket_witness() as (observer, child):
            # Block the leader on a handshake until the descendant is alive.
            # Its socket stays open only in the descendant after leader exit.
            parent = PARENT.replace("print('parent started', flush=True)",
                                    "os.read(int(sys.argv[4]), 1)")
            read_fd, write_fd = os.pipe()
            command = [sys.executable, "-c", parent, str(child.fileno()), DESCENDANT,
                       "exit", str(read_fd)]
            import threading
            observed = []
            def release():
                observed.append(observer.recv(5))
                os.write(write_fd, b"go")
            thread = threading.Thread(target=release)
            thread.start()
            try:
                result = run_process(command, timeout=5, pass_fds=(child.fileno(), read_fd),
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                self.assertEqual(result.returncode, 0)
                child.close()
                self.assertEqual(observer.recv(1), b"")
            finally:
                os.close(read_fd)
                os.close(write_fd)
                thread.join(timeout=5)
            self.assertFalse(thread.is_alive())
            self.assertEqual(observed, [b"ready"])

    def test_exited_leader_with_descendant_held_pipe_still_times_out(self):
        with self.socket_witness() as (observer, child):
            parent = PARENT.replace("stdout=subprocess.DEVNULL", "stdout=None")
            command = [sys.executable, "-c", parent, str(child.fileno()), DESCENDANT, "exit"]
            with self.assertRaises(subprocess.TimeoutExpired):
                run_process(command, timeout=2, pass_fds=(child.fileno(),),
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            child.close()
            self.assertEqual(observer.recv(5), b"ready")
            self.assertEqual(observer.recv(1), b"")

    def test_interrupts_clean_the_owned_group(self):
        for signum in (signal.SIGINT, signal.SIGTERM):
            with self.subTest(signal=signum), self.socket_witness() as (observer, child):
                command = self.parent_command(child, "hang")
                supervisor = subprocess.Popen(
                    [sys.executable, "-c", SUPERVISOR, str(SCRIPT_DIR),
                     json.dumps(command), str(child.fileno())],
                    pass_fds=(child.fileno(),), stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                )
                child.close()
                try:
                    self.assertEqual(observer.recv(5), b"ready")
                    supervisor.send_signal(signum)
                    supervisor.communicate(timeout=5)
                    self.assertNotEqual(supervisor.returncode, 0)
                    self.assertEqual(observer.recv(1), b"")
                finally:
                    if supervisor.poll() is None:
                        supervisor.kill()
                        supervisor.communicate(timeout=5)

    @staticmethod
    def parent_command(child, mode):
        return [sys.executable, "-c", PARENT, str(child.fileno()), DESCENDANT, mode]

    @staticmethod
    def socket_witness():
        from contextlib import contextmanager
        @contextmanager
        def witness():
            observer, child = socket.socketpair()
            observer.settimeout(5)
            try:
                yield observer, child
            finally:
                observer.close()
                child.close()
        return witness()


if __name__ == "__main__":
    unittest.main()
