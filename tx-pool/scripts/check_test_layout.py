#!/usr/bin/env python3
"""Enforce tx-pool test isolation and explicit test-only seam review."""

from __future__ import annotations

import json
from pathlib import Path
import re
import sys

from check_review_guide import (
    behavior_ids as registered_behavior_ids,
    load_registry,
    validate_registry,
)
from check_production_contracts import function_body, mask_rust_non_code, matching_brace


REPO_ROOT = Path(__file__).resolve().parents[2]
SOURCE_ROOT = REPO_ROOT / "tx-pool" / "src"
AUTHORITY_TEST_ROOT = SOURCE_ROOT / "authority" / "tests"
AUTHORITY_TEST_SUPPORT_PLAN = AUTHORITY_TEST_ROOT / "support" / "plan.rs"
AUTHORITY_TEST_SUPPORT_SCHEDULER = AUTHORITY_TEST_ROOT / "support" / "scheduler.rs"
AUTHORITY_TEST_SUPPORT_WORKER = AUTHORITY_TEST_ROOT / "support" / "worker.rs"
SYNC_TEST_ROOT = REPO_ROOT / "sync" / "src" / "tests"
SYNC_CHAIN_ONLY_FIXTURE = SYNC_TEST_ROOT / "util.rs"
MANIFEST_PATH = REPO_ROOT / "tx-pool" / "test-layout-manifest.json"
CFG_TEST = re.compile(r"#\[cfg\(test\)\]")
TEST_ATTRIBUTE = re.compile(r"(?m)^\s*#\[(?:tokio::)?test(?:\([^]]*\))?\]")
INLINE_TEST_MODULE = re.compile(
    r"(?m)^\s*(?:#\[cfg\(test\)\]\s*)?"
    r"mod\s+(?:tests|[A-Za-z0-9_]*(?:_tests|test_support))\s*\{"
)
TEST_MODULE_WIRING = re.compile(
    r"#\[cfg\(test\)\]\s*"
    r"(?:#\[path\s*=\s*\"([^\"]+)\"\]\s*)?"
    r"(?:pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*;"
)
BEHAVIOR_ID = re.compile(r"^TP-[A-Z]+-[0-9]{3}$")
FORBIDDEN_PRODUCTION = (
    (
        re.compile(
            r"\b(?:assert|assert_eq|assert_ne|debug_assert|debug_assert_eq|"
            r"debug_assert_ne|panic|unreachable|todo|unimplemented)\s*!"
        ),
        "panic-capable macro",
    ),
    (re.compile(r"\.(?:unwrap|expect)\s*\("), "panic-capable result extraction"),
    (re.compile(r"\bcatch_unwind\s*\("), "unwind-based control flow"),
    (re.compile(r"\.get_unchecked(?:_mut)?\s*\("), "unchecked indexing"),
)
REQUIRED_STATIC_LINTS = {
    "clippy::arithmetic_side_effects",
    "clippy::await_holding_lock",
    "clippy::expect_used",
    "clippy::indexing_slicing",
    "clippy::panic",
    "clippy::unreachable",
    "clippy::unwrap_used",
}


def repo_path(path: str) -> Path:
    resolved = (REPO_ROOT / path).resolve()
    try:
        resolved.relative_to(REPO_ROOT)
    except ValueError as error:
        raise ValueError(f"path escapes repository root: {path}") from error
    return resolved


