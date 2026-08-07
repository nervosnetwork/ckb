#!/usr/bin/env python3
"""Derive the model/production refinement frontier from semantic Rust roots."""

from __future__ import annotations

import argparse
from bisect import bisect_right
from collections import Counter
from dataclasses import dataclass
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile

from check_production_contracts import mask_rust_non_code, matching_brace


REPO_ROOT = Path(__file__).resolve().parents[2]
CONTRACT = REPO_ROOT / "tx-pool" / "architecture-contract.json"
BEHAVIOR_REGISTRY = REPO_ROOT / "tx-pool" / "review-behaviors.json"
CANARY = REPO_ROOT / "tx-pool" / "scripts" / "fixtures" / "model_refinement_canary.rs"
IDENTIFIER = re.compile(r"\b[A-Za-z_][A-Za-z0-9_]*\b")
TYPE_DECLARATION = re.compile(r"\b(?P<kind>enum|struct)\s+(?P<name>[A-Za-z_][A-Za-z0-9_]*)\b")
METHOD_DECLARATION = re.compile(
    r"(?:pub(?:\s*\([^)]*\))?\s+)?"
    r"(?:async\s+)?(?:const\s+)?(?:unsafe\s+)?"
    r"fn\s+(?P<name>[A-Za-z_][A-Za-z0-9_]*)\b"
)
SYNC_PRIMITIVES = (
    "CancellationToken",
    "JoinHandle",
    "Mutex",
    "Notify",
    "RwLock",
    "Semaphore",
    "mpsc",
    "oneshot",
    "watch",
)


@dataclass(frozen=True)
class Method:
    name: str
    path: str
    line: int
    signature: str
    source: str


@dataclass(frozen=True)
class TypeDeclaration:
    name: str
    kind: str
    path: str
    line: int
    source: str
    variants: tuple[str, ...]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--json",
        action="store_true",
        help="print the complete derived inventory instead of the compact gate result",
    )
    parser.add_argument(
        "--variant-flow",
        action="store_true",
        help=(
            "derive enum-variant producer/consumer sites with ast-grep; this is "
            "the slower M3 refinement gate"
        ),
    )
    expansion = parser.add_mutually_exclusive_group()
    expansion.add_argument(
        "--expanded-production",
        type=Path,
        help=(
            "cargo-expand output for production macro-producer evidence; requires "
            "--variant-flow"
        ),
    )
    expansion.add_argument(
        "--cargo-expand-production",
        action="store_true",
        help=(
            "run cargo expand for current production macro evidence; requires "
            "--variant-flow"
        ),
    )
    return parser.parse_args()


def repo_path(value: str) -> Path:
    path = (REPO_ROOT / value).resolve()
    try:
        path.relative_to(REPO_ROOT)
    except ValueError as error:
        raise ValueError(f"path escapes repository root: {value}") from error
    return path


def rust_sources(values: object, *, exclude_tests: bool) -> tuple[list[Path], list[str]]:
    if not isinstance(values, list) or not values or not all(
        isinstance(value, str) for value in values
    ):
        return [], ["refinement source roots must be a non-empty string list"]
    sources: set[Path] = set()
    errors: list[str] = []
    for value in values:
        try:
            root = repo_path(value)
        except ValueError as error:
            errors.append(str(error))
            continue
        if root.is_file():
            candidates = [root] if root.suffix == ".rs" else []
        elif root.is_dir():
            candidates = sorted(root.rglob("*.rs"))
        else:
            errors.append(f"refinement source root does not exist: {value}")
            continue
        for candidate in candidates:
            relative = candidate.relative_to(REPO_ROOT)
            if exclude_tests and "tests" in relative.parts:
                continue
            sources.add(candidate)
    if not sources:
        errors.append("refinement source roots discover no Rust files")
    return sorted(sources), errors


def find_declaration_body(masked: str, offset: int) -> tuple[int, int] | None:
    angle_depth = 0
    cursor = offset
    while cursor < len(masked):
        character = masked[cursor]
        if character == "<":
            angle_depth += 1
        elif character == ">" and angle_depth:
            angle_depth -= 1
        elif angle_depth == 0 and character == "{":
            closing = matching_brace(masked, cursor)
            return None if closing is None else (cursor, closing)
        elif angle_depth == 0 and character == "(":
            depth = 1
            end = cursor + 1
            while end < len(masked) and depth:
                if masked[end] == "(":
                    depth += 1
                elif masked[end] == ")":
                    depth -= 1
                end += 1
            return None if depth else (cursor, end - 1)
        elif angle_depth == 0 and character == ";":
            return (cursor, cursor)
        cursor += 1
    return None


def top_level_segments(body: str) -> list[str]:
    segments: list[str] = []
    start = 0
    braces = parentheses = brackets = angles = 0
    for index, character in enumerate(body):
        if character == "{":
            braces += 1
        elif character == "}":
            braces -= 1
        elif character == "(":
            parentheses += 1
        elif character == ")":
            parentheses -= 1
        elif character == "[":
            brackets += 1
        elif character == "]":
            brackets -= 1
        elif character == "<":
            angles += 1
        elif character == ">" and angles:
            angles -= 1
        elif (
            character == ","
            and braces == 0
            and parentheses == 0
            and brackets == 0
            and angles == 0
        ):
            segments.append(body[start:index])
            start = index + 1
    segments.append(body[start:])
    return segments


