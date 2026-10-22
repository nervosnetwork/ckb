#!/usr/bin/env python3
"""Capture and replay fixed-corpus local-node benchmarks from an explicit JSON configuration."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import selectors
import shutil
import signal
import socket
import subprocess
import sys
import time
import tomllib
import urllib.parse
import urllib.request
from pathlib import Path

from benchmark_build import binary_record, git_record, host_identity, load_build, sha256, validate_build
from external_observation import parse, require
from measurement_process import run_process, terminate_as_exception


def save(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def rpc(url, method, params=()):
    parsed = urllib.parse.urlparse(url)
    require(parsed.scheme == "http" and parsed.hostname in ("localhost", "127.0.0.1", "::1"),
            "external benchmark requires a local HTTP dev node")
    request = urllib.request.Request(url, data=json.dumps({
        "jsonrpc": "2.0", "id": 1, "method": method, "params": params,
    }).encode(), headers={"Content-Type": "application/json"})
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    with opener.open(request, timeout=10) as response:
        result = json.load(response)
    require(result.get("id") == 1 and result.get("jsonrpc") == "2.0"
            and "result" in result and "error" not in result, f"{method} failed: {result}")
    return result["result"]


def export_funding(url, fixture, output):
    """Export a stable canonical chain; the native producer derives live fixture cells."""
    require(set(fixture) == {"seed_key", "users", "lock_code_hash", "cell_dep"}, "funding fixture fields differ")
    tip = rpc(url, "get_tip_header")
    blocks = [rpc(url, "get_block_by_number", [hex(number)]) for number in range(int(tip["number"], 16) + 1)]
    require(rpc(url, "get_tip_header")["hash"] == tip["hash"]
            and blocks[-1]["header"]["hash"] == tip["hash"], "chain changed during funding export")
    with output.open("x") as stream:
        json.dump({"schema": 1, **fixture, "blocks": blocks}, stream)
        stream.write("\n")


def tree_identity(root):
    files = {}
    for path in sorted(root.rglob("*")):
        require(not path.is_symlink(), "seed must not contain symbolic links")
        if path.is_file():
            files[str(path.relative_to(root))] = {"size": path.stat().st_size, "sha256": sha256(path)}
    require(files, "seed directory is empty")
    return {"files": files, "sha256": hashlib.sha256(json.dumps(files, sort_keys=True).encode()).hexdigest()}


def configure_seed(node, config):
    path = node / "ckb.toml"
    contents = path.read_text()
    parsed = tomllib.loads(contents)
    require(parsed.get("data_dir") == "data", "dev seed requires its owned relative data_dir = data")
    spec = parsed["chain"]["spec"]
    require(spec == {"file": "specs/dev.toml"}
            and tomllib.loads((node / "specs/dev.toml").read_text())["pow"]["func"] == "Dummy",
            "requires an explicit local Dummy-PoW dev seed")
    changes = {
        "max_tx_verify_workers": str(config["workers"]),
        "listen_address": f'"127.0.0.1:{config["rpc_port"]}"',
        "listen_addresses": f'["/ip4/127.0.0.1/tcp/{config["rpc_port"] + 1}"]',
        "connect_outbound_interval_secs": "0",
    }
    for key, value in changes.items():
        contents, count = re.subn(rf"(?m)^{key} = .*", f"{key} = {value}", contents)
        require(count == 1, f"seed must contain exactly one {key}")
    path.write_text(contents)


def validate_config(config):
    paths = {"root", "bench", "bench_build", "node", "node_build", "miner", "miner_build", "seed", "corpus"}
    bounds = {"workers": (1, 256), "concurrency": (1, 1024), "poll_ms": (1, 1000),
              "mine_ms": (1, 10_000), "timeout_s": (1, 3600), "rpc_port": (1024, 65534)}
    require(set(config) == paths | bounds.keys(), "capture configuration fields differ")
    for name in paths:
        require(isinstance(config[name], str) and Path(config[name]).is_absolute(), f"{name} must be absolute")
    for name, (lower, upper) in bounds.items():
        require(type(config[name]) is int and lower <= config[name] <= upper, f"invalid {name}")


def external_build(receipt, binary, *, node):
    """Preserve the node/miner producer receipts alongside the native Cargo receipt."""
    require(receipt.get("sha256") == binary["sha256"] and isinstance(receipt.get("command"), list),
            "external executable differs from its build receipt")
    if node:
        require(receipt.get("status") == "" and receipt.get("patch") == ""
                and re.fullmatch("[0-9a-f]{40}", receipt.get("base", ""))
                and receipt.get("artifact", {}).get("target", {}).get("name") == "ckb"
                and receipt["artifact"]["profile"]["debug_assertions"] is False
                and "--locked" in receipt["command"] and "prod" in receipt["command"],
                "node receipt does not identify a clean production build")
    else:
        require(re.fullmatch("[0-9a-f]{40}", receipt.get("source_sha", ""))
                and isinstance(receipt.get("patch"), str) and "--locked" in receipt["command"],
                "miner receipt lacks source/patch/build identity")
    return receipt


def validate_usage(usage):
    require(usage.get("returncode") == 0 and usage.get("forced_kill") is False,
            "node did not shut down cleanly")
    require(usage.get("scope") == "whole_node_lifetime_including_startup_drain_and_shutdown",
            "node usage scope differs")
    for name in ("user_cpu_ns", "system_cpu_ns", "peak_rss_bytes"):
        require(type(usage.get(name)) is int and usage[name] >= 0, f"invalid node usage: {name}")
    require(usage["peak_rss_bytes"] > 0 and usage["user_cpu_ns"] + usage["system_cpu_ns"] > 0,
            "node usage observation is empty")


def validate_shutdown(stopped):
    require(set(stopped) == {"executor", "miner"}
            and stopped["executor"] == {"returncode": 0, "forced_kill": False}
            and stopped["miner"]["returncode"] in (0, -signal.SIGINT)
            and stopped["miner"]["forced_kill"] is False,
            "executor/miner did not finish or stop cleanly")


def stop_node(process):
    """Reap once with wait4 so early exit and normal shutdown both retain usage."""
    try:
        os.kill(process.pid, signal.SIGINT)
    except ProcessLookupError:
        pass
    deadline = time.monotonic() + 20
    killed = False
    while True:
        pid, status, usage = os.wait4(process.pid, os.WNOHANG)
        if pid:
            process.returncode = os.waitstatus_to_exitcode(status)
            return {"returncode": process.returncode, "forced_kill": killed,
                    "user_cpu_ns": round(usage.ru_utime * 1e9), "system_cpu_ns": round(usage.ru_stime * 1e9),
                    "peak_rss_bytes": usage.ru_maxrss * (1 if sys.platform == "darwin" else 1024),
                    "scope": "whole_node_lifetime_including_startup_drain_and_shutdown"}
        if time.monotonic() >= deadline and not killed:
            os.kill(process.pid, signal.SIGKILL)
            killed = True
        time.sleep(.02)


def run_owned(config, output):
    """Run only inside run_process's group; all children inherit its ownership."""
    node_dir = output / "node-data"
    url = f'http://127.0.0.1:{config["rpc_port"]}'
    environment = dict(os.environ, NO_PROXY="127.0.0.1,localhost,::1", no_proxy="127.0.0.1,localhost,::1")
    for port in (config["rpc_port"], config["rpc_port"] + 1):
        with socket.socket() as probe:
            probe.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            probe.bind(("127.0.0.1", port))
    children, commands = [], []
    node = None

    def start(name, command, **kwargs):
        with (output / f"{name}.log").open("wb") as log:
            process = subprocess.Popen(command, cwd=output, env=environment,
                                       stderr=log, stdout=kwargs.pop("stdout", log), **kwargs)
        children.append((name, process))
        commands.append({"name": name, "pid": process.pid, "command": command})
        save(output / "processes.json", commands)
        return process

    try:
        node = start("node", [config["node"], "-C", str(node_dir), "run", "--ba-advanced", "--indexer"])
        deadline = time.monotonic() + 60
        while True:
            try:
                rpc(url, "tx_pool_info")
                break
            except (OSError, ValueError):
                if time.monotonic() >= deadline:
                    raise TimeoutError("node readiness timed out")
                time.sleep(.1)
        bench = start("executor", [config["bench"], "external-run", config["corpus"], url,
                    str(config["concurrency"]), str(config["poll_ms"]), str(config["timeout_s"])],
                    stdin=subprocess.PIPE, stdout=subprocess.PIPE)
        with selectors.DefaultSelector() as selector:
            selector.register(bench.stdout, selectors.EVENT_READ)
            require(bool(selector.select(60)), "executor readiness timed out")
            ready = bench.stdout.readline()
        (output / "stdout.log").write_bytes(ready)
        require(ready == b"EXTERNAL_READY\n", "executor did not validate the starting seed")
        miner = start("miner", [config["miner"], "miner", "--rpc-urls", url,
                        "--mining-interval-ms", str(config["mine_ms"]), "--min-tx-size", "0"])
        stdout = b""
        try:
            stdout, _ = bench.communicate(input=b"start\n", timeout=config["timeout_s"] + 5)
        except subprocess.TimeoutExpired:
            bench.kill()
            stdout, _ = bench.communicate(timeout=5)
            raise
        finally:
            (output / "stdout.log").write_bytes(ready + stdout)
        require(bench.returncode == 0, "external executor failed; see original records")
        require(miner.poll() is None, "miner exited before confirmation completed")
        deadline = time.monotonic() + 30
        while True:
            pool = rpc(url, "tx_pool_info")
            tip = rpc(url, "get_tip_header")
            if pool["tip_hash"] == tip["hash"] and all(pool[name] == "0x0" for name in ("pending", "proposed", "orphan")):
                save(output / "final-state.json", {"pool": pool, "tip": tip})
                break
            if time.monotonic() >= deadline:
                raise TimeoutError("confirmed corpus did not leave an empty, aligned pool")
            time.sleep(.01)
    finally:
        stopped = {}
        for name, process in reversed(children):
            if name == "node":
                continue
            killed = False
            if process.poll() is None:
                process.send_signal(signal.SIGINT)
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    killed = True
                    process.wait(timeout=5)
            stopped[name] = {"returncode": process.returncode, "forced_kill": killed}
        save(output / "child-shutdown.json", stopped)
        if node is not None:
            save(output / "node-usage.json", stop_node(node))


