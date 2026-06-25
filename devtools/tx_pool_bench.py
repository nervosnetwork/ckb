#!/usr/bin/env python3
"""Run the tx-pool Criterion benchmark for both pipeline and sync modes and
print a comparison table.

Usage:
    python3 devtools/tx_pool_bench.py
    python3 devtools/tx_pool_bench.py --quick
"""

import os
import re
import subprocess
import sys
from collections import defaultdict
from typing import Dict, List, Tuple


QUICK = "--quick" in sys.argv


def run_cargo_bench(pipeline: bool) -> str:
    cmd = ["cargo", "bench", "-p", "ckb-tx-pool"]
    if pipeline:
        cmd.extend(["--features", "internal"])
    else:
        cmd.extend(["--no-default-features", "--features", "internal"])

    mode = "pipeline" if pipeline else "sync"
    print(f"\n>>> Running {mode} mode: {' '.join(cmd)}", flush=True)

    env = os.environ.copy()
    if QUICK:
        env["QUICK_BENCH"] = "1"

    # Stream output in real-time so progress is visible and hangs can be spotted.
    proc = subprocess.Popen(
        cmd,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        env=env,
    )
    lines: List[str] = []
    assert proc.stdout is not None
    for line in proc.stdout:
        print(line, end="", flush=True)
        lines.append(line)
    proc.wait()

    if proc.returncode != 0:
        raise RuntimeError(f"cargo bench failed for {mode} mode")
    return "".join(lines)


TIME_RE = re.compile(
    r"time:\s+\["
    r"(?P<lo>[\d.]+)\s+(?P<lo_unit>\w+)\s+"
    r"(?P<med>[\d.]+)\s+(?P<med_unit>\w+)\s+"
    r"(?P<hi>[\d.]+)\s+(?P<hi_unit>\w+)"
    r"\]"
)
THRPT_RE = re.compile(
    r"thrpt:\s+\["
    r"(?P<lo>[\d.]+)\s+(?P<lo_unit>[\w/]+)\s+"
    r"(?P<med>[\d.]+)\s+(?P<med_unit>[\w/]+)\s+"
    r"(?P<hi>[\d.]+)\s+(?P<hi_unit>[\w/]+)"
    r"\]"
)
NAME_RE = re.compile(
    r"^tx_pool_pipeline/(?P<mode>pipeline|sync)_"
    r"(?P<peers>\d+)peer_"
    r"(?P<workers>\d+)worker_"
    r"(?P<warm>warm_)?"
    r"(?P<tx_type>always_success|secp256k1|dependent_always_success|dependent_secp)_"
    r"(?P<size>\d+)$"
)


def to_ms(value: float, unit: str) -> float:
    if unit == "ns":
        return value / 1e6
    if unit == "us":
        return value / 1e3
    if unit == "ms":
        return value
    if unit == "s":
        return value * 1e3
    raise ValueError(f"unknown time unit: {unit}")


def to_elem_s(value: float, unit: str) -> float:
    if unit == "elem/s":
        return value
    if unit == "Kelem/s":
        return value * 1e3
    if unit == "Melem/s":
        return value * 1e6
    raise ValueError(f"unknown throughput unit: {unit}")


def parse_output(text: str) -> Dict[Tuple[str, int, int, bool, str, int], Dict]:
    """Parse Criterion text output.

    Returns a mapping of
    (mode, peers, workers, warm, tx_type, size) -> {time_ms, thrpt_elem_s}.
    """
    lines = text.splitlines()
    results = {}
    i = 0
    while i < len(lines):
        line = lines[i].strip()
        m = NAME_RE.match(line)
        if m:
            mode = m.group("mode")
            peers = int(m.group("peers"))
            workers = int(m.group("workers"))
            warm = m.group("warm") is not None
            tx_type = m.group("tx_type")
            size = int(m.group("size"))

            time_ms = None
            thrpt_elem_s = None
            j = i + 1
            while j < len(lines) and not NAME_RE.match(lines[j].strip()):
                t = TIME_RE.search(lines[j])
                if t:
                    time_ms = to_ms(float(t.group("med")), t.group("med_unit"))
                tp = THRPT_RE.search(lines[j])
                if tp:
                    thrpt_elem_s = to_elem_s(
                        float(tp.group("med")), tp.group("med_unit")
                    )
                j += 1

            if time_ms is None or thrpt_elem_s is None:
                raise RuntimeError(
                    f"could not parse results for {mode} {peers}peer {workers}worker "
                    f"{'warm ' if warm else ''}{tx_type} {size}"
                )
            results[(mode, peers, workers, warm, tx_type, size)] = {
                "time_ms": time_ms,
                "thrpt_elem_s": thrpt_elem_s,
            }
            i = j
        else:
            i += 1
    return results


def main() -> None:
    if QUICK:
        print("Quick mode: reduced peer/size/worker matrix (~5 minutes)")

    pipeline_text = run_cargo_bench(pipeline=True)
    sync_text = run_cargo_bench(pipeline=False)

    pipeline_results = parse_output(pipeline_text)
    sync_results = parse_output(sync_text)
    all_results = {**pipeline_results, **sync_results}

    # Organize by (tx_type, size, peers, workers, warm)
    groups = defaultdict(list)
    for (mode, peers, workers, warm, tx_type, size), vals in all_results.items():
        groups[(tx_type, size, peers, workers, warm)].append((mode, vals))

    print("\n# Tx-Pool Pipeline vs Sync Benchmark\n")
    for (tx_type, size, peers, workers, warm), modes in sorted(groups.items()):
        pipeline = next(
            (v for m, v in modes if m == "pipeline"),
            None,
        )
        sync = next(
            (v for m, v in modes if m == "sync"),
            None,
        )
        if pipeline is None or sync is None:
            continue

        ratio = pipeline["thrpt_elem_s"] / sync["thrpt_elem_s"]
        warm_label = "warm pool" if warm else "cold pool"
        print(f"## {tx_type} / {size} tx / {peers} peer(s) / {workers} worker(s) / {warm_label}\n")
        print("| mode | time | throughput |")
        print("|------|------|------------|")
        print(
            f"| pipeline | {pipeline['time_ms']:.2f} ms | {pipeline['thrpt_elem_s']/1e3:.2f} K tx/s |"
        )
        print(
            f"| sync | {sync['time_ms']:.2f} ms | {sync['thrpt_elem_s']/1e3:.2f} K tx/s |"
        )
        print(f"\npipeline / sync throughput: **{ratio:.2f}x**\n")


if __name__ == "__main__":
    main()
