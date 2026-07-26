#!/usr/bin/env python3
"""Enforce tx-pool test isolation and explicit test-only seam review."""

from __future__ import annotations

from collections import Counter
import json
from pathlib import Path
import re
import sys

from check_tx_pool_review_guide import (
    behavior_ids as registered_behavior_ids,
    load_registry,
    validate_registry,
)


REPO_ROOT = Path(__file__).resolve().parents[1]
SOURCE_ROOT = REPO_ROOT / "tx-pool" / "src"
MANIFEST_PATH = REPO_ROOT / "tx-pool" / "test-layout-manifest.json"
CFG_TEST = "#[cfg(test)]"
TEST_ATTRIBUTE = re.compile(r"(?m)^\s*#\[(?:tokio::)?test(?:\([^]]*\))?\]")
INLINE_TEST_MODULE = re.compile(
    r"(?m)^\s*(?:#\[cfg\(test\)\]\s*)?"
    r"mod\s+(?:tests|[A-Za-z0-9_]*(?:_tests|test_seam))\s*\{"
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
    base = source.parent if source.name == "mod.rs" else source.parent / source.stem
    direct = base / f"{module}.rs"
    nested = base / module / "mod.rs"
    if direct.exists():
        return direct.resolve()
    return nested.resolve()


def validate() -> list[str]:
    manifest = load_manifest()
    errors: list[str] = []
    if manifest.get("schema_version") != 1:
        errors.append("test-layout manifest schema_version must be 1")

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

    expected_wiring: set[tuple[str, str, str | None]] = set()
    for entry in manifest.get("module_wiring", []):
        try:
            file = entry["file"]
            module = entry["module"]
            module_path = entry.get("path")
        except (KeyError, TypeError) as error:
            errors.append(f"invalid module wiring entry {entry!r}: {error}")
            continue
        key = (file, module, module_path)
        if key in expected_wiring:
            errors.append(f"duplicate module wiring declaration: {key}")
            continue
        expected_wiring.add(key)
        try:
            source = repo_path(file)
        except ValueError as error:
            errors.append(str(error))
            continue
        target = expected_module_target(source, module, module_path)
        if not target.is_file():
            errors.append(
                f"test module {file}::{module} targets missing file {relative(target)}"
            )
        elif not is_test_source(target, allowed_roots, allowed_files):
            errors.append(
                f"test module {file}::{module} targets non-test path {relative(target)}"
            )

    discovered_wiring: set[tuple[str, str, str | None]] = set()
    for file, text in production_sources.items():
        for match in TEST_MODULE_WIRING.finditer(text):
            discovered_wiring.add((file, match.group(2), match.group(1)))
    missing_wiring = sorted(expected_wiring - discovered_wiring)
    extra_wiring = sorted(discovered_wiring - expected_wiring)
    if missing_wiring:
        errors.append(f"declared test module wiring disappeared: {missing_wiring}")
    if extra_wiring:
        errors.append(f"unreviewed test module wiring appeared: {extra_wiring}")

    expected_cfg = manifest.get("cfg_test_occurrences", {})
    actual_cfg = {
        file: text.count(CFG_TEST)
        for file, text in production_sources.items()
        if CFG_TEST in text
    }
    if actual_cfg != expected_cfg:
        missing = {
            file: count
            for file, count in expected_cfg.items()
            if actual_cfg.get(file) != count
        }
        extra = {
            file: count
            for file, count in actual_cfg.items()
            if expected_cfg.get(file) != count
        }
        if missing:
            errors.append(f"cfg(test) occurrence baseline changed: expected {missing}")
        if extra:
            errors.append(f"cfg(test) occurrence baseline changed: actual {extra}")

    seam_keys: set[tuple[str, str]] = set()
    seam_files = Counter()
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
        seam_files[file] += 1
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

    # Every production file with cfg(test) must be explained by module wiring,
    # a named seam, or a cfg(test)-only import/initializer supporting one of
    # those named surfaces. The exact per-file occurrence count then makes any
    # addition or removal a manifest-reviewed change.
    explained_files = {file for file, _, _ in expected_wiring} | set(seam_files)
    unexplained = sorted(set(expected_cfg) - explained_files)
    if unexplained:
        errors.append(f"cfg(test) files lack wiring or seam ownership: {unexplained}")

    return errors


def main() -> int:
    errors = validate()
    if errors:
        for error in errors:
            print(f"error: {error}", file=sys.stderr)
        return 1
    manifest = load_manifest()
    print(
        "validated tx-pool test isolation and production static safety: "
        f"{len(manifest['module_wiring'])} module wires, "
        f"{sum(manifest['cfg_test_occurrences'].values())} cfg(test) sites, "
        f"{sum(len(entry['symbols']) for entry in manifest['seams'])} named seams"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