def corpus_identity(path):
    with path.open() as stream:
        metadata = json.loads(next(stream))
        hashes = [json.loads(line)["hash"] for line in stream]
    return metadata, hashes


def artifacts(output):
    return {str(path.relative_to(output)): {"size": path.stat().st_size, "sha256": sha256(path)}
            for path in sorted(output.rglob("*")) if path.is_file() and path.name != "receipt.json"
            and "node-data" not in path.relative_to(output).parts}


def validate_node_logs(output):
    logger = tomllib.loads((output / "ckb.toml").read_text()).get("logger", {})
    if logger.get("log_to_file", False):
        require((output / "node-logs/run.log").is_file(), "configured node file log was not retained")


def finish_capture(output, receipt):
    """Archive evidence after the owned group stops, before deleting its database."""
    node_dir = output / "node-data"
    try:
        logs = node_dir / "data/logs"
        if logs.is_dir():
            shutil.copytree(logs, output / "node-logs")
        if receipt["state"] == "complete":
            validate_node_logs(output)
    except BaseException as error:
        receipt.update(state="failed", log_retention_error=f"{type(error).__name__}: {error}")
        raise
    finally:
        receipt["artifacts"] = artifacts(output)
        save(output / "receipt.json", receipt)
    # A failed archive leaves the complete source directory available for repair.
    if node_dir.exists():
        shutil.rmtree(node_dir)


