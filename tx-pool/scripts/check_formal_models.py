#!/usr/bin/env python3
"""Discover, validate and run every registered bounded TLA+ falsifier."""

from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
from typing import Any


TX_POOL_ROOT = Path(__file__).resolve().parents[1]
FORMAL_ROOT = TX_POOL_ROOT / "formal"
MANIFEST = FORMAL_ROOT / "models.json"
SUCCESS_SENTINEL = "Model checking completed. No error has been found."


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


def relative_files(suffix: str) -> set[str]:
    return {
        path.relative_to(FORMAL_ROOT).as_posix()
        for path in FORMAL_ROOT.rglob(f"*{suffix}")
        if path.is_file()
    }


def load_runs() -> list[dict[str, str]]:
    try:
        manifest: Any = json.loads(MANIFEST.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise RuntimeError(f"cannot read {MANIFEST.relative_to(TX_POOL_ROOT)}: {error}") from error
    if not isinstance(manifest, dict) or manifest.get("schema") != 1:
        raise RuntimeError("formal/models.json must be an object with schema 1")
    runs = manifest.get("runs")
    if not isinstance(runs, list) or not runs:
        raise RuntimeError("formal/models.json must register at least one run")

    normalized: list[dict[str, str]] = []
    pairs: set[tuple[str, str]] = set()
    registered_modules: set[str] = set()
    registered_configs: set[str] = set()
    for index, raw in enumerate(runs):
        if not isinstance(raw, dict):
            raise RuntimeError(f"formal run {index} must be an object")
        allowed = {"module", "config", "expected", "invariant"}
        unknown = set(raw) - allowed
        if unknown:
            raise RuntimeError(f"formal run {index} has unknown fields: {sorted(unknown)}")
        module = raw.get("module")
        config = raw.get("config")
        expected = raw.get("expected")
        invariant = raw.get("invariant")
        if not isinstance(module, str) or not module.endswith(".tla"):
            raise RuntimeError(f"formal run {index} has an invalid module")
        if not isinstance(config, str) or not config.endswith(".cfg"):
            raise RuntimeError(f"formal run {index} has an invalid config")
        if Path(module).is_absolute() or ".." in Path(module).parts:
            raise RuntimeError(f"formal run {index} module escapes formal/")
        if Path(config).is_absolute() or ".." in Path(config).parts:
            raise RuntimeError(f"formal run {index} config escapes formal/")
        if expected not in {"success", "invariant_violation"}:
            raise RuntimeError(f"formal run {index} has an invalid expected verdict")
        if expected == "success" and invariant is not None:
            raise RuntimeError(f"formal run {index} success verdict cannot name an invariant")
        if expected == "invariant_violation" and not isinstance(invariant, str):
            raise RuntimeError(f"formal run {index} must name its violated invariant")
        pair = (module, config)
        if pair in pairs:
            raise RuntimeError(f"duplicate formal run {module} / {config}")
        if config in registered_configs:
            raise RuntimeError(f"formal config {config} is registered more than once")
        pairs.add(pair)
        registered_modules.add(module)
        registered_configs.add(config)
        normalized.append(
            {
                "module": module,
                "config": config,
                "expected": expected,
                **({"invariant": invariant} if isinstance(invariant, str) else {}),
            }
        )

    discovered_modules = relative_files(".tla")
    discovered_configs = relative_files(".cfg")
    if registered_modules != discovered_modules:
        missing = sorted(discovered_modules - registered_modules)
        stale = sorted(registered_modules - discovered_modules)
        raise RuntimeError(f"formal module registration drift: missing={missing}, stale={stale}")
    if registered_configs != discovered_configs:
        missing = sorted(discovered_configs - registered_configs)
        stale = sorted(registered_configs - discovered_configs)
        raise RuntimeError(f"formal config registration drift: missing={missing}, stale={stale}")
    return normalized


def run_model(
    java: Path,
    tools: Path,
    module: str,
    config: str,
) -> subprocess.CompletedProcess[str]:
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
            module,
        ]
        return subprocess.run(
            command,
            cwd=FORMAL_ROOT,
            capture_output=True,
            text=True,
            timeout=120,
            check=False,
        )


def validate_verdict(run: dict[str, str], result: subprocess.CompletedProcess[str]) -> bool:
    expected = run["expected"]
    if expected == "success":
        return result.returncode == 0 and SUCCESS_SENTINEL in result.stdout
    witness = f"Invariant {run['invariant']} is violated."
    return result.returncode != 0 and witness in result.stdout


def main() -> int:
    try:
        runs = load_runs()
        java = find_java()
        tools = find_tla_tools()
    except (RuntimeError, subprocess.TimeoutExpired) as error:
        print(f"formal-model setup error: {error}", file=sys.stderr)
        return 2

    for run in runs:
        try:
            result = run_model(java, tools, run["module"], run["config"])
        except (OSError, subprocess.TimeoutExpired) as error:
            print(f"formal-model execution error: {error}", file=sys.stderr)
            return 2
        sys.stdout.write(result.stdout)
        sys.stderr.write(result.stderr)
        if not validate_verdict(run, result):
            print(
                "formal-model failure: unexpected TLC verdict for "
                f"{run['module']} / {run['config']} (expected {run['expected']})",
                file=sys.stderr,
            )
            return result.returncode or 1
        if run["expected"] == "invariant_violation":
            print(
                "TLC reachability falsifier found the required "
                f"{run['invariant']} witness."
            )
    print(f"validated {len(runs)} discovered formal-model runs")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
