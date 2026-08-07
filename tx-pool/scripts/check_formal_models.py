#!/usr/bin/env python3
"""Run the bounded TLA+ falsifiers without writing into the repository."""

from __future__ import annotations

import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile


TX_POOL_ROOT = Path(__file__).resolve().parents[1]
FORMAL_ROOT = TX_POOL_ROOT / "formal"


def usable_java(candidate: Path) -> bool:
    try:
        return subprocess.run(
            [str(candidate), "-version"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=10,
            check=False,
        ).returncode == 0
    except (OSError, subprocess.TimeoutExpired):
        return False


def find_java() -> Path:
    configured = os.environ.get("JAVA")
    candidates = [Path(configured)] if configured else []
    discovered = shutil.which("java")
    if discovered:
        candidates.append(Path(discovered))
    brew = shutil.which("brew")
    if brew:
        result = subprocess.run(
            [brew, "--prefix", "openjdk@21"],
            capture_output=True,
            text=True,
            timeout=10,
            check=False,
        )
        if result.returncode == 0:
            candidates.append(Path(result.stdout.strip()) / "bin" / "java")
    for candidate in candidates:
        if usable_java(candidate):
            return candidate
    raise RuntimeError("Java 11+ is required; set JAVA to an executable path")


def find_tla_tools() -> Path:
    configured = os.environ.get("TLA2TOOLS_JAR")
    candidates = [Path(configured)] if configured else []
    candidates.append(Path.home() / ".local" / "share" / "tlaplus" / "tla2tools.jar")
    for candidate in candidates:
        if candidate.is_file():
            return candidate
    raise RuntimeError(
        "tla2tools.jar is required; set TLA2TOOLS_JAR or install it under "
        "$HOME/.local/share/tlaplus"
    )


def run_model(java: Path, tools: Path, config: str) -> subprocess.CompletedProcess[str]:
    with tempfile.TemporaryDirectory(prefix="ckb-tx-pool-tlc-") as metadata:
        command = [
            str(java),
            "-XX:+UseParallelGC",
            "-jar",
            str(tools),
            "-cleanup",
            "-workers",
            "1",
            "-metadir",
            metadata,
            "-config",
            config,
            "PermitEffect.tla",
        ]
        return subprocess.run(
            command,
            cwd=FORMAL_ROOT,
            capture_output=True,
            text=True,
            timeout=120,
            check=False,
        )


def main() -> int:
    try:
        java = find_java()
        tools = find_tla_tools()
    except (RuntimeError, subprocess.TimeoutExpired) as error:
        print(f"formal-model setup error: {error}", file=sys.stderr)
        return 2

    try:
        result = run_model(java, tools, "PermitEffect.cfg")
    except (OSError, subprocess.TimeoutExpired) as error:
        print(f"formal-model execution error: {error}", file=sys.stderr)
        return 2
    sys.stdout.write(result.stdout)
    sys.stderr.write(result.stderr)
    success = "Model checking completed. No error has been found."
    if result.returncode != 0 or success not in result.stdout:
        print(
            "formal-model failure: TLC did not emit its complete success summary",
            file=sys.stderr,
        )
        return result.returncode or 1

    try:
        reachability = run_model(java, tools, "PermitEffectReachability.cfg")
    except (OSError, subprocess.TimeoutExpired) as error:
        print(f"formal-model execution error: {error}", file=sys.stderr)
        return 2
    sys.stdout.write(reachability.stdout)
    sys.stderr.write(reachability.stderr)
    witness = "Invariant NoEffectBlockedDirectHandoff is violated."
    if reachability.returncode == 0 or witness not in reachability.stdout:
        print(
            "formal-model failure: TLC did not reach the required blocked/handoff witness",
            file=sys.stderr,
        )
        return reachability.returncode or 1
    print("TLC reachability falsifier found the required blocked/handoff witness.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