def capture(config, output):
    validate_config(config)
    require(all(not output.resolve().is_relative_to(Path(config[name]).resolve()) for name in ("root", "seed")),
            "capture artifacts must be outside the source checkout and frozen seed")
    output.mkdir(parents=True, exist_ok=False)
    save(output / "config.json", config)
    receipt = {"schema": 1, "contract": "fixed_corpus_observed_confirmation_v2", "state": "preparing", "config": config}
    save(output / "receipt.json", receipt)
    try:
        bench_build = json.loads(Path(config["bench_build"]).read_text())
        bench, bench_build = load_build(Path(config["bench_build"]), Path(config["root"]),
                                        "profile_one_shot", bench_build["features"], Path(config["bench"]))
        node, miner = binary_record(Path(config["node"])), binary_record(Path(config["miner"]))
        receipt.update({"bench": bench, "bench_build": bench_build, "node": node, "miner": miner,
                        "node_build": external_build(json.loads(Path(config["node_build"]).read_text()), node, node=True),
                        "miner_build": external_build(json.loads(Path(config["miner_build"]).read_text()), miner, node=False),
                        "host": host_identity(), "corpus_sha256": sha256(Path(config["corpus"])),
                        "tools": {name: sha256(Path(__file__).with_name(name)) for name in
                                  ("external_benchmark.py", "external_observation.py", "measurement_process.py", "benchmark_build.py")}})
        receipt["seed"] = tree_identity(Path(config["seed"]))
        shutil.copytree(config["seed"], output / "node-data")
        require(tree_identity(output / "node-data") == receipt["seed"], "seed changed while being copied")
        configure_seed(output / "node-data", config)
        shutil.copyfile(output / "node-data/ckb.toml", output / "ckb.toml")
        shutil.copyfile(output / "node-data/specs/dev.toml", output / "dev.toml")
        receipt["state"] = "running"
        save(output / "receipt.json", receipt)
        with (output / "runner.log").open("w") as log:
            completed = run_process([sys.executable, str(Path(__file__).resolve()), "__capture__", str(output)],
                                    timeout=config["timeout_s"] + 180, stdout=log, stderr=subprocess.STDOUT)
        require(completed.returncode == 0, "capture process failed; preserve its logs")
        metadata, hashes = corpus_identity(Path(config["corpus"]))
        receipt["observation"] = parse((output / "stdout.log").read_text(), metadata, hashes,
                                       load={name: config[name] for name in ("concurrency", "poll_ms", "timeout_s")})
        usage = json.loads((output / "node-usage.json").read_text())
        validate_usage(usage)
        validate_shutdown(json.loads((output / "child-shutdown.json").read_text()))
        receipt["node_usage"] = usage
        require(sha256(Path(config["corpus"])) == receipt["corpus_sha256"], "corpus changed during capture")
        for kind in ("bench", "node", "miner"):
            require(binary_record(Path(config[kind])) == receipt[kind], f"{kind} executable changed during capture")
        require(git_record(Path(config["root"])) == bench_build["source"], "benchmark source changed during capture")
        for name, digest in receipt["tools"].items():
            require(sha256(Path(__file__).with_name(name)) == digest, "capture tool changed during measurement")
        receipt["state"] = "complete"
    except BaseException as error:
        receipt["state"] = "failed"
        receipt["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        finish_capture(output, receipt)
    return receipt


def replay(output):
    receipt = json.loads((output / "receipt.json").read_text())
    require(receipt.get("schema") == 1 and receipt.get("contract") == "fixed_corpus_observed_confirmation_v2"
            and receipt.get("state") == "complete", "capture is incomplete or unsupported")
    require(artifacts(output) == receipt["artifacts"], "capture artifacts changed")
    validate_node_logs(output)
    require(json.loads((output / "config.json").read_text()) == receipt["config"], "saved configuration differs")
    validate_config(receipt["config"])
    corpus = Path(receipt["config"]["corpus"])
    require(sha256(corpus) == receipt["corpus_sha256"], "frozen corpus changed")
    build = receipt["bench_build"]
    validate_build(build, build["source"], "profile_one_shot", build["features"], receipt["bench"])
    external_build(receipt["node_build"], receipt["node"], node=True)
    external_build(receipt["miner_build"], receipt["miner"], node=False)
    metadata, hashes = corpus_identity(corpus)
    observation = parse((output / "stdout.log").read_text(), metadata, hashes,
                        load={name: receipt["config"][name] for name in ("concurrency", "poll_ms", "timeout_s")})
    require(observation == receipt["observation"], "saved observation differs from raw evidence")
    require(json.loads((output / "node-usage.json").read_text()) == receipt["node_usage"], "node usage differs")
    validate_usage(receipt["node_usage"])
    validate_shutdown(json.loads((output / "child-shutdown.json").read_text()))
    return observation


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    for name in ("capture", "replay", "__capture__"):
        command = commands.add_parser(name)
        if name == "capture":
            command.add_argument("config", type=Path)
        command.add_argument("output", type=Path)
    funding = commands.add_parser("export-funding")
    funding.add_argument("url")
    funding.add_argument("fixture", type=Path)
    funding.add_argument("output", type=Path)
    args = parser.parse_args()
    if args.command == "capture":
        result = capture(json.loads(args.config.read_text()), args.output.resolve())
        print(json.dumps(result["observation"], indent=2))
    elif args.command == "replay":
        print(json.dumps(replay(args.output.resolve()), indent=2))
    elif args.command == "export-funding":
        export_funding(args.url, json.loads(args.fixture.read_text()), args.output)
    else:
        require(os.getpid() == os.getpgrp() == os.getsid(0), "capture requires an owned process group")
        with terminate_as_exception():
            run_owned(json.loads((args.output / "config.json").read_text()), args.output)


if __name__ == "__main__":
    main()