def enum_variants(body: str) -> tuple[str, ...]:
    variants: list[str] = []
    for segment in top_level_segments(body):
        names = re.findall(
            r"(?m)^\s*(?:#\[[^\n]*\]\s*)*([A-Z][A-Za-z0-9_]*)\b",
            segment,
        )
        if names:
            variants.append(names[0])
    return tuple(variants)


def declaration_key(path: str, name: str) -> str:
    return f"{path}::{name}"


def declarations(paths: list[Path]) -> tuple[dict[str, TypeDeclaration], list[str]]:
    discovered: dict[str, TypeDeclaration] = {}
    errors: list[str] = []
    for path in paths:
        source = path.read_text()
        masked = mask_rust_non_code(source)
        for match in TYPE_DECLARATION.finditer(masked):
            body_range = find_declaration_body(masked, match.end())
            if body_range is None:
                errors.append(
                    f"cannot parse {match.group('kind')} {match.group('name')} in "
                    f"{path.relative_to(REPO_ROOT)}"
                )
                continue
            opening, closing = body_range
            body = masked[opening + 1 : closing] if opening != closing else ""
            name = match.group("name")
            relative = path.relative_to(REPO_ROOT).as_posix()
            declaration = TypeDeclaration(
                name=name,
                kind=match.group("kind"),
                path=relative,
                line=source.count("\n", 0, match.start()) + 1,
                source=body,
                variants=enum_variants(body) if match.group("kind") == "enum" else (),
            )
            key = declaration_key(relative, name)
            if key in discovered:
                errors.append(f"duplicate refinement declaration {key}")
            else:
                discovered[key] = declaration
    return discovered, errors


def type_impls(path: Path) -> list[tuple[str, str, int]]:
    source = path.read_text()
    masked = mask_rust_non_code(source)
    impls: list[tuple[str, str, int]] = []
    for declaration in re.finditer(r"\bimpl\b", masked):
        opening = masked.find("{", declaration.end())
        if opening < 0:
            continue
        header = masked[declaration.end() : opening]
        header = header.strip()
        if header.startswith("<"):
            depth = 0
            end = None
            for index, character in enumerate(header):
                if character == "<":
                    depth += 1
                elif character == ">":
                    depth -= 1
                    if depth == 0:
                        end = index + 1
                        break
            if end is None:
                continue
            header = header[end:].lstrip()
        target = re.split(r"\bfor\b", header)[-1]
        target = target.split(" where ", 1)[0].strip()
        target = target.split("<", 1)[0]
        owner_names = IDENTIFIER.findall(target)
        if not owner_names:
            continue
        closing = matching_brace(masked, opening)
        if closing is None:
            continue
        impls.append(
            (
                owner_names[-1],
                masked[opening + 1 : closing],
                source.count("\n", 0, opening) + 1,
            )
        )
    return impls


def impl_ranges(path: Path) -> list[tuple[int, int, str]]:
    """Return source ranges whose `Self` is bound to one concrete impl owner."""
    source = path.read_text()
    masked = mask_rust_non_code(source)
    ranges: list[tuple[int, int, str]] = []
    for declaration in re.finditer(r"\bimpl\b", masked):
        opening = masked.find("{", declaration.end())
        if opening < 0:
            continue
        header = masked[declaration.end() : opening].strip()
        if header.startswith("<"):
            depth = 0
            end = None
            for index, character in enumerate(header):
                if character == "<":
                    depth += 1
                elif character == ">":
                    depth -= 1
                    if depth == 0:
                        end = index + 1
                        break
            if end is None:
                continue
            header = header[end:].lstrip()
        target = re.split(r"\bfor\b", header)[-1]
        target = target.split(" where ", 1)[0].strip()
        target = target.split("<", 1)[0]
        owner_names = IDENTIFIER.findall(target)
        closing = matching_brace(masked, opening)
        if owner_names and closing is not None:
            ranges.append((opening + 1, closing, owner_names[-1]))
    return sorted(ranges)


