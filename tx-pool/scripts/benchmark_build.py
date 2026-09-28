#!/usr/bin/env python3
"""Build one frozen tx-pool benchmark and bind its executable to Cargo's inputs."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import shlex
import subprocess
from pathlib import Path

from measurement_process import run_process


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def command_output(command: list[str], root: Path | None = None) -> str:
    try:
        return run_process(command, cwd=root, text=True, stdout=subprocess.PIPE,
                           stderr=subprocess.STDOUT, timeout=120, check=True).stdout.strip()
    except (OSError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
        raise RuntimeError(f"cannot run {command[0]} in {root or Path.cwd()}: {error}") from error


def git_record(root: Path) -> dict[str, str]:
    root = root.resolve()
    if command_output(["git", "status", "--porcelain=v1", "--untracked-files=all"], root):
        raise RuntimeError(f"measurement worktree is dirty: {root}")
    return {"root": str(root), "commit": command_output(["git", "rev-parse", "HEAD"], root),
            "cargo_lock_sha256": sha256(root / "Cargo.lock"),
            "cargo_manifest_sha256": sha256(root / "Cargo.toml"),
            "tx_pool_manifest_sha256": sha256(root / "tx-pool/Cargo.toml")}


def binary_record(path: Path) -> dict[str, object]:
    resolved = path.expanduser().resolve()
    if not resolved.is_file():
        raise RuntimeError(f"fixed binary does not exist: {resolved}")
    return {"path": str(resolved), "sha256": sha256(resolved), "size": resolved.stat().st_size}


def effective_features(features: str, allocation: bool, bench: str) -> str:
    """Use the same Cargo-qualified features for metadata, builds and receipts."""
    enabled = {value if "/" in value else f"ckb-tx-pool/{value}"
               for value in re.split(r"[,\s]+", features.strip()) if value}
    observation = "ckb-tx-pool/allocation-observation"
    if allocation:
        enabled.add(observation)
    elif observation in enabled:
        raise ValueError("allocation-observation feature requires allocation observation mode")
    if bench == "packing_one_shot":
        enabled.add("ckb-tx-pool/packing-bench")
    return ",".join(sorted(enabled))


def host_identity() -> dict[str, object]:
    if platform.system() == "Darwin":
        cpu = command_output(["sysctl", "-n", "machdep.cpu.brand_string"])
    elif platform.system() == "Linux":
        cpu = next((line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines()
                    if line.startswith(("model name", "Hardware"))), platform.processor())
    else:
        cpu = platform.processor()
    return {"platform": platform.platform(), "machine": platform.machine(),
            "node": platform.node(), "cpu_model": cpu,
            "python": platform.python_version(), "cpu_count": os.cpu_count(),
            "rustc": command_output(["rustc", "-Vv"]), "cargo": command_output(["cargo", "-V"])}


def build_command(bench: str, features: str) -> list[str]:
    command = ["cargo", "bench", "-p", "ckb-tx-pool", "--bench", bench, "--no-run",
               "--locked", "--profile", "prod", "--message-format=json"]
    if features:
        command.extend(("--features", features))
    return command


def cargo_messages(output: str):
    for line in output.splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(message, dict):
            yield message


def parse_cargo_artifact(output: str, bench: str) -> dict[str, object]:
    """Select the one executable Cargo produced for the requested benchmark."""
    artifacts = []
    for message in cargo_messages(output):
        target = message.get("target", {})
        if (message.get("reason") == "compiler-artifact" and target.get("name") == bench
                and "bench" in target.get("kind", []) and message.get("executable")):
            artifacts.append(message)
    if len(artifacts) != 1:
        raise RuntimeError(f"Cargo reported {len(artifacts)} {bench} executables")
    return artifacts[0]


def rocksdb_build_observation(output: str) -> dict[str, object]:
    """Read the build output selected by this Cargo invocation, including cached builds."""
    messages = list(cargo_messages(output))
    libraries = [message for message in messages
                 if message.get("reason") == "compiler-artifact"
                 and message.get("target", {}).get("name") == "ckb_librocksdb_sys"
                 and "lib" in message.get("target", {}).get("kind", [])]
    if len(libraries) != 1:
        raise RuntimeError(f"Cargo reported {len(libraries)} RocksDB libraries")
    package_id = libraries[0]["package_id"]
    scripts = [message for message in messages
               if message.get("reason") == "build-script-executed"
               and message.get("package_id") == package_id]
    if len(scripts) != 1 or not scripts[0].get("out_dir"):
        raise RuntimeError("Cargo did not identify one RocksDB build-script output")
    # Cargo stores stdout beside OUT_DIR. Never select a different cached build
    # by modification time, or rerun probes to infer what this artifact used.
    path = Path(scripts[0]["out_dir"]).parent / "output"
    try:
        lines = [line.removeprefix("PLATFORM_CXXFLAGS:").strip()
                 for line in path.read_text().splitlines() if line.startswith("PLATFORM_CXXFLAGS:")]
        if len(lines) > 1:
            raise ValueError("duplicate PLATFORM_CXXFLAGS records")
        flags = json.loads(lines[0]) if lines else None
        if lines and (not isinstance(flags, list) or any(not isinstance(flag, str) for flag in flags)):
            raise ValueError("PLATFORM_CXXFLAGS must be a string array")
    except (OSError, ValueError) as error:
        raise RuntimeError(f"cannot observe RocksDB detected flags in {path}: {error}") from error
    return {"package_id": package_id, "build_script": scripts[0], "detected_cxxflags": flags}


def rocksdb_configuration(receipt: dict[str, object]) -> tuple[str, list[str]] | None:
    """Validate the archived observation; absent evidence never means an empty flag list."""
    if receipt.get("schema") == 1:
        return None
    observation = receipt.get("rocksdb_build")
    if not isinstance(observation, dict):
        raise RuntimeError("build receipt lacks its RocksDB observation")
    package_id = observation.get("package_id")
    script = observation.get("build_script", {})
    flags = observation.get("detected_cxxflags")
    if (not isinstance(package_id, str) or not package_id or not isinstance(script, dict)
            or script.get("reason") != "build-script-executed" or script.get("package_id") != package_id
            or not isinstance(script.get("out_dir"), str) or not script["out_dir"]
            or "detected_cxxflags" not in observation
            or (flags is not None and (not isinstance(flags, list)
                                      or any(not isinstance(flag, str) for flag in flags)))):
        raise RuntimeError("invalid RocksDB build observation")
    return None if flags is None else (package_id, flags)


def validate_rocksdb_builds(baseline: dict[str, object], candidate: dict[str, object]) -> None:
    configurations = [rocksdb_configuration(receipt) for receipt in (baseline, candidate)]
    if any(configuration is None for configuration in configurations):
        raise RuntimeError("comparison requires recorded RocksDB detected flags on both sides; rebuild with the current builder")
    if configurations[0] != configurations[1]:
        raise RuntimeError(f"RocksDB package or detected flags differ: {configurations[0]!r} != {configurations[1]!r}")


def build_binary(root: Path, target_dir: Path, features: str, bench: str) -> dict[str, object]:
    root, target_dir = root.resolve(), target_dir.resolve()
    source = git_record(root)
    command = build_command(bench, features)
    environment = os.environ.copy()
    environment.update(CARGO_TARGET_DIR=str(target_dir), CARGO_INCREMENTAL="0")
    encoded = environment.get("CARGO_ENCODED_RUSTFLAGS")
    inherited = encoded.split("\x1f") if encoded else shlex.split(environment.pop("RUSTFLAGS", ""))
    environment.pop("RUSTFLAGS", None)
    environment["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(
        [*inherited, f"--remap-path-prefix={root}=/ckb-txpool-cross-source"])
    toolchain = {name: command_output([name, "-Vv" if name == "rustc" else "-V"], root)
                 for name in ("rustc", "cargo")}
    completed = run_process(command, timeout=3600, cwd=root, env=environment, text=True,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
    if completed.returncode != 0:
        raise RuntimeError(f"benchmark build failed ({completed.returncode}):\n"
                           f"{completed.stdout[-4000:]}\n{completed.stderr[-4000:]}")
    artifact = parse_cargo_artifact(completed.stdout, bench)
    if git_record(root) != source:
        raise RuntimeError("source changed during benchmark build")
    binary = binary_record(Path(artifact["executable"]))
    # Normalize Cargo's executable path once; archived receipts need no filesystem
    # or original symlinks to check their producer/artifact relationship.
    artifact = artifact | {"executable": binary["path"]}
    return {"schema": 2, "kind": "tx_pool_benchmark_build", "source": source,
            "bench": bench, "features": features, "profile": "prod", "toolchain": toolchain,
            "command": command, "environment": {name: environment[name] for name in
                ("CARGO_TARGET_DIR", "CARGO_INCREMENTAL", "CARGO_ENCODED_RUSTFLAGS")},
            "cargo_artifact": artifact, "binary": binary,
            "rocksdb_build": rocksdb_build_observation(completed.stdout)}


def validate_build(receipt: dict[str, object], source: dict[str, object], bench: str,
                   features: str, binary: dict[str, object]) -> None:
    """The same source-to-artifact contract applies to live and archived observations."""
    if (receipt.get("schema") not in (1, 2) or receipt.get("kind") != "tx_pool_benchmark_build"
            or receipt.get("source") != source or receipt.get("bench") != bench
            or receipt.get("profile") != "prod" or receipt.get("features") != features):
        raise RuntimeError("build receipt source, bench, profile or features differ")
    artifact = receipt.get("cargo_artifact", {})
    if (receipt.get("command") != build_command(bench, features)
            or artifact.get("reason") != "compiler-artifact"
            or artifact.get("target", {}).get("name") != bench
            or "bench" not in artifact.get("target", {}).get("kind", [])
            or not artifact.get("executable")
            or artifact["executable"] != receipt["binary"]["path"]
            or any(not receipt.get("toolchain", {}).get(name) for name in ("rustc", "cargo"))):
        raise RuntimeError("build receipt does not contain the matching Cargo build observation")
    if any(binary.get(field) is None or binary.get(field) != receipt["binary"].get(field)
           for field in ("sha256", "size")):
        raise RuntimeError("binary differs from its Cargo build receipt")
    rocksdb_configuration(receipt)


def load_build(path: Path, root: Path, bench: str, features: str,
               supplied: Path | None = None) -> tuple[dict[str, object], dict[str, object]]:
    """Verify a producer-owned receipt; a copied executable may change only its path."""
    receipt = json.loads(path.read_text())
    binary = binary_record(supplied or Path(receipt["binary"]["path"]))
    validate_build(receipt, git_record(root), bench, features, binary)
    return binary, receipt


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--bench", choices=("profile_one_shot", "packing_one_shot"), required=True)
    parser.add_argument("--target-dir", type=Path, required=True)
    parser.add_argument("--features", default="")
    parser.add_argument("--allocation-observation", choices=("disabled", "enabled"), default="disabled")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists() or args.output.resolve().is_relative_to(args.root.resolve()):
        parser.error("build receipt must be a new path outside the source checkout")
    features = effective_features(args.features, args.allocation_observation == "enabled", args.bench)
    receipt = build_binary(args.root, args.target_dir, features, args.bench)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("x") as output:
        output.write(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps(receipt["binary"], indent=2))


if __name__ == "__main__":
    main()
