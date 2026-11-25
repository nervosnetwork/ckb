"""Bound the lifetime of an owned POSIX measurement process and its children."""

from __future__ import annotations

import math
import os
import signal
import subprocess
from contextlib import contextmanager
from threading import current_thread, main_thread


@contextmanager
def terminate_as_exception():
    """Let the normal cleanup path handle SIGTERM in command-line runners."""
    def terminate(signum, _frame):
        raise SystemExit(128 + signum)

    previous = None
    if current_thread() is main_thread():
        previous = signal.signal(signal.SIGTERM, terminate)
    try:
        yield
    finally:
        if previous is not None:
            signal.signal(signal.SIGTERM, previous)


def run_process(command, *, timeout: float, check: bool = False, **kwargs):
    """Run in a new session; clean its group on success, failure or interruption.

    Measurement commands must keep descendants in the inherited process group.
    A failed or interrupted capture is not a valid artifact; kill immediately
    instead of allowing unmeasured work to contaminate the next attempt.
    """
    if os.name != "posix":
        raise RuntimeError("measurement process ownership requires POSIX")
    if not math.isfinite(timeout) or timeout <= 0:
        raise ValueError("process timeout must be positive")
    with terminate_as_exception():
        return _run_process(command, timeout=timeout, check=check, **kwargs)


def _run_process(command, *, timeout: float, check: bool, **kwargs):
    process = subprocess.Popen(command, start_new_session=True, **kwargs)
    try:
        stdout, stderr = process.communicate(timeout=timeout)
    except BaseException as error:
        # The leader may already have exited while a descendant holds a pipe.
        # Kill by the session's initial process group, not by leader liveness.
        kill_group(process.pid)
        try:
            stdout, stderr = process.communicate(timeout=5)
        except subprocess.TimeoutExpired:
            # Do not hang on a pipe held by a command that violated ownership.
            for stream in (process.stdin, process.stdout, process.stderr):
                if stream is not None:
                    stream.close()
            process.wait(timeout=5)
        else:
            # Preserve partial logs for the caller even on Ctrl-C/SIGTERM.
            error.output, error.stderr = stdout, stderr
        raise
    finally:
        kill_group(process.pid)
        process.wait(timeout=5)
    result = subprocess.CompletedProcess(command, process.returncode, stdout, stderr)
    if check:
        result.check_returncode()
    return result


def kill_group(group: int) -> None:
    try:
        os.killpg(group, signal.SIGKILL)
    except ProcessLookupError:
        pass