def ast_grep_ranges(paths: list[Path]) -> tuple[dict[Path, list[tuple[int, int]]], list[str]]:
    executable = shutil.which("ast-grep")
    if executable is None:
        return {}, ["--variant-flow requires ast-grep on PATH"]
    ranges: dict[Path, list[tuple[int, int]]] = {path.resolve(): [] for path in paths}
    errors: list[str] = []
    for kind in ("match_pattern", "tuple_struct_pattern", "struct_pattern"):
        command = [
            executable,
            "run",
            "--lang",
            "rust",
            "--kind",
            kind,
            "--json=stream",
            *[str(path) for path in paths],
        ]
        result = subprocess.run(
            command,
            cwd=REPO_ROOT,
            check=False,
            capture_output=True,
            text=True,
        )
        if result.returncode not in (0, 1) or (result.returncode == 1 and result.stderr):
            errors.append(
                f"ast-grep {kind} failed ({result.returncode}): {result.stderr.strip()}"
            )
            continue
        for line in result.stdout.splitlines():
            item = json.loads(line)
            path = (REPO_ROOT / item["file"]).resolve()
            offsets = item["range"]["byteOffset"]
            ranges.setdefault(path, []).append((offsets["start"], offsets["end"]))

    # A let-condition includes its RHS. Only the pattern before the first
    # top-level `=` consumes a variant; an enum value on the RHS produces one.
    command = [
        executable,
        "run",
        "--lang",
        "rust",
        "--kind",
        "let_condition",
        "--json=stream",
        *[str(path) for path in paths],
    ]
    result = subprocess.run(
        command,
        cwd=REPO_ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode not in (0, 1) or (result.returncode == 1 and result.stderr):
        errors.append(
            f"ast-grep let_condition failed ({result.returncode}): "
            f"{result.stderr.strip()}"
        )
    else:
        for line in result.stdout.splitlines():
            item = json.loads(line)
            path = (REPO_ROOT / item["file"]).resolve()
            offsets = item["range"]["byteOffset"]
            text = item["text"]
            equals = top_level_equals(text)
            if equals is not None:
                start = offsets["start"]
                ranges.setdefault(path, []).append((start, start + equals))

    for path, path_ranges in ranges.items():
        path_ranges.sort(key=lambda value: (value[0], -value[1]))
        maximal: list[tuple[int, int]] = []
        for start, end in path_ranges:
            if maximal and end <= maximal[-1][1]:
                continue
            maximal.append((start, end))
        ranges[path] = maximal
    return ranges, errors


def top_level_equals(source: str) -> int | None:
    parentheses = brackets = braces = angles = 0
    for index, character in enumerate(source):
        if character == "(":
            parentheses += 1
        elif character == ")":
            parentheses -= 1
        elif character == "[":
            brackets += 1
        elif character == "]":
            brackets -= 1
        elif character == "{":
            braces += 1
        elif character == "}":
            braces -= 1
        elif character == "<":
            angles += 1
        elif character == ">" and angles:
            angles -= 1
        elif (
            character == "="
            and parentheses == 0
            and brackets == 0
            and braces == 0
            and angles == 0
        ):
            return index
    return None


def variant_flow(
    paths: list[Path], declarations_by_name: dict[str, TypeDeclaration]
) -> tuple[dict[tuple[str, str], dict[str, object]], list[str]]:
    enum_names = {
        declaration.name: set(declaration.variants)
        for declaration in declarations_by_name.values()
        if declaration.kind == "enum"
    }
    duplicate_names = [
        name
        for name in enum_names
        if sum(
            declaration.kind == "enum" and declaration.name == name
            for declaration in declarations_by_name.values()
        )
        > 1
    ]
    if duplicate_names:
        return {}, [
            "variant-flow requires path disambiguation for duplicate enum names: "
            f"{sorted(duplicate_names)}"
        ]
    pattern_ranges, errors = ast_grep_ranges(paths)
    if errors:
        return {}, errors
    flow: dict[tuple[str, str], dict[str, object]] = {
        (name, variant): {"producers": [], "consumers": []}
        for name, variants in enum_names.items()
        for variant in variants
    }
    # The lookahead permits overlapping path pairs, so
    # `module::Enum::Variant` yields both `module::Enum` and the semantically
    # relevant `Enum::Variant` instead of losing the latter.
    reference = re.compile(
        r"(?=\b(?P<owner>Self|[A-Za-z_][A-Za-z0-9_]*)\s*::\s*"
        r"(?P<variant>[A-Z][A-Za-z0-9_]*)\b)"
    )
    for path in paths:
        resolved = path.resolve()
        source = path.read_text()
        masked = mask_rust_non_code(source)
        ranges = pattern_ranges.get(resolved, [])
        starts = [start for start, _ in ranges]
        owners = impl_ranges(path)
        owner_starts = [start for start, _, _ in owners]
        for match in reference.finditer(masked):
            line_start = masked.rfind("\n", 0, match.start()) + 1
            line = masked[line_start : masked.find("\n", match.end())]
            if re.match(r"\s*(?:pub\s+)?use\b", line):
                continue
            owner = match.group("owner")
            if owner == "Self":
                index = bisect_right(owner_starts, match.start()) - 1
                if index < 0 or match.start() >= owners[index][1]:
                    continue
                owner = owners[index][2]
            variant = match.group("variant")
            if variant not in enum_names.get(owner, set()):
                continue
            range_index = bisect_right(starts, match.start()) - 1
            consumer = range_index >= 0 and match.start() < ranges[range_index][1]
            site = {
                "path": (
                    path.relative_to(REPO_ROOT).as_posix()
                    if path.is_relative_to(REPO_ROOT)
                    else str(path)
                ),
                "line": source.count("\n", 0, match.start()) + 1,
            }
            key = "consumers" if consumer else "producers"
            flow[(owner, variant)][key].append(site)
    return flow, []


def construction_flow(
    paths: list[Path], declarations_by_name: dict[str, TypeDeclaration]
) -> tuple[dict[str, list[dict[str, object]]], list[str]]:
    """Derive explicit struct/newtype construction sites from Rust syntax."""
    executable = shutil.which("ast-grep")
    if executable is None:
        return {}, ["--variant-flow requires ast-grep on PATH"]
    struct_keys_by_name: dict[str, list[str]] = {}
    for key, declaration in declarations_by_name.items():
        if declaration.kind == "struct":
            struct_keys_by_name.setdefault(declaration.name, []).append(key)
    flow: dict[str, list[dict[str, object]]] = {
        key: []
        for keys in struct_keys_by_name.values()
        for key in keys
    }
    pattern_ranges, errors = ast_grep_ranges(paths)
    if errors:
        return {}, errors
    construction = re.compile(
        r"\b(?P<owner>Self|[A-Z][A-Za-z0-9_]*)\s*"
        r"(?P<syntax>\{|\(|::\s*default\s*\()"
    )
    for path in paths:
        source = path.read_text()
        masked = mask_rust_non_code(source)
        owners = impl_ranges(path)
        owner_starts = [start for start, _, _ in owners]
        ranges = pattern_ranges.get(path.resolve(), [])
        range_starts = [start for start, _ in ranges]
        for match in construction.finditer(masked):
            range_index = bisect_right(range_starts, match.start()) - 1
            if range_index >= 0 and match.start() < ranges[range_index][1]:
                continue
            line_start = masked.rfind("\n", 0, match.start()) + 1
            prefix = masked[line_start : match.start()]
            if re.search(
                r"\b(?:struct|enum|type|for)\s*$|\bimpl(?:\s*<[^>\n]*>)?\s*$",
                prefix,
            ):
                continue
            owner = match.group("owner")
            offset = match.start()
            if owner == "Self":
                index = bisect_right(owner_starts, offset) - 1
                if index < 0 or offset >= owners[index][1]:
                    continue
                owner = owners[index][2]
            candidates = struct_keys_by_name.get(owner, [])
            if not candidates:
                continue
            relative = (
                path.relative_to(REPO_ROOT).as_posix()
                if path.is_relative_to(REPO_ROOT)
                else None
            )
            if len(candidates) == 1:
                key = candidates[0]
            else:
                local = [
                    candidate
                    for candidate in candidates
                    if relative is not None
                    and declarations_by_name[candidate].path == relative
                ]
                if len(local) != 1:
                    continue
                key = local[0]
            site = {
                "path": relative if relative is not None else str(path),
                "line": source.count("\n", 0, offset) + 1,
            }
            if site not in flow[key]:
                flow[key].append(site)
    return flow, errors


def validate_canary(*, include_variant_flow: bool) -> list[str]:
    """Prove that both syntax discovery and the negative binding gate can fail."""
    discovered, errors = declarations([CANARY])
    if errors:
        return [f"refinement canary declaration error: {error}" for error in errors]
    expected = {
        declaration_key(
            "tx-pool/scripts/fixtures/model_refinement_canary.rs", name
        )
        for name in (
            "CanaryPayload",
            "CanaryEvent",
            "CanaryBoundary",
            "CanaryUnconstructedCapability",
        )
    }
    if set(discovered) != expected:
        errors.append(
            "refinement canary parser drift: "
            f"expected {sorted(expected)}, discovered {sorted(discovered)}"
        )
        return errors
    event = discovered[
        declaration_key(
            "tx-pool/scripts/fixtures/model_refinement_canary.rs", "CanaryEvent"
        )
    ]
    if event.variants != ("Tuple", "Struct", "Unit"):
        errors.append(
            "refinement canary enum drift: "
            f"expected Tuple/Struct/Unit, discovered {list(event.variants)}"
        )
    event_methods = {method.name for method in methods([CANARY])["CanaryEvent"]}
    if event_methods != {"tuple", "structured", "unit", "consume"}:
        errors.append(
            "refinement canary method drift: "
            f"discovered {sorted(event_methods)}"
        )

    path = "tx-pool/scripts/fixtures/model_refinement_canary.rs"
    model_roots, root_errors = validate_roots(
        {
            f"{path}::CanaryEvent": "bound_event",
            f"{path}::CanaryBoundary": "deliberately_unbound_boundary",
        },
        discovered,
        "canary_model_roots",
    )
    production_roots, production_root_errors = validate_roots(
        {f"{path}::CanaryEvent": "bound_production_event"},
        discovered,
        "canary_production_roots",
    )
    errors.extend(root_errors)
    errors.extend(production_root_errors)
    if errors:
        return errors
    negative = {
        "canary": {
            "model_roles": ["bound_event"],
            "production_roles": ["bound_production_event"],
            "behavior_ids": ["TP-CANARY"],
        }
    }
    _, _, _, negative_errors = validate_bindings(
        negative, model_roots, production_roots, {"TP-CANARY"}
    )
    expected_error = (
        "unbound refinement model roles: ['deliberately_unbound_boundary']"
    )
    if negative_errors != [expected_error]:
        errors.append(
            "refinement negative-binding canary did not fail exactly: "
            f"{negative_errors}"
        )

    positive = {
        "canary": {
            "model_roles": ["bound_event", "deliberately_unbound_boundary"],
            "production_roles": ["bound_production_event"],
            "behavior_ids": ["TP-CANARY"],
        }
    }
    _, _, _, positive_errors = validate_bindings(
        positive, model_roots, production_roots, {"TP-CANARY"}
    )
    if positive_errors:
        errors.append(
            f"refinement positive-binding canary failed: {positive_errors}"
        )
    if include_variant_flow:
        flow, flow_errors = variant_flow([CANARY], discovered)
        errors.extend(flow_errors)
        for variant in event.variants:
            evidence = flow.get(("CanaryEvent", variant), {})
            if not evidence.get("producers") or not evidence.get("consumers"):
                errors.append(
                    "refinement variant-flow canary lacks both directions for "
                    f"CanaryEvent::{variant}: {evidence}"
                )
        constructors, constructor_errors = construction_flow([CANARY], discovered)
        errors.extend(constructor_errors)
        boundary_key = declaration_key(path, "CanaryBoundary")
        unconstructed_key = declaration_key(path, "CanaryUnconstructedCapability")
        if not constructors.get(boundary_key):
            errors.append("refinement construction-flow canary lost its positive witness")
        if constructors.get(unconstructed_key):
            errors.append("refinement construction-flow negative canary did not fail")
    return errors


def methods(paths: list[Path]) -> dict[str, list[Method]]:
    discovered: dict[str, list[Method]] = {}
    for path in paths:
        relative = path.relative_to(REPO_ROOT).as_posix()
        for owner, body, impl_line in type_impls(path):
            depth = 0
            depth_at = [0] * (len(body) + 1)
            for index, character in enumerate(body):
                depth_at[index] = depth
                if character == "{":
                    depth += 1
                elif character == "}":
                    depth -= 1
            depth_at[len(body)] = depth
            for match in METHOD_DECLARATION.finditer(body):
                if depth_at[match.start()] != 0:
                    continue
                opening = body.find("{", match.end())
                if opening < 0:
                    continue
                closing = matching_brace(body, opening)
                if closing is None:
                    continue
                method_source = body[match.start() : closing + 1]
                signature = body[match.start() : opening]
                line = impl_line + body.count("\n", 0, match.start())
                discovered.setdefault(owner, []).append(
                    Method(match.group("name"), relative, line, signature, method_source)
                )
    for owner in discovered:
        discovered[owner].sort(key=lambda method: (method.path, method.line, method.name))
    return discovered


def validate_roots(
    value: object, declarations_by_name: dict[str, TypeDeclaration], field: str
) -> tuple[list[dict[str, str]], list[str]]:
    if not isinstance(value, dict) or not value:
        return [], [f"refinement {field} must be a non-empty mapping"]
    roots: list[dict[str, str]] = []
    errors: list[str] = []
    roles: set[str] = set()
    names: set[str] = set()
    for qualified, role in value.items():
        if not isinstance(qualified, str) or not isinstance(role, str) or not role:
            errors.append(
                f"refinement {field} contains an invalid root: {qualified!r}: {role!r}"
            )
            continue
        try:
            path, type_name = qualified.rsplit("::", 1)
        except ValueError:
            errors.append(
                f"refinement {field} root must be path::Type: {qualified!r}"
            )
            continue
        root = {"path": path, "type": type_name, "role": role}
        declaration = declarations_by_name.get(
            declaration_key(root["path"], root["type"])
        )
        if declaration is None:
            errors.append(f"refinement {field} type is absent: {root['type']}")
        elif declaration.path != root["path"]:
            errors.append(
                f"refinement {field} path mismatch for {root['type']}: "
                f"contract={root['path']}, Rust={declaration.path}"
            )
        if root["role"] in roles:
            errors.append(f"refinement {field} repeats role {root['role']!r}")
        if root["type"] in names:
            errors.append(f"refinement {field} repeats type {root['type']!r}")
        roles.add(root["role"])
        names.add(root["type"])
        roots.append(root)
    roots.sort(key=lambda root: (root["path"], root["type"], root["role"]))
    return roots, errors


def validate_bindings(
    value: object,
    model_roots: list[dict[str, str]],
    production_roots: list[dict[str, str]],
    behavior_ids: set[str],
) -> tuple[dict[str, object], dict[str, str], dict[str, str], list[str]]:
    if not isinstance(value, dict) or not value:
        return {}, {}, {}, ["refinement semantic_bindings must be a non-empty mapping"]
    known_model = {root["role"] for root in model_roots}
    known_production = {root["role"] for root in production_roots}
    model_owner: dict[str, str] = {}
    production_owner: dict[str, str] = {}
    normalized: dict[str, object] = {}
    errors: list[str] = []
    for binding, entry in value.items():
        if not isinstance(binding, str) or not binding:
            errors.append(f"refinement binding has invalid name {binding!r}")
            continue
        if not isinstance(entry, dict) or set(entry) != {
            "model_roles",
            "production_roles",
            "behavior_ids",
        }:
            errors.append(f"refinement binding {binding!r} has an invalid shape")
            continue
        lists: dict[str, list[str]] = {}
        for field in ("model_roles", "production_roles", "behavior_ids"):
            items = entry.get(field)
            if not isinstance(items, list) or not items or not all(
                isinstance(item, str) and item for item in items
            ):
                errors.append(
                    f"refinement binding {binding!r} {field} must be a non-empty string list"
                )
                lists[field] = []
                continue
            if items != sorted(items) or len(items) != len(set(items)):
                errors.append(
                    f"refinement binding {binding!r} {field} must be sorted and unique"
                )
            lists[field] = items
        for role in lists["model_roles"]:
            if role not in known_model:
                errors.append(
                    f"refinement binding {binding!r} uses unknown model role {role!r}"
                )
            elif role in model_owner:
                errors.append(
                    f"model role {role!r} is bound by both {model_owner[role]!r} and "
                    f"{binding!r}"
                )
            else:
                model_owner[role] = binding
        for role in lists["production_roles"]:
            if role not in known_production:
                errors.append(
                    f"refinement binding {binding!r} uses unknown production role {role!r}"
                )
            elif role in production_owner:
                errors.append(
                    f"production role {role!r} is bound by both "
                    f"{production_owner[role]!r} and {binding!r}"
                )
            else:
                production_owner[role] = binding
        unknown_behaviors = set(lists["behavior_ids"]).difference(behavior_ids)
        if unknown_behaviors:
            errors.append(
                f"refinement binding {binding!r} uses unknown behaviors "
                f"{sorted(unknown_behaviors)}"
            )
        normalized[binding] = lists
    missing_model = known_model.difference(model_owner)
    missing_production = known_production.difference(production_owner)
    if missing_model:
        errors.append(f"unbound refinement model roles: {sorted(missing_model)}")
    if missing_production:
        errors.append(
            f"unbound refinement production roles: {sorted(missing_production)}"
        )
    return normalized, model_owner, production_owner, errors


def reachable_inventory(
    roots: list[dict[str, str]],
    declarations_by_name: dict[str, TypeDeclaration],
    methods_by_owner: dict[str, list[Method]],
    declaration_paths: list[Path],
    reference_paths: list[Path],
    role_bindings: dict[str, str],
    source_variant_flow: dict[tuple[str, str], dict[str, object]] | None = None,
    expanded_variant_flow: dict[tuple[str, str], dict[str, object]] | None = None,
    source_construction_flow: dict[str, list[dict[str, object]]] | None = None,
    expanded_construction_flow: dict[str, list[dict[str, object]]] | None = None,
) -> dict[str, object]:
    names: dict[str, set[str]] = {}
    for key, declaration in declarations_by_name.items():
        names.setdefault(declaration.name, set()).add(key)

    def type_methods(key: str) -> list[Method]:
        declaration = declarations_by_name[key]
        candidates = methods_by_owner.get(declaration.name, [])
        if len(names[declaration.name]) == 1:
            return candidates
        return [method for method in candidates if method.path == declaration.path]

    semantic_sources: dict[str, str] = {}
    dependency_graph: dict[str, set[str]] = {}
    for key, declaration in declarations_by_name.items():
        source = declaration.source + "\n" + "\n".join(
            method.signature for method in type_methods(key)
        )
        semantic_sources[key] = source
        dependency_graph[key] = {
            dependency
            for name in IDENTIFIER.findall(source)
            for dependency in names.get(name, ())
        }

    root_roles: dict[str, set[str]] = {}
    for root in roots:
        start = declaration_key(root["path"], root["type"])
        visited = {start}
        frontier = [start]
        while frontier:
            key = frontier.pop()
            for dependency in sorted(dependency_graph[key].difference(visited)):
                visited.add(dependency)
                frontier.append(dependency)
        for key in visited:
            root_roles.setdefault(key, set()).add(root["role"])
    reachable = set(root_roles)

    combined = "\n".join(
        mask_rust_non_code(path.read_text()) for path in reference_paths
    )
    identifier_counts = Counter(IDENTIFIER.findall(combined))
    qualified_counts = Counter(
        re.findall(
            r"(?=\b([A-Za-z_][A-Za-z0-9_]*)\s*::\s*"
            r"([A-Za-z_][A-Za-z0-9_]*)\b)",
            combined,
        )
    )
    types: list[dict[str, object]] = []
    primitive_counts = {primitive: 0 for primitive in SYNC_PRIMITIVES}
    for key in sorted(reachable):
        declaration = declarations_by_name[key]
        name = declaration.name
        reachable_methods = type_methods(key)
        semantic_source = semantic_sources[key]
        for primitive in SYNC_PRIMITIVES:
            primitive_counts[primitive] += len(
                re.findall(rf"\b{re.escape(primitive)}\b", semantic_source)
            )
        dependencies = sorted(
            {
                dependency
                for identifier in IDENTIFIER.findall(semantic_source)
                for dependency in names.get(identifier, ())
                if dependency in reachable and dependency != key
            }
        )
        variants = []
        impl_source = "\n".join(method.source for method in reachable_methods)
        self_variant_counts = Counter(
            re.findall(r"\bSelf\s*::\s*([A-Za-z_][A-Za-z0-9_]*)\b", impl_source)
        )
        for variant in declaration.variants:
            variant_entry: dict[str, object] = {
                "name": variant,
                "qualified_references": qualified_counts[(name, variant)]
                + self_variant_counts[variant],
            }
            if source_variant_flow is not None:
                variant_entry["source_flow"] = source_variant_flow[(name, variant)]
            if expanded_variant_flow is not None:
                variant_entry["expanded_flow"] = expanded_variant_flow[(name, variant)]
            variants.append(variant_entry)
        type_entry: dict[str, object] = {
                "name": name,
                "kind": declaration.kind,
                "path": declaration.path,
                "line": declaration.line,
                "dependencies": dependencies,
                "reachable_from": sorted(root_roles[key]),
                "semantic_bindings": sorted(
                    {role_bindings[role] for role in root_roles[key]}
                ),
                "methods": [
                    {"name": method.name, "path": method.path, "line": method.line}
                    for method in reachable_methods
                ],
                "references": identifier_counts[name] - len(names[name]),
                "variants": variants,
            }
        if declaration.kind == "struct" and source_construction_flow is not None:
            type_entry["source_constructors"] = source_construction_flow[key]
        if declaration.kind == "struct" and expanded_construction_flow is not None:
            type_entry["expanded_constructors"] = expanded_construction_flow[key]
        types.append(type_entry)
    unreachable_types = []
    for key, declaration in sorted(declarations_by_name.items()):
        if key in reachable:
            continue
        name = declaration.name
        declaration_methods = type_methods(key)
        semantic_source = semantic_sources[key]
        impl_source = "\n".join(method.source for method in declaration_methods)
        self_variant_counts = Counter(
            re.findall(r"\bSelf\s*::\s*([A-Za-z_][A-Za-z0-9_]*)\b", impl_source)
        )
        type_entry = {
                "name": name,
                "kind": declaration.kind,
                "path": declaration.path,
                "line": declaration.line,
                "dependencies": sorted(
                    {
                        dependency
                        for identifier in IDENTIFIER.findall(semantic_source)
                        for dependency in names.get(identifier, ())
                        if dependency != key
                    }
                ),
                "reachable_from": [],
                "semantic_bindings": [],
                "methods": [
                    {"name": method.name, "path": method.path, "line": method.line}
                    for method in declaration_methods
                ],
                "references": identifier_counts[name] - len(names[name]),
                "variants": [
                    {
                        "name": variant,
                        "qualified_references": qualified_counts[(name, variant)]
                        + self_variant_counts[variant],
                        **(
                            {"source_flow": source_variant_flow[(name, variant)]}
                            if source_variant_flow is not None
                            else {}
                        ),
                        **(
                            {"expanded_flow": expanded_variant_flow[(name, variant)]}
                            if expanded_variant_flow is not None
                            else {}
                        ),
                    }
                    for variant in declaration.variants
                ],
            }
        if declaration.kind == "struct" and source_construction_flow is not None:
            type_entry["source_constructors"] = source_construction_flow[key]
        if declaration.kind == "struct" and expanded_construction_flow is not None:
            type_entry["expanded_constructors"] = expanded_construction_flow[key]
        unreachable_types.append(type_entry)
    return {
        "roots": roots,
        "source_files": [
            path.relative_to(REPO_ROOT).as_posix() for path in declaration_paths
        ],
        "reference_files": [
            path.relative_to(REPO_ROOT).as_posix() for path in reference_paths
        ],
        "sync_primitive_references": {
            name: count for name, count in primitive_counts.items() if count
        },
        "types": types,
        "unreachable_types": unreachable_types,
    }


def derive(args: argparse.Namespace) -> tuple[dict[str, object], list[str]]:
    try:
        contract = json.loads(CONTRACT.read_text())
    except (OSError, json.JSONDecodeError) as error:
        return {}, [f"cannot load architecture contract: {error}"]
    refinement = contract.get("refinement_inventory")
    if not isinstance(refinement, dict) or refinement.get("schema_version") != 1:
        return {}, ["architecture contract refinement_inventory schema must be 1"]

    model_paths, model_errors = rust_sources(
        refinement.get("model_source_roots"), exclude_tests=False
    )
    production_paths, production_errors = rust_sources(
        refinement.get("production_source_roots"), exclude_tests=True
    )
    production_reference_paths, production_reference_errors = rust_sources(
        refinement.get("production_reference_roots"), exclude_tests=True
    )
    model_declarations, model_declaration_errors = declarations(model_paths)
    production_declarations, production_declaration_errors = declarations(production_paths)
    model_roots, model_root_errors = validate_roots(
        refinement.get("model_roots"), model_declarations, "model_roots"
    )
    production_roots, production_root_errors = validate_roots(
        refinement.get("production_roots"), production_declarations, "production_roots"
    )
    try:
        registry = json.loads(BEHAVIOR_REGISTRY.read_text())
    except (OSError, json.JSONDecodeError) as error:
        return {}, [f"cannot load review behavior registry: {error}"]
    behavior_ids = {
        behavior.get("id")
        for behavior in registry.get("behaviors", [])
        if isinstance(behavior, dict) and isinstance(behavior.get("id"), str)
    }
    bindings, model_role_bindings, production_role_bindings, binding_errors = (
        validate_bindings(
            refinement.get("semantic_bindings"),
            model_roots,
            production_roots,
            behavior_ids,
        )
    )
    errors = [
        *model_errors,
        *production_errors,
        *production_reference_errors,
        *model_declaration_errors,
        *production_declaration_errors,
        *model_root_errors,
        *production_root_errors,
        *binding_errors,
    ]
    if errors:
        return {}, errors
    if (
        args.expanded_production is not None or args.cargo_expand_production
    ) and not args.variant_flow:
        return {}, ["production expansion requires --variant-flow"]

    model_variant_flow = None
    production_variant_flow = None
    expanded_variant_flow = None
    model_construction_flow = None
    production_construction_flow = None
    expanded_construction_flow = None
    generated_expansion: Path | None = None
    if args.variant_flow:
        model_variant_flow, model_flow_errors = variant_flow(
            model_paths, model_declarations
        )
        production_variant_flow, production_flow_errors = variant_flow(
            production_reference_paths, production_declarations
        )
        model_construction_flow, model_construction_errors = construction_flow(
            model_paths, model_declarations
        )
        production_construction_flow, production_construction_errors = construction_flow(
            production_reference_paths, production_declarations
        )
        errors.extend(model_flow_errors)
        errors.extend(production_flow_errors)
        errors.extend(model_construction_errors)
        errors.extend(production_construction_errors)
        expanded_argument = args.expanded_production
        if args.cargo_expand_production:
            executable = shutil.which("cargo")
            if executable is None:
                errors.append("--cargo-expand-production requires cargo on PATH")
            else:
                result = subprocess.run(
                    [
                        executable,
                        "expand",
                        "-p",
                        "ckb-tx-pool",
                        "--features",
                        "internal",
                        "--lib",
                    ],
                    cwd=REPO_ROOT,
                    check=False,
                    capture_output=True,
                    text=True,
                )
                if result.returncode != 0:
                    errors.append(
                        f"cargo expand failed ({result.returncode}): "
                        f"{result.stderr.strip()}"
                    )
                else:
                    temporary = tempfile.NamedTemporaryFile(
                        mode="w", suffix=".rs", delete=False
                    )
                    with temporary:
                        temporary.write(result.stdout)
                    generated_expansion = Path(temporary.name)
                    expanded_argument = generated_expansion
        if expanded_argument is not None:
            expanded = expanded_argument.resolve()
            if not expanded.is_file():
                errors.append(
                    f"expanded production source does not exist: {expanded_argument}"
                )
            else:
                expanded_variant_flow, expanded_flow_errors = variant_flow(
                    [expanded], production_declarations
                )
                expanded_construction_flow, expanded_construction_errors = construction_flow(
                    [expanded], production_declarations
                )
                errors.extend(expanded_flow_errors)
                errors.extend(expanded_construction_errors)
    if generated_expansion is not None:
        generated_expansion.unlink(missing_ok=True)
    if errors:
        return {}, errors
    inventory = {
        "schema_version": 1,
        "authority": "tx-pool/architecture-contract.json#refinement_inventory",
        "semantic_bindings": bindings,
        "model": reachable_inventory(
            model_roots,
            model_declarations,
            methods(model_paths),
            model_paths,
            model_paths,
            model_role_bindings,
            model_variant_flow,
            source_construction_flow=model_construction_flow,
        ),
        "production": reachable_inventory(
            production_roots,
            production_declarations,
            methods(production_paths),
            production_paths,
            production_reference_paths,
            production_role_bindings,
            production_variant_flow,
            expanded_variant_flow,
            production_construction_flow,
            expanded_construction_flow,
        ),
    }
    return inventory, []


def main() -> int:
    args = parse_args()
    canary_errors = validate_canary(include_variant_flow=args.variant_flow)
    if canary_errors:
        for error in canary_errors:
            print(f"error: {error}", file=sys.stderr)
        return 1
    inventory, errors = derive(args)
    if errors:
        for error in errors:
            print(f"error: {error}", file=sys.stderr)
        return 1
    if args.json:
        print(json.dumps(inventory, indent=2, sort_keys=True))
        return 0
    model = inventory["model"]
    production = inventory["production"]
    flow_suffix = ""
    if args.variant_flow:
        model_variants = [
            variant for type_entry in model["types"] for variant in type_entry["variants"]
        ]
        production_variants = [
            variant
            for type_entry in production["types"]
            for variant in type_entry["variants"]
        ]
        source_constructorless = sum(
            not variant["source_flow"]["producers"]
            for variant in production_variants
        )
        expanded_constructorless = (
            sum(
                not variant["expanded_flow"]["producers"]
                for variant in production_variants
            )
            if args.expanded_production is not None or args.cargo_expand_production
            else source_constructorless
        )
        model_constructorless = sum(
            not variant["source_flow"]["producers"] for variant in model_variants
        )
        model_types = {
            (entry["path"], entry["name"]): entry for entry in model["types"]
        }
        production_types = {
            (entry["path"], entry["name"]): entry
            for entry in production["types"]
        }
        model_root_structs = [
            model_types[(root["path"], root["type"])]
            for root in model["roots"]
            if model_types[(root["path"], root["type"])]["kind"] == "struct"
        ]
        production_root_structs = [
            production_types[(root["path"], root["type"])]
            for root in production["roots"]
            if production_types[(root["path"], root["type"])]["kind"] == "struct"
        ]
        model_root_constructorless = sum(
            not entry["source_constructors"] for entry in model_root_structs
        )
        production_source_root_constructorless = sum(
            not entry["source_constructors"] for entry in production_root_structs
        )
        production_expanded_root_constructorless = (
            sum(
                not entry["expanded_constructors"]
                for entry in production_root_structs
            )
            if args.expanded_production is not None or args.cargo_expand_production
            else production_source_root_constructorless
        )
        flow_suffix = (
            "; variant-flow constructorless: "
            f"model={model_constructorless}, "
            f"production-source={source_constructorless}, "
            f"production-expanded={expanded_constructorless}; "
            "root-struct constructorless: "
            f"model={model_root_constructorless}, "
            f"production-source={production_source_root_constructorless}, "
            f"production-expanded={production_expanded_root_constructorless}"
        )
        if model_constructorless or model_root_constructorless or (
            (args.expanded_production is not None or args.cargo_expand_production)
            and (
                expanded_constructorless
                or production_expanded_root_constructorless
            )
        ):
            print(
                "error: rooted route has no mechanically reachable constructor",
                file=sys.stderr,
            )
            return 1
    print(
        "validated mechanically derived refinement frontier: "
        f"model={len(model['types'])} reachable/"
        f"{len(model['unreachable_types'])} outside roots/"
        f"{len(model['source_files'])} files, production={len(production['types'])} "
        f"reachable/{len(production['unreachable_types'])} outside roots/"
        f"{len(production['source_files'])} files"
        f"{flow_suffix}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
