"""Retain live sync state and node logs before the external test tears down.

Load explicitly with pytest -p ci_sync_diagnostics and set CKB_DIAGNOSTICS_DIR.
The original test, its deadline and its assertions remain unchanged.
"""

import json
import os
from pathlib import Path
import shutil

import pytest
import requests


@pytest.hookimpl(hookwrapper=True)
def pytest_runtest_makereport(item, call):
    report = (yield).get_result()
    cluster = getattr(item.cls, "cluster", None)
    if report.when != "call" or cluster is None:
        return
    output = Path(os.environ["CKB_DIAGNOSTICS_DIR"])
    output.mkdir(parents=True, exist_ok=True)
    observations = []
    for index, node in enumerate(cluster.ckb_nodes):
        url = node.getClient().url
        observed = {"url": url}
        for method in (
            "get_peers",
            "sync_state",
            "get_tip_header",
            "get_blockchain_info",
            "tx_pool_info",
        ):
            try:
                observed[method] = requests.post(
                    url,
                    json={"id": 1, "jsonrpc": "2.0", "method": method, "params": []},
                    timeout=2,
                ).json()
            except (requests.RequestException, ValueError) as error:
                observed[method] = {"diagnostic_error": str(error)}
        logs = output / f"node-{index}"
        logs.mkdir()
        for relative in (
            "data/logs",
            "node.log",
            "ckb.toml",
            "ckb-miner.toml",
            "dev.toml",
        ):
            source = Path(node.ckb_dir) / relative
            if source.is_dir():
                shutil.copytree(source, logs / source.name)
            elif source.is_file():
                shutil.copy2(source, logs / source.name)
        observations.append(observed)
    (output / "live-state.json").write_text(
        json.dumps(
            {"test": item.nodeid, "outcome": report.outcome, "nodes": observations},
            indent=2,
        )
        + "\n"
    )