def load_manifest() -> dict:
    try:
        return json.loads(MANIFEST_PATH.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise SystemExit(f"cannot load test-layout manifest: {error}") from error


def relative(path: Path) -> str:
    return path.relative_to(REPO_ROOT).as_posix()


def is_test_source(path: Path, roots: tuple[Path, ...], files: set[Path]) -> bool:
    if path in files:
        return True
    return any(path.is_relative_to(root) for root in roots)


def expected_module_target(source: Path, module: str, module_path: str | None) -> Path:
    if module_path is not None:
        return (source.parent / module_path).resolve()
    base = (
        source.parent
        if source.name in {"lib.rs", "main.rs", "mod.rs"}
        else source.parent / source.stem
    )
    direct = base / f"{module}.rs"
    nested = base / module / "mod.rs"
    if direct.exists():
        return direct.resolve()
    return nested.resolve()


def require_compact_fragments(
    body: str, owner: str, fragments: tuple[str, ...]
) -> list[str]:
    compact = "".join(mask_rust_non_code(body).split())
    return [
        f"{owner} lost required proof fragment {fragment!r}"
        for fragment in fragments
        if fragment not in compact
    ]


def validate_dependency_progress_layout() -> list[str]:
    """Keep every maintenance drain bound to the complete test-only rank."""

    try:
        support_plan = AUTHORITY_TEST_SUPPORT_PLAN.read_text()
        drain = function_body(support_plan, "drain_dependency_maintenance_for_foundation")
    except (OSError, ValueError) as error:
        return [f"cannot inspect dependency progress evidence: {error}"]
    if drain is None:
        return ["the rank-derived dependency drain disappeared"]

    errors = require_compact_fragments(
        drain,
        "rank-derived dependency drain",
        (
            "whilebefore_rank.value()!=0",
            "MissingSuccessor(before_rank)",
            "before_rank.strictly_decreases_to(after_rank)",
            "DependencyMaintenanceDrainError::Nondecreasing",
            "DependencyMaintenanceDrainError::ResidualSuccessor",
        ),
    )
    for path in sorted(AUTHORITY_TEST_ROOT.rglob("*.rs")):
        try:
            masked = mask_rust_non_code(path.read_text())
        except (OSError, ValueError) as error:
            errors.append(f"cannot inspect authority test source {relative(path)}: {error}")
            continue
        for loop in re.finditer(r"\b(?:while|for|loop)\b[^;{]*\{", masked, re.S):
            opening = masked.find("{", loop.start(), loop.end())
            closing = matching_brace(masked, opening)
            if closing is None:
                errors.append(f"cannot inspect loop ownership in {relative(path)}")
                continue
            body = masked[opening + 1 : closing]
            if (
                path != AUTHORITY_TEST_SUPPORT_PLAN
                and ".plan_dependency_maintenance(" in "".join(body.split())
            ):
                errors.append(
                    "dependency maintenance loop bypasses the rank-derived drain in "
                    f"{relative(path)}"
                )
    return errors


def validate_test_worker_ownership() -> list[str]:
    """Keep long-running authority test tasks under one structured owner."""

    try:
        source = AUTHORITY_TEST_SUPPORT_WORKER.read_text()
        spawn_maintenance = function_body(source, "spawn_maintenance")
        spawn_observed = function_body(source, "spawn_observed_maintenance")
        shutdown = function_body(source, "shutdown")
        request_stop = function_body(source, "request_stop")
        drop_owner = function_body(source, "drop")
    except (OSError, ValueError) as error:
        return [f"cannot inspect structured test-worker ownership: {error}"]
    bodies = (spawn_maintenance, spawn_observed, shutdown, request_stop, drop_owner)
    if any(body is None for body in bodies):
        return ["the structured test-worker construction or teardown surface disappeared"]

    errors: list[str] = []
    for method, body in (
        ("spawn_maintenance", spawn_maintenance),
        ("spawn_observed_maintenance", spawn_observed),
    ):
        compact = "".join(mask_rust_non_code(body).split())
        reserve = compact.find(".try_reserve(1)")
        spawn = compact.find("handle.spawn(")
        if min(reserve, spawn) < 0 or reserve >= spawn:
            errors.append(f"{method} must reserve task ownership before spawning")
    errors.extend(
        require_compact_fragments(
            shutdown,
            "structured test-worker shutdown",
            (
                "self.request_stop()",
                "self.tasks.iter_mut().rev()",
                "tokio::time::timeout(TEST_WORKER_SHUTDOWN_TIMEOUT,&muttask.handle).await",
                "task.handle.abort()",
                "drop((&muttask.handle).await)",
                "self.tasks.clear()",
            ),
        )
    )
    errors.extend(
        require_compact_fragments(
            request_stop,
            "structured test-worker cancellation",
            ("ChunkCommand::Stop", "self.cancel.cancel()"),
        )
    )
    errors.extend(
        require_compact_fragments(
            drop_owner,
            "structured test-worker Drop",
            ("self.request_stop()", "task.handle.abort()"),
        )
    )

    spawn_worker_sites: list[Path] = []
    for path in sorted(AUTHORITY_TEST_ROOT.rglob("*.rs")):
        try:
            masked = mask_rust_non_code(path.read_text())
        except (OSError, ValueError) as error:
            errors.append(f"cannot inspect authority test source {relative(path)}: {error}")
            continue
        sites = list(re.finditer(r"\.\s*spawn_workers\s*\(", masked))
        spawn_worker_sites.extend(path for _site in sites)
        if path == AUTHORITY_TEST_SUPPORT_WORKER:
            continue
        if sites:
            errors.append(
                f"raw authority worker-generation spawn remains in {relative(path)}"
            )
        if re.search(r"\brun_maintenance_driver(?:_for_foundation)?\s*\(", masked):
            errors.append(f"raw maintenance-driver spawn remains in {relative(path)}")
        for retired in ("stop_worker_set", "join_workers", "AuthorityWorkerHandles"):
            if retired in masked:
                errors.append(
                    f"retired manual test-worker ownership {retired} remains in {relative(path)}"
                )
    if spawn_worker_sites != [AUTHORITY_TEST_SUPPORT_WORKER]:
        sites = [relative(path) for path in spawn_worker_sites]
        errors.append(f"expected one structured worker-generation constructor, found {sites}")
    return errors


def validate_scheduler_set_observation() -> list[str]:
    """Keep scheduler refinement independent of the production slot compiler."""

    try:
        source = AUTHORITY_TEST_SUPPORT_SCHEDULER.read_text()
        stored = function_body(source, "stored_set_observation")
        slots = function_body(source, "slots")
    except (OSError, ValueError) as error:
        return [f"cannot inspect scheduler set observation: {error}"]
    if stored is None or slots is None:
        return ["the stored scheduler set observation disappeared"]

    errors = require_compact_fragments(
        stored,
        "stored scheduler set observation",
        (
            "self.slots().into_iter()",
            "QueueKey::Resolve(key)",
            "QueueKey::Verify(key)",
            "SchedulerSlot::Ready(key)",
        ),
    )
    errors.extend(
        require_compact_fragments(
            slots,
            "stored scheduler set traversal",
            (
                "for(owner,entries)in&self.resolve.by_owner",
                "for(owner,entries)in&self.verify.by_owner",
                "self.ready.iter()",
            ),
        )
    )
    if "self.slot(" in "".join(mask_rust_non_code(stored + slots).split()):
        errors.append(
            "stored scheduler set observation must not reuse the production slot compiler"
        )
    return errors


def validate() -> list[str]:
    manifest = load_manifest()
    errors: list[str] = []
    if manifest.get("schema_version") != 2:
        errors.append("test-layout manifest schema_version must be 2")
    for retired_field in ("module_wiring", "cfg_test_occurrences"):
        if retired_field in manifest:
            errors.append(
                f"test-layout manifest may not copy discoverable {retired_field}"
            )

    registry = load_registry()
    errors.extend(validate_registry(registry))
    known_behavior_ids = registered_behavior_ids(registry)

    try:
        allowed_roots = tuple(repo_path(path) for path in manifest["allowed_test_roots"])
        allowed_files = {repo_path(path) for path in manifest["allowed_test_files"]}
    except (KeyError, TypeError, ValueError) as error:
        return [f"invalid allowed test path declaration: {error}"]

    for path in (*allowed_roots, *allowed_files):
        if not path.exists():
            errors.append(f"declared test path does not exist: {relative(path)}")

    production_sources: dict[str, str] = {}
    for source in sorted(SOURCE_ROOT.rglob("*.rs")):
        text = source.read_text()
        if is_test_source(source, allowed_roots, allowed_files):
            continue
        name = relative(source)
        production_sources[name] = text
        if TEST_ATTRIBUTE.search(text):
            errors.append(f"test function remains in production source: {name}")
        if INLINE_TEST_MODULE.search(text):
            errors.append(f"inline test module remains in production source: {name}")

    # Production uses compiler lints for expression-aware indexing/arithmetic
    # and a conservative source gate for macros/APIs Clippy cannot forbid as a
    # family (notably assert and catch_unwind). The benchmark module is an
    # explicitly test-only fixture and owns its narrowly scoped lint allowance
    # at the module declaration in lib.rs; no runtime module may inherit it.
    runtime_sources = {
        name: text
        for name, text in production_sources.items()
        if name != "tx-pool/src/benchmark.rs"
    }
    for name, text in runtime_sources.items():
        for pattern, description in FORBIDDEN_PRODUCTION:
            for match in pattern.finditer(text):
                line = text.count("\n", 0, match.start()) + 1
                errors.append(
                    f"{description} remains in production source: {name}:{line}"
                )

    crate_root = runtime_sources.get("tx-pool/src/lib.rs", "")
    missing_lints = sorted(
        lint for lint in REQUIRED_STATIC_LINTS if lint not in crate_root
    )
    if missing_lints:
        errors.append(f"tx-pool production static lint gate is incomplete: {missing_lints}")
    for name, text in runtime_sources.items():
        for lint in REQUIRED_STATIC_LINTS:
            if re.search(
                rf"#!\s*\[allow\([^]]*{re.escape(lint)}", text, re.DOTALL
            ):
                errors.append(f"production source weakens static lint {lint}: {name}")

    # A live but unstarted tx-pool builder owns the receiving half of the
    # reliable capacity-one chain-transition channel. Sync's chain-only tests
    # must retire that capability through one named fixture boundary before
    # starting chain work; direct relay extraction can otherwise leave the
    # second best-tip transition blocked forever. Production-like relayer tests
    # start the tx-pool and are intentionally outside this directory.
    try:
        chain_only_fixture = SYNC_CHAIN_ONLY_FIXTURE.read_text()
    except OSError as error:
        errors.append(f"cannot read sync chain-only fixture: {error}")
        chain_only_fixture = ""
    for required in (
        "fn disable_tx_pool_and_take_relay_receiver",
        "pack.take_relay_tx_receiver()",
        "drop(pack.take_tx_pool_builder())",
    ):
        if required not in chain_only_fixture:
            errors.append(f"sync chain-only fixture lost required boundary: {required}")
    for source in sorted(SYNC_TEST_ROOT.rglob("*.rs")):
        if source == SYNC_CHAIN_ONLY_FIXTURE:
            continue
        text = source.read_text()
        if "pack.take_relay_tx_receiver()" in text:
            errors.append(
                "sync chain-only test bypasses the named tx-pool retirement "
                f"boundary: {relative(source)}"
            )

    discovered_wiring: set[tuple[str, str, str | None]] = set()
    wiring_spans: dict[str, list[tuple[int, int]]] = {}
    for file, text in production_sources.items():
        for match in TEST_MODULE_WIRING.finditer(text):
            module_path = match.group(1)
            module = match.group(2)
            key = (file, module, module_path)
            if key in discovered_wiring:
                errors.append(f"duplicate test module wiring: {key}")
                continue
            discovered_wiring.add(key)
            wiring_spans.setdefault(file, []).append(match.span())
            source = repo_path(file)
            target = expected_module_target(source, module, module_path)
            if not target.is_file():
                errors.append(
                    f"test module {file}::{module} targets missing file {relative(target)}"
                )
            elif not is_test_source(target, allowed_roots, allowed_files):
                errors.append(
                    f"test module {file}::{module} targets non-test path {relative(target)}"
                )

    seam_keys: set[tuple[str, str]] = set()
    seam_identifiers: dict[str, set[str]] = {}
    for entry in manifest.get("seams", []):
        file = entry.get("file")
        symbols = entry.get("symbols")
        kind = entry.get("kind")
        behavior_ids = entry.get("behavior_ids")
        if not isinstance(file, str) or not isinstance(symbols, list) or not symbols:
            errors.append(f"invalid seam file/symbol list: {entry!r}")
            continue
        if not isinstance(kind, str) or not kind.strip():
            errors.append(f"test seam has no reviewable kind: {file} {symbols}")
        if not isinstance(behavior_ids, list) or not behavior_ids:
            errors.append(f"test seam has no behavior IDs: {file} {symbols}")
        else:
            for behavior_id in behavior_ids:
                if not isinstance(behavior_id, str) or not BEHAVIOR_ID.fullmatch(behavior_id):
                    errors.append(f"invalid behavior ID {behavior_id!r} for {file}")
                elif behavior_id not in known_behavior_ids:
                    errors.append(f"unknown behavior ID {behavior_id!r} for {file}")
        source = production_sources.get(file)
        if source is None:
            errors.append(f"test seam points outside production source: {file}")
            continue
        for symbol in symbols:
            if not isinstance(symbol, str) or not symbol:
                errors.append(f"invalid test seam symbol in {file}: {symbol!r}")
                continue
            key = (file, symbol)
            if key in seam_keys:
                errors.append(f"duplicate test seam symbol: {file}::{symbol}")
            seam_keys.add(key)
            identifier = symbol.rsplit("::", 1)[-1]
            if re.search(rf"\b{re.escape(identifier)}\b", source) is None:
                errors.append(f"test seam symbol disappeared: {file}::{symbol}")
            else:
                seam_identifiers.setdefault(file, set()).add(identifier)

    # Module wiring is discovered and validated above. Every other cfg(test)
    # occurrence must be an exact named seam. A copied per-file count would
    # merely bless current drift, so the manifest has no count baseline.
    for file, text in production_sources.items():
        spans = wiring_spans.get(file, [])
        identifiers = seam_identifiers.get(file, set())
        for match in CFG_TEST.finditer(text):
            if any(start <= match.start() < end for start, end in spans):
                continue
            window = text[match.end() : match.end() + 320]
            if any(
                re.search(rf"\b{re.escape(identifier)}\b", window)
                for identifier in identifiers
            ):
                continue
            line = text.count("\n", 0, match.start()) + 1
            errors.append(
                f"test-only production item must move to a dedicated test file or "
                f"become a named irreducible seam: {file}:{line}"
            )

    errors.extend(validate_dependency_progress_layout())
    errors.extend(validate_test_worker_ownership())
    errors.extend(validate_scheduler_set_observation())
    return errors


def main() -> int:
    errors = validate()
    if errors:
        for error in errors:
            print(f"error: {error}", file=sys.stderr)
        return 1
    manifest = load_manifest()
    roots = tuple(repo_path(root) for root in manifest["allowed_test_roots"])
    files = {repo_path(file) for file in manifest["allowed_test_files"]}
    module_wires = sum(
        len(TEST_MODULE_WIRING.findall(path.read_text()))
        for path in SOURCE_ROOT.rglob("*.rs")
        if not is_test_source(path, roots, files)
    )
    print(
        "validated tx-pool test isolation and production static safety: "
        f"{module_wires} discovered module wires, "
        f"{sum(len(entry['symbols']) for entry in manifest['seams'])} named seams, "
        "rank-derived dependency drains, structured worker ownership and an independent "
        "stored scheduler observation"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
