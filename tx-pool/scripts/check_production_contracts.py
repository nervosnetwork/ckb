#!/usr/bin/env python3
"""Validate cross-crate production contracts that Rust types cannot seal."""

from __future__ import annotations

import os
from pathlib import Path
import sys


REPO_ROOT = Path(__file__).resolve().parents[2]
CHAIN_VERIFY = REPO_ROOT / "chain" / "src" / "verify.rs"
TX_POOL_SERVICE = REPO_ROOT / "tx-pool" / "src" / "service.rs"
TX_POOL_CONTROLLER = REPO_ROOT / "tx-pool" / "src" / "service" / "controller.rs"
TX_POOL_BUILDER = REPO_ROOT / "tx-pool" / "src" / "service" / "builder.rs"
TX_POOL_AUTHORITY_SERVICE = (
    REPO_ROOT / "tx-pool" / "src" / "authority" / "service.rs"
)


def function_body(source: str, name: str) -> str | None:
    marker = f"fn {name}"
    start = source.find(marker)
    if start < 0:
        return None
    opening = source.find("{", start)
    if opening < 0:
        return None
    depth = 0
    for offset in range(opening, len(source)):
        character = source[offset]
        if character == "{":
            depth += 1
        elif character == "}":
            depth -= 1
            if depth == 0:
                return source[opening + 1 : offset]
    return None


def production_rust_sources() -> list[Path]:
    sources: list[Path] = []
    excluded = {".git", "target", "test", "tests", "benches"}
    for root, directories, files in os.walk(REPO_ROOT):
        directories[:] = [name for name in directories if name not in excluded]
        base = Path(root)
        sources.extend(base / name for name in files if name.endswith(".rs"))
    return sources


def validate_chain_transition_publication() -> list[str]:
    errors: list[str] = []
    chain = CHAIN_VERIFY.read_text()
    helper = function_body(chain, "install_chain_tip_transition")
    if helper is None:
        return [
            "chain/src/verify.rs must own install_chain_tip_transition as the sole "
            "best-tip publication boundary"
        ]

    required = (
        "self.shared.store_snapshot(Arc::clone(&new_snapshot))",
        ".update_tx_pool_for_reorg(",
    )
    for fragment in required:
        if helper.count(fragment) != 1:
            errors.append(
                "install_chain_tip_transition must contain exactly one "
                f"{fragment!r} operation"
            )
    if "service_started()" in helper:
        errors.append(
            "authoritative chain-tip publication must not depend on RPC readiness"
        )

    direct_snapshot_writers: list[str] = []
    reorg_publishers: list[str] = []
    for source in production_rust_sources():
        text = source.read_text()
        relative = source.relative_to(REPO_ROOT).as_posix()
        if ".store_snapshot(" in text and source.is_relative_to(REPO_ROOT / "chain" / "src"):
            direct_snapshot_writers.extend(
                relative for _ in range(text.count(".store_snapshot("))
            )
        if ".update_tx_pool_for_reorg(" in text:
            reorg_publishers.extend(
                relative for _ in range(text.count(".update_tx_pool_for_reorg("))
            )
    if direct_snapshot_writers != ["chain/src/verify.rs"]:
        errors.append(
            "chain best-tip snapshots must be installed only by the reviewed transition "
            f"boundary, found {direct_snapshot_writers}"
        )
    if reorg_publishers != ["chain/src/verify.rs"]:
        errors.append(
            "the ordered tx-pool reorg channel must have one production publisher, "
            f"found {reorg_publishers}"
        )
    if chain.count("self.install_chain_tip_transition(&fork, new_snapshot);") != 2:
        errors.append(
            "normal best-block and truncate paths must both install their transition "
            "through install_chain_tip_transition"
        )
    return errors


def validate_startup_backpressure() -> list[str]:
    errors: list[str] = []
    service = TX_POOL_SERVICE.read_text()
    controller = TX_POOL_CONTROLLER.read_text()
    builder = TX_POOL_BUILDER.read_text()
    authority_service = TX_POOL_AUTHORITY_SERVICE.read_text()

    if "const REORG_CHANNEL_SIZE: usize = 1;" not in service:
        errors.append("the ordered reorg startup boundary must retain capacity one")
    update = function_body(controller, "update_tx_pool_for_reorg")
    if update is None:
        errors.append("TxPoolController::update_tx_pool_for_reorg disappeared")
    else:
        if "reorg_sender.send(notify)" not in update:
            errors.append("authoritative reorg delivery must use bounded async send")
        if "try_send" in update or "service_started" in update:
            errors.append(
                "authoritative reorg delivery may neither drop on capacity nor gate on readiness"
            )

    assemble = builder.find("AuthorityService::assemble(")
    replay = builder.find("service.replay_persisted(")
    ready = builder.find("started.store(true, Ordering::Release)")
    if min(assemble, replay, ready) < 0 or not assemble < replay < ready:
        errors.append(
            "startup must assemble the reorg consumer before persistence replay and publish "
            "RPC readiness only after replay"
        )
    assembly = function_body(authority_service, "assemble")
    if assembly is None or "run_ordered_reorg_driver" not in assembly:
        errors.append("AuthorityService::assemble must own the ordered reorg consumer")
    return errors


def main() -> int:
    errors = [
        *validate_chain_transition_publication(),
        *validate_startup_backpressure(),
    ]
    if errors:
        for error in errors:
            print(f"error: {error}", file=sys.stderr)
        return 1
    print(
        "validated cross-crate chain-tip publication, startup ordering and "
        "bounded reorg backpressure"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
