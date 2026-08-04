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


REPO_ROOT = Path(__file__).resolve().parents[2]
SOURCE_ROOT = REPO_ROOT / "tx-pool" / "src"
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
        f"{sum(len(entry['symbols']) for entry in manifest['seams'])} named seams"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
