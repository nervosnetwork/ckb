#!/usr/bin/env python3
"""Derive the model/production refinement frontier from semantic Rust roots."""

from __future__ import annotations

import argparse
import ast
from bisect import bisect_right
from collections import Counter, deque
from dataclasses import dataclass
import hashlib
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
CALL_REFERENCE = re.compile(
    r"(?P<qualified>(?:\b[A-Za-z_][A-Za-z0-9_]*\s*::\s*)*)"
    r"(?P<name>[A-Za-z_][A-Za-z0-9_]*)\s*(?:::\s*<[^>{};]*>)?\s*\("
)
FUNCTION_VALUE_REFERENCE = re.compile(
    r"[(,=]\s*&?\s*"
    r"(?P<qualified>(?:\b[A-Za-z_][A-Za-z0-9_]*\s*::\s*)*)"
    r"(?P<name>[A-Za-z_][A-Za-z0-9_]*)\s*(?=[,),;])"
)
QUALIFIED_IDENTIFIER = re.compile(
    r"\b(?P<qualified>[A-Za-z_][A-Za-z0-9_]*"
    r"(?:\s*::\s*[A-Za-z_][A-Za-z0-9_]*)+)\b"
)
IMPL_DECLARATION = re.compile(r"(?m)^[ \t]*(?:unsafe\s+)?impl\b")
EXPANDED_LINT_ATTRIBUTE = re.compile(
    r"#\s*\[\s*allow\s*\(\s*non_exhaustive_omitted_patterns\s*\)\s*\]"
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
MODULE_DECLARATION = re.compile(
    r"(?m)^[ \t]*(?:pub(?:\s*\([^\n)]*\))?\s+)?mod\s+"
    r"(?P<name>[A-Za-z_][A-Za-z0-9_]*)\s*;"
)
CFG_TOKEN = re.compile(
    r'\s*(?:(?P<identifier>[A-Za-z_][A-Za-z0-9_-]*)|'
    r'(?P<string>"(?:\\.|[^"\\])*")|(?P<punctuation>[=(),]))'
)


@dataclass(frozen=True)
class Method:
    name: str
    path: str
    line: int
    signature: str
    source: str


@dataclass(frozen=True)
class Function:
    name: str
    path: str
    line: int
    signature: str
    source: str
    owner: str | None
    is_test: bool


@dataclass(frozen=True)
class OwnershipSeed:
    node: str
    kind: str
    label: str
    path: str
    line: int
    symbol: str


@dataclass(frozen=True)
class TypeDeclaration:
    name: str
    kind: str
    visibility: str
    path: str
    line: int
    source: str
    variants: tuple[str, ...]


def requires_internal_constructor(visibility: str) -> bool:
    return visibility != "pub"


@dataclass(frozen=True)
class CfgEnvironment:
    test: bool
    features: frozenset[str]


@dataclass(frozen=True)
class ModuleDeclaration:
    name: str
    path_override: str | None
    cfg_expressions: tuple[str, ...]
    line: int


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


class _CfgParser:
    def __init__(self, source: str, environment: CfgEnvironment) -> None:
        self.environment = environment
        self.tokens: list[tuple[str, str]] = []
        cursor = 0
        while cursor < len(source):
            match = CFG_TOKEN.match(source, cursor)
            if match is None:
                if source[cursor:].strip():
                    raise ValueError(f"unsupported cfg syntax {source!r}")
                break
            kind = next(
                name
                for name in ("identifier", "string", "punctuation")
                if match.group(name) is not None
            )
            self.tokens.append((kind, match.group(kind)))
            cursor = match.end()
        self.index = 0

    def peek(self, value: str | None = None) -> tuple[str, str] | None:
        token = self.tokens[self.index] if self.index < len(self.tokens) else None
        if value is not None and (token is None or token[1] != value):
            return None
        return token

    def take(self, value: str | None = None) -> tuple[str, str]:
        token = self.peek(value)
        if token is None:
            expected = value if value is not None else "token"
            raise ValueError(f"expected {expected!r} in cfg expression")
        self.index += 1
        return token

    def expression(self) -> bool:
        kind, identifier = self.take()
        if kind != "identifier":
            raise ValueError("cfg expression must start with an identifier")
        if self.peek("(") is not None:
            self.take("(")
            values: list[bool] = []
            if self.peek(")") is None:
                while True:
                    values.append(self.expression())
                    if self.peek(",") is None:
                        break
                    self.take(",")
            self.take(")")
            if identifier == "any":
                return any(values)
            if identifier == "all":
                return all(values)
            if identifier == "not" and len(values) == 1:
                return not values[0]
            raise ValueError(f"unsupported cfg operator {identifier!r}")
        if self.peek("=") is not None:
            self.take("=")
            string_kind, encoded = self.take()
            if string_kind != "string" or identifier != "feature":
                raise ValueError(f"unsupported cfg predicate {identifier!r}")
            feature = json.loads(encoded)
            return feature in self.environment.features
        if identifier == "test":
            return self.environment.test
        raise ValueError(f"unsupported cfg atom {identifier!r}")

    def evaluate(self) -> bool:
        result = self.expression()
        if self.index != len(self.tokens):
            raise ValueError("cfg expression has trailing tokens")
        return result


def evaluate_cfg(source: str, environment: CfgEnvironment) -> bool:
    return _CfgParser(source, environment).evaluate()


def preceding_outer_attributes(source: str, offset: int) -> list[str]:
    """Return the contiguous raw outer attributes immediately before one item."""

    attributes: list[str] = []
    cursor = offset
    while True:
        while cursor and source[cursor - 1].isspace():
            cursor -= 1
        if cursor == 0 or source[cursor - 1] != "]":
            break
        start = source.rfind("#[", 0, cursor)
        if start < 0:
            break
        closing = source.find("]", start + 2)
        if closing != cursor - 1:
            break
        attributes.append(source[start + 2 : closing].strip())
        cursor = start
    attributes.reverse()
    return attributes


def module_declarations(path: Path) -> tuple[list[ModuleDeclaration], list[str]]:
    errors: list[str] = []
    try:
        source = path.read_text()
        masked = mask_rust_non_code(source)
    except (OSError, ValueError) as error:
        return [], [f"cannot inspect Rust modules in {path}: {error}"]
    declarations: list[ModuleDeclaration] = []
    for match in MODULE_DECLARATION.finditer(masked):
        attributes = preceding_outer_attributes(source, match.start())
        cfg_expressions: list[str] = []
        path_override = None
        for attribute in attributes:
            cfg = re.fullmatch(r"cfg\s*\((.*)\)", attribute, re.S)
            if cfg is not None:
                cfg_expressions.append(cfg.group(1).strip())
                continue
            path_attribute = re.fullmatch(
                r'path\s*=\s*("(?:\\.|[^"\\])*")', attribute, re.S
            )
            if path_attribute is not None:
                if path_override is not None:
                    errors.append(
                        f"module {match.group('name')} in {path} repeats #[path]"
                    )
                else:
                    path_override = json.loads(path_attribute.group(1))
        declarations.append(
            ModuleDeclaration(
                name=match.group("name"),
                path_override=path_override,
                cfg_expressions=tuple(cfg_expressions),
                line=source.count("\n", 0, match.start()) + 1,
            )
        )
    return declarations, errors


def resolve_module_path(parent: Path, declaration: ModuleDeclaration) -> Path:
    if declaration.path_override is not None:
        return (parent.parent / declaration.path_override).resolve()
    base = (
        parent.parent
        if parent.name in {"lib.rs", "main.rs", "mod.rs"}
        else parent.parent / parent.stem
    )
    file_candidate = base / f"{declaration.name}.rs"
    directory_candidate = base / declaration.name / "mod.rs"
    existing = [
        candidate.resolve()
        for candidate in (file_candidate, directory_candidate)
        if candidate.is_file()
    ]
    if len(existing) != 1:
        relative = parent.relative_to(REPO_ROOT)
        raise ValueError(
            f"module {declaration.name!r} at {relative}:{declaration.line} "
            f"resolves to {len(existing)} files"
        )
    return existing[0]


def rust_module_graph(
    root: Path, environment: CfgEnvironment
) -> tuple[set[Path], list[dict[str, object]], list[str]]:
    """Derive one feature/test source universe from Rust module declarations."""

    paths: set[Path] = set()
    edges: list[dict[str, object]] = []
    errors: list[str] = []
    frontier = [root.resolve()]
    while frontier:
        parent = frontier.pop()
        if parent in paths:
            continue
        try:
            parent.relative_to(REPO_ROOT)
        except ValueError:
            errors.append(f"Rust module path escapes repository root: {parent}")
            continue
        if not parent.is_file():
            errors.append(f"Rust module source is absent: {parent}")
            continue
        paths.add(parent)
        declarations, declaration_errors = module_declarations(parent)
        errors.extend(declaration_errors)
        for declaration in declarations:
            try:
                enabled = all(
                    evaluate_cfg(expression, environment)
                    for expression in declaration.cfg_expressions
                )
            except (ValueError, json.JSONDecodeError) as error:
                relative = parent.relative_to(REPO_ROOT)
                errors.append(
                    f"cannot evaluate module cfg at {relative}:{declaration.line}: {error}"
                )
                continue
            if not enabled:
                continue
            try:
                child = resolve_module_path(parent, declaration)
                child.relative_to(REPO_ROOT)
            except ValueError as error:
                errors.append(str(error))
                continue
            edges.append(
                {
                    "parent": parent.relative_to(REPO_ROOT).as_posix(),
                    "module": declaration.name,
                    "child": child.relative_to(REPO_ROOT).as_posix(),
                }
            )
            if child not in paths:
                frontier.append(child)
    edges.sort(key=lambda row: (row["parent"], row["module"], row["child"]))
    return paths, edges, errors


def canonical_sha256(value: object) -> str:
    payload = json.dumps(
        value, sort_keys=True, separators=(",", ":"), ensure_ascii=True
    ).encode()
    return hashlib.sha256(payload).hexdigest()


MECHANISM_COMPLETENESS_AUTHORITY = (
    "tx-pool/scripts/check_model_refinement.py#mechanism_completeness_projection_v1"
)
_RELEASE_MANIFEST_BASENAME = "security-regression-manifest.json"
_RELEASE_WRITE_METHODS = frozenset({"write_text", "write_bytes"})


def _release_state_policy_projection_v1(
    sources: dict[str, str] | None = None,
) -> tuple[dict[str, str] | None, list[str]]:
    """Find scripts with a data-flow path from the release manifest to a write.

    This is deliberately a small capability analysis rather than a spelling
    heuristic.  It follows the canonical path through assignments, argparse
    result attributes, function arguments and method aliases.  Names containing
    ``manifest`` carry no authority by themselves.
    """

    if sources is None:
        sources = {}
        scripts = repo_path("tx-pool/scripts")
        for path in sorted(scripts.glob("check_*.py")):
            if path.name == "check_all.py":
                continue
            try:
                sources[path.relative_to(REPO_ROOT).as_posix()] = path.read_text()
            except OSError as error:
                return None, [f"complexity cannot read policy script {path}: {error}"]

    projection: dict[str, str] = {}
    for path, source in sorted(sources.items()):
        source_path = Path(path)
        if (
            not source_path.name.startswith("check_")
            or source_path.suffix != ".py"
            or source_path.name == "check_all.py"
        ):
            continue
        try:
            tree = ast.parse(source, filename=path)
        except SyntaxError:
            continue

        module_scope = "<module>"
        functions = {
            node.name: node
            for node in tree.body
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
        }
        function_scopes = {
            name: f"{name}:{node.lineno}" for name, node in functions.items()
        }
        local_names: dict[str, set[str]] = {module_scope: set()}
        for name, node in functions.items():
            scope = function_scopes[name]
            parameters = {
                argument.arg
                for argument in (
                    *node.args.posonlyargs,
                    *node.args.args,
                    *node.args.kwonlyargs,
                )
            }
            if node.args.vararg is not None:
                parameters.add(node.args.vararg.arg)
            if node.args.kwarg is not None:
                parameters.add(node.args.kwarg.arg)
            stored = {
                child.id
                for child in ast.walk(node)
                if isinstance(child, ast.Name) and isinstance(child.ctx, ast.Store)
            }
            local_names[scope] = parameters | stored

        def variable_key(scope: str, name: str, kind: str = "origin") -> tuple[str, str, str]:
            owner = scope if scope != module_scope and name in local_names[scope] else module_scope
            return kind, owner, name

        constant_key = ("origin", "<constant>", _RELEASE_MANIFEST_BASENAME)
        edges: dict[tuple[str, str, str], set[tuple[str, str, str]]] = {}
        seeds = {constant_key}
        sink_requirements: list[set[tuple[str, str, str]]] = []

        def add_edges(
            target: tuple[str, str, str], dependencies: set[tuple[str, str, str]]
        ) -> None:
            if dependencies:
                edges.setdefault(target, set()).update(dependencies)

        def expression_origins(node: ast.AST | None, scope: str) -> set[tuple[str, str, str]]:
            if node is None:
                return set()
            if (
                isinstance(node, ast.Constant)
                and isinstance(node.value, str)
                and _RELEASE_MANIFEST_BASENAME in node.value
            ):
                return {constant_key}
            if isinstance(node, ast.Name):
                return {variable_key(scope, node.id)}
            result: set[tuple[str, str, str]] = set()
            if isinstance(node, ast.Attribute):
                result.update(expression_origins(node.value, scope))
                result.add(("origin", "<argparse-attribute>", node.attr))
            if isinstance(node, ast.Call) and isinstance(node.func, ast.Name):
                callee = functions.get(node.func.id)
                if callee is not None:
                    result.add(("origin", function_scopes[node.func.id], "<return>"))
            for child in ast.iter_child_nodes(node):
                result.update(expression_origins(child, scope))
            return result

        def sink_origins(node: ast.AST | None, scope: str) -> set[tuple[str, str, str]]:
            if isinstance(node, ast.Attribute) and node.attr in _RELEASE_WRITE_METHODS:
                return expression_origins(node.value, scope)
            if (
                isinstance(node, ast.Attribute)
                and node.attr == "write"
                and isinstance(node.value, ast.Call)
                and (
                    (
                        isinstance(node.value.func, ast.Name)
                        and node.value.func.id == "open"
                    )
                    or (
                        isinstance(node.value.func, ast.Attribute)
                        and node.value.func.attr == "open"
                    )
                )
                and node.value.args
            ):
                return expression_origins(node.value.args[0], scope).union(
                    expression_origins(
                        node.value.func.value
                        if isinstance(node.value.func, ast.Attribute)
                        else None,
                        scope,
                    )
                )
            if (
                isinstance(node, ast.Call)
                and (
                    (isinstance(node.func, ast.Name) and node.func.id == "getattr")
                    or (
                        isinstance(node.func, ast.Attribute)
                        and node.func.attr == "__getattribute__"
                    )
                )
                and len(node.args) >= 2
                and isinstance(node.args[1], ast.Constant)
                and node.args[1].value in _RELEASE_WRITE_METHODS
            ):
                return expression_origins(node.args[0], scope)
            if isinstance(node, ast.Name):
                return {variable_key(scope, node.id, "sink")}
            return set()

        def assigned_names(node: ast.AST) -> set[str]:
            return {
                child.id
                for child in ast.walk(node)
                if isinstance(child, ast.Name) and isinstance(child.ctx, ast.Store)
            }

        scoped_roots: list[tuple[str, list[ast.stmt]]] = [(module_scope, tree.body)]
        scoped_roots.extend(
            (function_scopes[name], node.body) for name, node in functions.items()
        )
        for scope, statements in scoped_roots:
            nodes = [child for statement in statements for child in ast.walk(statement)]
            for node in nodes:
                if isinstance(node, (ast.Assign, ast.AnnAssign)):
                    value = node.value
                    targets = node.targets if isinstance(node, ast.Assign) else [node.target]
                    names = set().union(*(assigned_names(target) for target in targets))
                    origins = expression_origins(value, scope)
                    sink_values = sink_origins(value, scope)
                    for name in names:
                        add_edges(variable_key(scope, name), origins)
                        add_edges(variable_key(scope, name, "sink"), sink_values)
                elif isinstance(node, ast.Return):
                    add_edges(
                        ("origin", scope, "<return>"),
                        expression_origins(node.value, scope),
                    )
                    add_edges(
                        ("sink", scope, "<return>"), sink_origins(node.value, scope)
                    )
                if not isinstance(node, ast.Call):
                    continue

                sink_dependencies = sink_origins(node.func, scope)
                if (
                    isinstance(node.func, ast.Call)
                    and isinstance(node.func.func, ast.Attribute)
                    and node.func.func.attr == "methodcaller"
                    and node.func.args
                    and isinstance(node.func.args[0], ast.Constant)
                    and node.func.args[0].value in _RELEASE_WRITE_METHODS
                ):
                    sink_dependencies.update(
                        dependency
                        for argument in node.args
                        for dependency in expression_origins(argument, scope)
                    )
                if sink_dependencies:
                    sink_requirements.append(sink_dependencies)

                if (
                    isinstance(node.func, ast.Attribute)
                    and node.func.attr == "add_argument"
                    and node.args
                    and isinstance(node.args[0], ast.Constant)
                    and isinstance(node.args[0].value, str)
                ):
                    option = node.args[0].value
                    destination = next(
                        (
                            keyword.value.value
                            for keyword in node.keywords
                            if keyword.arg == "dest"
                            and isinstance(keyword.value, ast.Constant)
                            and isinstance(keyword.value.value, str)
                        ),
                        option.lstrip("-").replace("-", "_"),
                    )
                    default = next(
                        (keyword.value for keyword in node.keywords if keyword.arg == "default"),
                        None,
                    )
                    add_edges(
                        ("origin", "<argparse-attribute>", destination),
                        expression_origins(default, scope),
                    )

                if not isinstance(node.func, ast.Name) or node.func.id not in functions:
                    continue
                callee = functions[node.func.id]
                callee_scope = function_scopes[node.func.id]
                positional = [*callee.args.posonlyargs, *callee.args.args]
                for parameter, argument in zip(positional, node.args):
                    add_edges(
                        ("origin", callee_scope, parameter.arg),
                        expression_origins(argument, scope),
                    )
                    add_edges(
                        ("sink", callee_scope, parameter.arg),
                        sink_origins(argument, scope),
                    )
                parameters_by_name = {
                    parameter.arg: parameter
                    for parameter in (*positional, *callee.args.kwonlyargs)
                }
                for keyword in node.keywords:
                    if keyword.arg not in parameters_by_name:
                        continue
                    add_edges(
                        ("origin", callee_scope, keyword.arg),
                        expression_origins(keyword.value, scope),
                    )
                    add_edges(
                        ("sink", callee_scope, keyword.arg),
                        sink_origins(keyword.value, scope),
                    )

        tainted = set(seeds)
        changed = True
        while changed:
            changed = False
            for target, dependencies in edges.items():
                if target not in tainted and dependencies.intersection(tainted):
                    tainted.add(target)
                    changed = True
        if any(requirement.intersection(tainted) for requirement in sink_requirements):
            projection[path] = hashlib.sha256(source.encode()).hexdigest()

    if not projection:
        return None, ["complexity has no release-state policy authority"]
    return projection, []


def mechanism_completeness_projection_v1(
    semantic_census: object,
    *,
    inventory: dict[str, object] | None = None,
    policy_sources: dict[str, str] | None = None,
) -> tuple[dict[str, object] | None, list[str]]:
    """Derive the completeness boundary outside its consuming certificate gate."""

    errors: list[str] = []
    if inventory is None:
        errors.extend(validate_canary(include_variant_flow=False))
        arguments = argparse.Namespace(
            json=False,
            variant_flow=False,
            expanded_production=None,
            cargo_expand_production=False,
        )
        inventory, inventory_errors = derive(arguments)
        errors.extend(inventory_errors)
    if not isinstance(inventory, dict):
        return None, errors or ["mechanism completeness inventory is invalid"]

    source_roles = inventory.get("source_role_census")
    bindings = inventory.get("semantic_bindings")
    if not isinstance(source_roles, dict) or not isinstance(bindings, dict):
        return None, ["mechanism completeness source graph is invalid"]
    unreferenced = source_roles.get("unreferenced_rust_sources")
    if unreferenced != []:
        errors.append(
            "mechanism completeness retains unreferenced Rust sources: "
            f"{unreferenced!r}"
        )

    side_projection: dict[str, dict[str, object]] = {}
    for side in ("production", "model"):
        value = inventory.get(side)
        if not isinstance(value, dict):
            errors.append(f"mechanism completeness {side} inventory is invalid")
            continue
        if value.get("unreachable_types") != []:
            errors.append(f"mechanism completeness {side} retains unowned types")
        if value.get("unreached_connector_functions") != []:
            errors.append(
                f"mechanism completeness {side} retains unowned connector functions"
            )
        roots = value.get("roots")
        types = value.get("types")
        connectors = value.get("connector_functions")
        necessity_sha256 = value.get("necessity_sha256")
        if (
            not isinstance(roots, list)
            or not isinstance(types, list)
            or not isinstance(connectors, list)
            or re.fullmatch(r"[0-9a-f]{64}", str(necessity_sha256)) is None
        ):
            errors.append(f"mechanism completeness {side} graph shape is invalid")
            continue
        root_roles = sorted(
            root.get("role")
            for root in roots
            if isinstance(root, dict) and isinstance(root.get("role"), str)
        )
        if len(root_roles) != len(roots) or len(root_roles) != len(set(root_roles)):
            errors.append(f"mechanism completeness {side} roots are not injective")
        side_projection[side] = {
            "necessity_sha256": necessity_sha256,
            "root_roles": root_roles,
            "state_bearing_type_count": len(types),
            "connector_function_count": len(connectors),
        }

    binding_behavior_ids = sorted(
        {
            behavior_id
            for binding in bindings.values()
            if isinstance(binding, dict)
            for behavior_id in binding.get("behavior_ids", [])
            if isinstance(behavior_id, str)
        }
    )
    grains = (
        semantic_census.get("semantic_grains")
        if isinstance(semantic_census, dict)
        else None
    )
    if not isinstance(grains, list):
        errors.append("mechanism completeness semantic grains are invalid")
        census_behavior_ids: list[str] = []
    else:
        census_behavior_ids = sorted(
            {
                grain.get("behavior_id")
                for grain in grains
                if isinstance(grain, dict)
                and isinstance(grain.get("behavior_id"), str)
            }
        )
    if binding_behavior_ids != census_behavior_ids:
        errors.append(
            "mechanism completeness source/census behavior projections differ: "
            f"source={binding_behavior_ids}, census={census_behavior_ids}"
        )

    policy_projection, policy_errors = _release_state_policy_projection_v1(
        policy_sources
    )
    errors.extend(policy_errors)
    if errors or policy_projection is None:
        return None, errors

    projection = {
        "schema_version": 1,
        "authority": MECHANISM_COMPLETENESS_AUTHORITY,
        "derivation_source_sha256": {
            "model_refinement": hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
        },
        "source_role_census_sha256": source_roles.get("census_sha256"),
        "necessity_census_sha256": inventory.get("necessity_census_sha256"),
        "behavior_ids": binding_behavior_ids,
        "production": side_projection.get("production"),
        "model": side_projection.get("model"),
        "release_policy_authorities": sorted(policy_projection),
        "negative_mutation_basis": [
            "unregistered_state_bearing_type",
            "unregistered_connector_function",
            "unreferenced_compile_role_source",
            "semantic_behavior_projection_mismatch",
            "duplicate_release_policy_writer",
            "unrelated_release_writer_name",
            "parameterized_release_writer",
            "reflective_release_writer",
        ],
    }
    return projection, []


def cargo_tx_pool_metadata() -> tuple[dict[str, object] | None, list[str]]:
    result = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        return None, [f"cargo metadata failed: {result.stderr.strip()}"]
    try:
        metadata = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        return None, [f"cargo metadata returned invalid JSON: {error}"]
    packages = [
        package
        for package in metadata.get("packages", [])
        if package.get("name") == "ckb-tx-pool"
    ]
    if len(packages) != 1:
        return None, [f"cargo metadata found {len(packages)} ckb-tx-pool packages"]
    return packages[0], []


def expanded_feature_set(
    features: dict[str, object], selected: set[str]
) -> tuple[set[str], list[str]]:
    enabled = set(selected)
    errors: list[str] = []
    frontier = list(selected)
    while frontier:
        feature = frontier.pop()
        members = features.get(feature)
        if not isinstance(members, list) or not all(
            isinstance(member, str) for member in members
        ):
            errors.append(f"Cargo feature {feature!r} has an invalid definition")
            continue
        for member in members:
            candidate = member.split("?", 1)[0]
            if candidate.startswith("dep:") or "/" in candidate:
                continue
            if candidate in features and candidate not in enabled:
                enabled.add(candidate)
                frontier.append(candidate)
    return enabled, errors


def source_metrics(paths: set[Path]) -> tuple[dict[str, object], list[str]]:
    errors: list[str] = []
    file_hashes: dict[str, str] = {}
    bytes_total = physical_lines = code_lines = 0
    for path in sorted(paths):
        try:
            payload = path.read_bytes()
            source = payload.decode()
            masked = mask_rust_non_code(source)
        except (OSError, UnicodeDecodeError, ValueError) as error:
            errors.append(f"cannot measure source {path}: {error}")
            continue
        relative = path.relative_to(REPO_ROOT).as_posix()
        file_hashes[relative] = hashlib.sha256(payload).hexdigest()
        bytes_total += len(payload)
        physical_lines += len(source.splitlines())
        code_lines += sum(bool(line.strip()) for line in masked.splitlines())
    identity = {
        "files": sorted(file_hashes),
        "file_sha256": file_hashes,
    }
    return {
        "file_count": len(file_hashes),
        "bytes": bytes_total,
        "physical_lines": physical_lines,
        "nonblank_code_lines": code_lines,
        "content_sha256": canonical_sha256(identity),
        "files": sorted(file_hashes),
    }, errors


def derive_source_role_census(
    refinement: dict[str, object]
) -> tuple[dict[str, object] | None, list[str]]:
    """Derive disjoint compile roles without copying paths into an allowlist."""

    package, errors = cargo_tx_pool_metadata()
    if package is None:
        return None, errors
    features = package.get("features")
    targets = package.get("targets")
    if not isinstance(features, dict) or not isinstance(targets, list):
        return None, errors + ["ckb-tx-pool Cargo metadata has an invalid shape"]
    library_targets = [
        target
        for target in targets
        if isinstance(target, dict) and target.get("kind") == ["lib"]
    ]
    if len(library_targets) != 1:
        return None, errors + [
            f"ckb-tx-pool Cargo metadata found {len(library_targets)} library targets"
        ]
    try:
        root = Path(library_targets[0]["src_path"]).resolve()
        root.relative_to(REPO_ROOT)
    except (KeyError, TypeError, ValueError) as error:
        return None, errors + [f"ckb-tx-pool library root is invalid: {error}"]

    default_selected = {"default"} if "default" in features else set()
    default_features, default_feature_errors = expanded_feature_set(
        features, default_selected
    )
    internal_features, internal_feature_errors = expanded_feature_set(
        features, default_selected | {"internal"}
    )
    errors.extend(default_feature_errors)
    errors.extend(internal_feature_errors)
    if "internal" not in features:
        errors.append("ckb-tx-pool Cargo metadata has no internal feature")

    environments = {
        "default": CfgEnvironment(False, frozenset(default_features)),
        "internal": CfgEnvironment(False, frozenset(internal_features)),
        "test_internal": CfgEnvironment(True, frozenset(internal_features)),
    }
    graphs: dict[str, set[Path]] = {}
    graph_edges: dict[str, list[dict[str, object]]] = {}
    for name, environment in environments.items():
        paths, edges, graph_errors = rust_module_graph(root, environment)
        graphs[name] = paths
        graph_edges[name] = edges
        errors.extend(f"{name} graph: {error}" for error in graph_errors)

    if not graphs["default"].issubset(graphs["internal"]):
        errors.append("default Rust module graph is not a subset of internal")
    if not graphs["internal"].issubset(graphs["test_internal"]):
        errors.append("internal Rust module graph is not a subset of test+internal")

    default_paths = graphs["default"]
    internal_only = graphs["internal"] - default_paths
    test_only = graphs["test_internal"] - graphs["internal"]
    model_only = {
        path
        for path in test_only
        if path.is_relative_to(REPO_ROOT / "tx-pool" / "src" / "tests" / "model")
    }
    test_support_only = test_only - model_only
    compile_roles = {
        "default_production": default_paths,
        "internal_feature_only": internal_only,
        "test_model_only": model_only,
        "test_support_only": test_support_only,
    }
    role_metrics: dict[str, object] = {}
    for role, paths in compile_roles.items():
        metrics, metric_errors = source_metrics(paths)
        role_metrics[role] = metrics
        errors.extend(metric_errors)

    all_sources = set((REPO_ROOT / "tx-pool" / "src").rglob("*.rs"))
    unreferenced = sorted(
        path.relative_to(REPO_ROOT).as_posix()
        for path in all_sources - graphs["test_internal"]
    )
    production_roots = refinement.get("production_roots")
    model_roots = refinement.get("model_roots")
    if not isinstance(production_roots, dict) or not isinstance(model_roots, dict):
        errors.append("refinement roots are unavailable for source-role census")
        production_roots = {}
        model_roots = {}
    production_root_paths = {
        repo_path(key.rsplit("::", 1)[0])
        for key in production_roots
        if isinstance(key, str) and "::" in key
    }
    model_root_paths = {
        repo_path(key.rsplit("::", 1)[0])
        for key in model_roots
        if isinstance(key, str) and "::" in key
    }
    missing_production_roots = sorted(
        path.relative_to(REPO_ROOT).as_posix()
        for path in production_root_paths - default_paths
    )
    missing_model_roots = sorted(
        path.relative_to(REPO_ROOT).as_posix()
        for path in model_root_paths - model_only
    )
    if missing_production_roots:
        errors.append(
            f"production semantic roots are outside default graph: {missing_production_roots}"
        )
    if missing_model_roots:
        errors.append(f"model semantic roots are outside model graph: {missing_model_roots}")

    manifest_path = Path(package.get("manifest_path", "")).resolve()
    try:
        manifest_relative = manifest_path.relative_to(REPO_ROOT).as_posix()
        manifest_sha256 = hashlib.sha256(manifest_path.read_bytes()).hexdigest()
    except (OSError, ValueError) as error:
        errors.append(f"cannot bind ckb-tx-pool Cargo manifest: {error}")
        manifest_relative = ""
        manifest_sha256 = ""
    graph_payload = {
        name: {
            "features": sorted(environment.features),
            "test": environment.test,
            "edges": graph_edges[name],
        }
        for name, environment in environments.items()
    }
    payload = {
        "schema_version": 1,
        "cargo": {
            "package": package.get("name"),
            "manifest": manifest_relative,
            "manifest_sha256": manifest_sha256,
            "library_root": root.relative_to(REPO_ROOT).as_posix(),
            "default_features": sorted(default_features),
            "internal_features": sorted(internal_features),
        },
        "module_graph_sha256": canonical_sha256(graph_payload),
        "compile_roles": role_metrics,
        "unreferenced_rust_sources": unreferenced,
        "semantic_root_coverage": {
            "production_root_files": sorted(
                path.relative_to(REPO_ROOT).as_posix()
                for path in production_root_paths
            ),
            "model_root_files": sorted(
                path.relative_to(REPO_ROOT).as_posix() for path in model_root_paths
            ),
            "default_files_without_root_declaration": sorted(
                path.relative_to(REPO_ROOT).as_posix()
                for path in default_paths - production_root_paths
            ),
            "model_files_without_root_declaration": sorted(
                path.relative_to(REPO_ROOT).as_posix()
                for path in model_only - model_root_paths
            ),
        },
    }
    return {**payload, "census_sha256": canonical_sha256(payload)}, errors


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


def declarations(
    paths: list[Path], environment: CfgEnvironment | None = None
) -> tuple[dict[str, TypeDeclaration], list[str]]:
    discovered: dict[str, TypeDeclaration] = {}
    errors: list[str] = []
    for path in paths:
        source = path.read_text()
        masked = mask_rust_non_code(source)
        for match in TYPE_DECLARATION.finditer(masked):
            if environment is not None:
                item_start = source.rfind("\n", 0, match.start()) + 1
                attributes = preceding_outer_attributes(source, item_start)
                cfg_expressions = [
                    cfg.group(1).strip()
                    for attribute in attributes
                    if (cfg := re.fullmatch(r"cfg\s*\((.*)\)", attribute, re.S))
                    is not None
                ]
                try:
                    if not all(
                        evaluate_cfg(expression, environment)
                        for expression in cfg_expressions
                    ):
                        continue
                except (ValueError, json.JSONDecodeError) as error:
                    errors.append(
                        f"cannot evaluate type cfg in {path.relative_to(REPO_ROOT)}:"
                        f"{source.count(chr(10), 0, match.start()) + 1}: {error}"
                    )
                    continue
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
            line_start = masked.rfind("\n", 0, match.start()) + 1
            visibility_match = re.search(
                r"\bpub(?P<scope>\s*\([^)]*\))?\s*$",
                masked[line_start : match.start()],
            )
            visibility = "private"
            if visibility_match is not None:
                scope = visibility_match.group("scope")
                visibility = "pub" if scope is None else f"pub{''.join(scope.split())}"
            declaration = TypeDeclaration(
                name=name,
                kind=match.group("kind"),
                visibility=visibility,
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
    # An `impl Trait` argument is not an impl item and must not rebind `Self`.
    # Repository Rust is formatted before this gate, so a real impl item starts
    # at the beginning of a logical line (with optional indentation/`unsafe`).
    for declaration in IMPL_DECLARATION.finditer(masked):
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


def impl_ranges(
    path: Path, environment: CfgEnvironment | None = None
) -> list[tuple[int, int, str]]:
    """Return source ranges whose `Self` is bound to one concrete impl owner."""
    source = path.read_text()
    masked = mask_rust_non_code(source)
    ranges: list[tuple[int, int, str]] = []
    for declaration in IMPL_DECLARATION.finditer(masked):
        if environment is not None:
            item_start = source.rfind("\n", 0, declaration.start()) + 1
            attributes = preceding_outer_attributes(source, item_start)
            cfg_expressions = [
                cfg.group(1).strip()
                for attribute in attributes
                if (cfg := re.fullmatch(r"cfg\s*\((.*)\)", attribute, re.S))
                is not None
            ]
            if not all(
                evaluate_cfg(expression, environment)
                for expression in cfg_expressions
            ):
                continue
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
    command = [
        executable,
        "run",
        "--lang",
        "rust",
        "--kind",
        "ERROR",
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
            f"ast-grep parser-health query failed ({result.returncode}): "
            f"{result.stderr.strip()}"
        )
    elif result.stdout:
        sites = []
        for line in result.stdout.splitlines()[:8]:
            item = json.loads(line)
            start = item["range"]["start"]
            sites.append(f"{item['file']}:{start['line'] + 1}:{start['column'] + 1}")
        errors.append(
            "ast-grep cannot classify refinement flow over a syntax-error tree: "
            f"{sites}"
        )
        return ranges, errors
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
        aliases = {
            match.group("alias"): match.group("target")
            for match in re.finditer(
                r"\buse\s+(?:[A-Za-z_][A-Za-z0-9_]*\s*::\s*)*"
                r"(?P<target>[A-Za-z_][A-Za-z0-9_]*)\s+as\s+"
                r"(?P<alias>[A-Za-z_][A-Za-z0-9_]*)\s*;",
                masked,
            )
            if match.group("target") in enum_names
        }
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
            else:
                owner = aliases.get(owner, owner)
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
            "CanaryFreeFunctionPayload",
            "CanaryEvidencePayload",
            "CanaryUnregisteredEvidencePayload",
            "CanaryUnconstructedCapability",
            "CanaryExternalEvent",
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

    canary_functions, function_errors = functions(
        [CANARY], CfgEnvironment(True, frozenset())
    )
    errors.extend(function_errors)
    function_by_name = {
        function.name: (key, function)
        for key, function in canary_functions.items()
    }
    graph_seeds: list[OwnershipSeed] = []
    for name, kind in (
        ("canary_behavior_entry", "production_behavior"),
        ("canary_registered_evidence", "registered_model_evidence"),
    ):
        entry = function_by_name.get(name)
        if entry is None:
            errors.append(f"refinement ownership canary lacks function {name}")
            continue
        key, function = entry
        graph_seeds.append(
            OwnershipSeed(
                key,
                kind,
                "TP-CANARY",
                function.path,
                function.line,
                name,
            )
        )
    external_key = declaration_key(path, "CanaryExternalEvent")
    external = discovered[external_key]
    graph_seeds.append(
        OwnershipSeed(
            type_node(external_key),
            "production_behavior",
            "TP-CANARY",
            external.path,
            external.line,
            external.name,
        )
    )
    if not errors:
        inventory = reachable_inventory(
            [root for root in model_roots if root["role"] == "bound_event"],
            discovered,
            canary_functions,
            [CANARY],
            [CANARY],
            {"bound_event": "canary"},
            graph_seeds,
        )
        owned = {entry["name"]: entry for entry in inventory["types"]}
        unowned = {entry["name"] for entry in inventory["unreachable_types"]}
        expected_owned = {
            "CanaryPayload",
            "CanaryEvent",
            "CanaryBoundary",
            "CanaryFreeFunctionPayload",
            "CanaryEvidencePayload",
            "CanaryExternalEvent",
        }
        expected_unowned = {
            "CanaryUnregisteredEvidencePayload",
            "CanaryUnconstructedCapability",
        }
        if set(owned) != expected_owned or unowned != expected_unowned:
            errors.append(
                "refinement ownership graph canary drift: "
                f"owned={sorted(owned)}, unowned={sorted(unowned)}"
            )
        expected_sources = {
            "CanaryFreeFunctionPayload": ("semantic_root", "bound_event"),
            "CanaryBoundary": ("production_behavior", "TP-CANARY"),
            "CanaryEvidencePayload": (
                "registered_model_evidence",
                "TP-CANARY",
            ),
        }
        for name, source in expected_sources.items():
            observed = {
                (entry["kind"], entry["label"])
                for entry in owned.get(name, {}).get("owned_from", [])
            }
            if source not in observed:
                errors.append(
                    f"refinement ownership canary {name} lacks {source}: "
                    f"{sorted(observed)}"
                )
        free_function_witness = {
            (node["kind"], node["name"])
            for node in owned["CanaryFreeFunctionPayload"]["ownership_witness"][
                "nodes"
            ]
        }
        if ("function", "canary_root_payload") not in free_function_witness:
            errors.append(
                "refinement ownership canary did not traverse the free-function edge"
            )
        unreached_connectors = {
            (entry["path"], entry["name"])
            for entry in inventory["unreached_connector_functions"]
        }
        expected_unreached_connectors = {
            (
                "tx-pool/scripts/fixtures/model_refinement_canary.rs",
                "canary_unregistered_evidence",
            )
        }
        if unreached_connectors != expected_unreached_connectors:
            errors.append(
                "refinement connector-omission canary drift: "
                f"expected={sorted(expected_unreached_connectors)}, "
                f"observed={sorted(unreached_connectors)}"
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
        external_flow = flow.get(("CanaryExternalEvent", "External"), {})
        if external_flow.get("producers") or requires_internal_constructor(
            external.visibility
        ):
            errors.append(
                "refinement external-construction canary lost its public boundary"
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


def function_key(function: Function) -> str:
    return f"function:{function.path}:{function.line}:{function.name}"


def type_node(key: str) -> str:
    return f"type:{key}"


def function_body_range(masked: str, offset: int) -> tuple[int, int] | None:
    """Find a function body without treating its parameter list as a body."""

    parentheses = brackets = angles = 0
    cursor = offset
    while cursor < len(masked):
        character = masked[cursor]
        if character == "(":
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
        elif character == "{" and parentheses == brackets == angles == 0:
            closing = matching_brace(masked, cursor)
            return None if closing is None else (cursor, closing)
        elif character == ";" and parentheses == brackets == angles == 0:
            return None
        cursor += 1
    return None


def functions(
    paths: list[Path], environment: CfgEnvironment | None = None
) -> tuple[dict[str, Function], list[str]]:
    """Discover body-owning Rust functions and their concrete impl owner."""

    discovered: dict[str, Function] = {}
    errors: list[str] = []
    for path in paths:
        source = path.read_text()
        masked = mask_rust_non_code(source)
        relative = path.relative_to(REPO_ROOT).as_posix()
        all_owners = impl_ranges(path)
        owners = impl_ranges(path, environment)
        all_owner_starts = [start for start, _, _ in all_owners]
        owner_starts = [start for start, _, _ in owners]
        depth = 0
        depth_at = [0] * (len(masked) + 1)
        for index, character in enumerate(masked):
            depth_at[index] = depth
            if character == "{":
                depth += 1
            elif character == "}":
                depth -= 1
        depth_at[len(masked)] = depth
        for match in METHOD_DECLARATION.finditer(masked):
            all_owner_index = bisect_right(all_owner_starts, match.start()) - 1
            inside_impl = (
                all_owner_index >= 0
                and match.start() < all_owners[all_owner_index][1]
            )
            enabled_owner_index = bisect_right(owner_starts, match.start()) - 1
            inside_enabled_impl = (
                enabled_owner_index >= 0
                and match.start() < owners[enabled_owner_index][1]
            )
            if inside_impl and not inside_enabled_impl:
                continue
            if environment is not None:
                item_start = source.rfind("\n", 0, match.start()) + 1
                attributes = preceding_outer_attributes(source, item_start)
                cfg_expressions = [
                    cfg.group(1).strip()
                    for attribute in attributes
                    if (cfg := re.fullmatch(r"cfg\s*\((.*)\)", attribute, re.S))
                    is not None
                ]
                try:
                    if not all(
                        evaluate_cfg(expression, environment)
                        for expression in cfg_expressions
                    ):
                        continue
                except (ValueError, json.JSONDecodeError) as error:
                    errors.append(
                        f"cannot evaluate function cfg in {relative}:"
                        f"{source.count(chr(10), 0, match.start()) + 1}: {error}"
                    )
                    continue
            body_range = function_body_range(masked, match.end())
            if body_range is None:
                continue
            opening, closing = body_range
            owner = None
            if inside_enabled_impl:
                impl_start, _, candidate = owners[enabled_owner_index]
                if depth_at[match.start()] == depth_at[impl_start]:
                    owner = candidate
            line = source.count("\n", 0, match.start()) + 1
            item_start = source.rfind("\n", 0, match.start()) + 1
            attributes = preceding_outer_attributes(source, item_start)
            function = Function(
                name=match.group("name"),
                path=relative,
                line=line,
                signature=masked[match.start() : opening],
                source=masked[match.start() : closing + 1],
                owner=owner,
                is_test=any(
                    re.fullmatch(
                        r"(?:[A-Za-z_][A-Za-z0-9_]*::)*test(?:\s*\(.*\))?",
                        attribute,
                        re.S,
                    )
                    is not None
                    for attribute in attributes
                ),
            )
            key = function_key(function)
            if key in discovered:
                errors.append(f"duplicate function inventory key {key}")
            else:
                discovered[key] = function
    return discovered, errors


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


def rust_module_name(path: str) -> str:
    candidate = Path(path)
    return candidate.parent.name if candidate.name == "mod.rs" else candidate.stem


def referenced_type_nodes(
    source: str,
    path: str,
    declarations_by_name: dict[str, TypeDeclaration],
    names: dict[str, set[str]],
) -> set[str]:
    """Resolve type references locally, uniquely or by an explicit module path."""

    dependencies: set[str] = set()
    for identifier in set(IDENTIFIER.findall(source)):
        candidates = names.get(identifier, set())
        local = {
            candidate
            for candidate in candidates
            if declarations_by_name[candidate].path == path
        }
        if len(local) == 1:
            dependencies.add(type_node(next(iter(local))))
        elif len(candidates) == 1:
            dependencies.add(type_node(next(iter(candidates))))
    for match in QUALIFIED_IDENTIFIER.finditer(source):
        segments = re.split(r"\s*::\s*", match.group("qualified"))
        for index, identifier in enumerate(segments):
            candidates = names.get(identifier, set())
            if not candidates:
                continue
            if index == 0:
                continue
            hint = segments[index - 1]
            hinted = {
                candidate
                for candidate in candidates
                if rust_module_name(declarations_by_name[candidate].path) == hint
            }
            if len(hinted) == 1:
                dependencies.add(type_node(next(iter(hinted))))
    return dependencies


def called_function_nodes(
    source: str,
    path: str,
    functions_by_key: dict[str, Function],
    functions_by_name: dict[str, set[str]],
) -> set[str]:
    """Resolve free-function calls/values without conflating method names."""

    dependencies: set[str] = set()

    def resolve(name: str, qualified: str) -> str | None:
        candidates = {
            candidate
            for candidate in functions_by_name.get(name, set())
            if functions_by_key[candidate].owner is None
        }
        if not candidates:
            return None
        if qualified:
            segments = IDENTIFIER.findall(qualified)
            hint = segments[-1] if segments else ""
            if hint and hint not in {"crate", "self", "super"}:
                if hint[:1].isupper():
                    return None
                hinted = {
                    candidate
                    for candidate in candidates
                    if rust_module_name(functions_by_key[candidate].path) == hint
                }
                if len(hinted) == 1:
                    return next(iter(hinted))
                return None
        local = {
            candidate
            for candidate in candidates
            if functions_by_key[candidate].path == path
        }
        if len(local) == 1:
            return next(iter(local))
        if len(candidates) == 1:
            return next(iter(candidates))
        return None

    for match in CALL_REFERENCE.finditer(source):
        prefix = source[max(0, match.start() - 24) : match.start()]
        if prefix.rstrip().endswith(".") or re.search(r"\bfn\s*$", prefix):
            continue
        dependency = resolve(match.group("name"), match.group("qualified"))
        if dependency is not None:
            dependencies.add(dependency)
    for match in FUNCTION_VALUE_REFERENCE.finditer(source):
        dependency = resolve(match.group("name"), match.group("qualified"))
        if dependency is not None:
            dependencies.add(dependency)
    return dependencies


def production_behavior_seeds(
    registry: dict[str, object],
    declarations_by_name: dict[str, TypeDeclaration],
    functions_by_key: dict[str, Function],
    paths: set[str],
) -> tuple[list[OwnershipSeed], list[dict[str, str]], list[str]]:
    """Derive production entry points from the rule-to-owner registry."""

    functions_by_path_name: dict[tuple[str, str], list[tuple[str, Function]]] = {}
    for key, function in functions_by_key.items():
        functions_by_path_name.setdefault((function.path, function.name), []).append(
            (key, function)
        )
    seeds: list[OwnershipSeed] = []
    non_connectors: list[dict[str, str]] = []
    errors: list[str] = []
    for behavior in registry.get("behaviors", []):
        if not isinstance(behavior, dict) or not isinstance(behavior.get("id"), str):
            continue
        behavior_id = behavior["id"]
        for owner in behavior.get("implementation_owners", []):
            if not isinstance(owner, dict) or owner.get("path") not in paths:
                continue
            path = owner["path"]
            for symbol in owner.get("symbols", []):
                identifiers = IDENTIFIER.findall(symbol) if isinstance(symbol, str) else []
                if not identifiers:
                    continue
                name = identifiers[-1]
                declaration = declarations_by_name.get(declaration_key(path, name))
                candidates = functions_by_path_name.get((path, name), [])
                explicit_function = re.search(r"\bfn\b", symbol) is not None
                explicit_type = re.search(r"\b(?:enum|struct)\b", symbol) is not None
                if declaration is not None and not explicit_function:
                    seeds.append(
                        OwnershipSeed(
                            type_node(declaration_key(path, name)),
                            "production_behavior",
                            behavior_id,
                            path,
                            declaration.line,
                            symbol,
                        )
                    )
                elif candidates and not explicit_type:
                    for key, function in candidates:
                        seeds.append(
                            OwnershipSeed(
                                key,
                                "production_behavior",
                                behavior_id,
                                path,
                                function.line,
                                symbol,
                            )
                        )
                else:
                    non_connectors.append(
                        {"behavior_id": behavior_id, "path": path, "symbol": symbol}
                    )
    unique = {
        (seed.node, seed.kind, seed.label, seed.path, seed.line, seed.symbol): seed
        for seed in seeds
    }
    return list(unique.values()), non_connectors, errors


def model_evidence_seeds(
    registry: dict[str, object],
    functions_by_key: dict[str, Function],
    paths: set[str],
) -> tuple[list[OwnershipSeed], list[str]]:
    """Bind only registered mathematical-model tests into the proof graph."""

    functions_by_name: dict[str, list[tuple[str, Function]]] = {}
    for key, function in functions_by_key.items():
        if function.path in paths:
            functions_by_name.setdefault(function.name, []).append((key, function))
    seeds: list[OwnershipSeed] = []
    errors: list[str] = []
    for evidence in registry.get("unit_evidence", []):
        if not isinstance(evidence, dict):
            continue
        test = evidence.get("test")
        behavior_id = evidence.get("behavior_id")
        if (
            not isinstance(test, str)
            or not isinstance(behavior_id, str)
        ):
            continue
        name = test.rsplit("::", 1)[-1]
        candidates = [
            (key, function)
            for key, function in functions_by_name.get(name, [])
            if function.is_test
        ]
        if len(candidates) != 1:
            errors.append(
                f"registered model evidence {test!r} resolves to "
                f"{len(candidates)} test functions"
            )
            continue
        key, function = candidates[0]
        seeds.append(
            OwnershipSeed(
                key,
                "registered_model_evidence",
                behavior_id,
                function.path,
                function.line,
                test,
            )
        )
    return seeds, errors


def reachable_inventory(
    roots: list[dict[str, str]],
    declarations_by_name: dict[str, TypeDeclaration],
    functions_by_key: dict[str, Function],
    declaration_paths: list[Path],
    reference_paths: list[Path],
    role_bindings: dict[str, str],
    ownership_seeds: list[OwnershipSeed],
    non_connector_owner_symbols: list[dict[str, str]] | None = None,
    source_variant_flow: dict[tuple[str, str], dict[str, object]] | None = None,
    expanded_variant_flow: dict[tuple[str, str], dict[str, object]] | None = None,
    source_construction_flow: dict[str, list[dict[str, object]]] | None = None,
    expanded_construction_flow: dict[str, list[dict[str, object]]] | None = None,
) -> dict[str, object]:
    names: dict[str, set[str]] = {}
    for key, declaration in declarations_by_name.items():
        names.setdefault(declaration.name, set()).add(key)

    functions_by_name: dict[str, set[str]] = {}
    for key, function in functions_by_key.items():
        functions_by_name.setdefault(function.name, set()).add(key)

    def owner_key(function: Function) -> str | None:
        if function.owner is None:
            return None
        candidates = names.get(function.owner, set())
        local = {
            candidate
            for candidate in candidates
            if declarations_by_name[candidate].path == function.path
        }
        if len(local) == 1:
            return next(iter(local))
        if len(candidates) == 1:
            return next(iter(candidates))
        return None

    owner_by_function = {
        key: owner_key(function) for key, function in functions_by_key.items()
    }

    def type_functions(key: str) -> list[tuple[str, Function]]:
        return sorted(
            [
                (function_key_value, functions_by_key[function_key_value])
                for function_key_value, owner in owner_by_function.items()
                if owner == key
            ],
            key=lambda entry: (entry[1].path, entry[1].line, entry[1].name),
        )

    dependency_graph: dict[str, set[str]] = {
        type_node(key): set() for key in declarations_by_name
    }
    dependency_graph.update({key: set() for key in functions_by_key})
    for key, declaration in declarations_by_name.items():
        owned_functions = type_functions(key)
        node = type_node(key)
        dependency_graph[node].update(
            referenced_type_nodes(
                declaration.source, declaration.path, declarations_by_name, names
            )
        )
        dependency_graph[node].update(
            function_key_value for function_key_value, _ in owned_functions
        )
    for key, function in functions_by_key.items():
        dependency_graph[key].update(
            referenced_type_nodes(
                function.source, function.path, declarations_by_name, names
            )
        )
        dependency_graph[key].update(
            called_function_nodes(
                function.source,
                function.path,
                functions_by_key,
                functions_by_name,
            )
        )
        if owner_by_function[key] is not None:
            dependency_graph[key].add(type_node(owner_by_function[key]))

    semantic_seeds = [
        OwnershipSeed(
            type_node(declaration_key(root["path"], root["type"])),
            "semantic_root",
            root["role"],
            root["path"],
            declarations_by_name[
                declaration_key(root["path"], root["type"])
            ].line,
            root["type"],
        )
        for root in roots
    ]
    unique_seeds = {
        (seed.node, seed.kind, seed.label, seed.path, seed.line, seed.symbol): seed
        for seed in [*semantic_seeds, *ownership_seeds]
    }
    all_seeds = sorted(
        unique_seeds.values(),
        key=lambda seed: (
            seed.kind,
            seed.label,
            seed.path,
            seed.line,
            seed.symbol,
        ),
    )
    node_ownership: dict[str, set[tuple[str, str]]] = {}
    node_witness: dict[
        str, tuple[tuple[object, ...], OwnershipSeed, list[str]]
    ] = {}
    kind_priority = {
        "semantic_root": 0,
        "production_behavior": 1,
        "registered_model_evidence": 2,
    }
    for seed in all_seeds:
        if seed.node not in dependency_graph:
            continue
        visited = {seed.node}
        distance = {seed.node: 0}
        predecessor: dict[str, str | None] = {seed.node: None}
        frontier = deque([seed.node])
        while frontier:
            node = frontier.popleft()
            for dependency in sorted(dependency_graph[node].difference(visited)):
                visited.add(dependency)
                distance[dependency] = distance[node] + 1
                predecessor[dependency] = node
                frontier.append(dependency)
        for node in visited:
            node_ownership.setdefault(node, set()).add((seed.kind, seed.label))
            if not node.startswith("type:"):
                continue
            score = (
                distance[node],
                kind_priority[seed.kind],
                seed.label,
                seed.path,
                seed.line,
                seed.symbol,
            )
            current = node_witness.get(node)
            if current is not None and current[0] <= score:
                continue
            path = []
            cursor: str | None = node
            while cursor is not None:
                path.append(cursor)
                cursor = predecessor[cursor]
            path.reverse()
            node_witness[node] = (score, seed, path)
    reachable_types = {
        key for key in declarations_by_name if type_node(key) in node_ownership
    }
    graph_reachable_functions = {
        key for key in functions_by_key if key in node_ownership
    }
    inventory_paths = {
        path.relative_to(REPO_ROOT).as_posix() for path in declaration_paths
    }
    inventory_function_keys = {
        key
        for key, function in functions_by_key.items()
        if function.path in inventory_paths
    }
    reachable_functions = graph_reachable_functions.intersection(
        inventory_function_keys
    )

    def describe_node(node: str) -> dict[str, object]:
        if node.startswith("type:"):
            declaration = declarations_by_name[node.removeprefix("type:")]
            return {
                "kind": "type",
                "name": declaration.name,
                "path": declaration.path,
                "line": declaration.line,
            }
        function = functions_by_key[node]
        return {
            "kind": "function",
            "name": function.name,
            "path": function.path,
            "line": function.line,
            "signature_sha256": hashlib.sha256(
                function.signature.encode()
            ).hexdigest(),
        }

    def ownership_witness(node: str) -> dict[str, object]:
        _, seed, path = node_witness[node]
        return {
            "source": {
                "kind": seed.kind,
                "label": seed.label,
                "path": seed.path,
                "line": seed.line,
                "symbol": seed.symbol,
            },
            "nodes": [describe_node(path_node) for path_node in path],
        }

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
    for key in sorted(reachable_types):
        declaration = declarations_by_name[key]
        name = declaration.name
        reachable_methods = [function for _, function in type_functions(key)]
        owners = node_ownership[type_node(key)]
        semantic_roles = {
            label for kind, label in owners if kind == "semantic_root"
        }
        for primitive in SYNC_PRIMITIVES:
            primitive_counts[primitive] += len(
                re.findall(rf"\b{re.escape(primitive)}\b", declaration.source)
            )
        dependencies = sorted(
            dependency.removeprefix("type:")
            for dependency in dependency_graph[type_node(key)]
            if dependency.startswith("type:")
            and dependency.removeprefix("type:") in reachable_types
            and dependency != type_node(key)
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
            "visibility": declaration.visibility,
            "path": declaration.path,
            "line": declaration.line,
            "dependencies": dependencies,
            "reachable_from": sorted(semantic_roles),
            "semantic_bindings": sorted(
                {role_bindings[role] for role in semantic_roles}
            ),
            "owned_from": [
                {"kind": kind, "label": label}
                for kind, label in sorted(owners)
            ],
            "ownership_witness": ownership_witness(type_node(key)),
            "behavior_ids": sorted(
                {
                    label
                    for kind, label in owners
                    if kind
                    in {"production_behavior", "registered_model_evidence"}
                }
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
        if key in reachable_types:
            continue
        name = declaration.name
        declaration_methods = [function for _, function in type_functions(key)]
        impl_source = "\n".join(method.source for method in declaration_methods)
        self_variant_counts = Counter(
            re.findall(r"\bSelf\s*::\s*([A-Za-z_][A-Za-z0-9_]*)\b", impl_source)
        )
        type_entry = {
            "name": name,
            "kind": declaration.kind,
            "visibility": declaration.visibility,
            "path": declaration.path,
            "line": declaration.line,
            "dependencies": sorted(
                dependency.removeprefix("type:")
                for dependency in dependency_graph[type_node(key)]
                if dependency.startswith("type:")
                and dependency != type_node(key)
            ),
            "reachable_from": [],
            "semantic_bindings": [],
            "owned_from": [],
            "ownership_witness": None,
            "behavior_ids": [],
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
    for key in reachable_functions:
        function = functions_by_key[key]
        for primitive in SYNC_PRIMITIVES:
            primitive_counts[primitive] += len(
                re.findall(rf"\b{re.escape(primitive)}\b", function.source)
            )

    def function_entry(key: str) -> dict[str, object]:
        function = functions_by_key[key]
        owners = node_ownership.get(key, set())
        return {
            "name": function.name,
            "path": function.path,
            "line": function.line,
            "owner": function.owner,
            "is_test": function.is_test,
            "owned_from": [
                {"kind": kind, "label": label} for kind, label in sorted(owners)
            ],
            "type_dependencies": sorted(
                dependency.removeprefix("type:")
                for dependency in dependency_graph[key]
                if dependency.startswith("type:")
            ),
            "calls": [
                {
                    "name": functions_by_key[dependency].name,
                    "path": functions_by_key[dependency].path,
                    "line": functions_by_key[dependency].line,
                }
                for dependency in sorted(dependency_graph[key])
                if dependency in functions_by_key
            ],
        }

    function_inventory = [function_entry(key) for key in sorted(reachable_functions)]
    unreachable_functions = [
        function_entry(key)
        for key in sorted(inventory_function_keys.difference(reachable_functions))
    ]
    graph_scope = (
        "enum/struct necessity census; functions and methods are directional "
        "connectors, while aliases, constants, macros and traits remain named "
        "syntax boundaries rather than census items"
    )

    def compact_witness(entry: dict[str, object]) -> dict[str, object] | None:
        witness = entry["ownership_witness"]
        if witness is None:
            return None
        return {
            "source": {
                key: value
                for key, value in witness["source"].items()
                if key != "line"
            },
            "nodes": [
                {key: value for key, value in node.items() if key != "line"}
                for node in witness["nodes"]
            ],
        }

    necessity_payload = {
        "schema_version": 1,
        "scope": graph_scope,
        "source_files": [
            path.relative_to(REPO_ROOT).as_posix() for path in declaration_paths
        ],
        "reference_files": [
            path.relative_to(REPO_ROOT).as_posix() for path in reference_paths
        ],
        "non_connector_owner_symbols": sorted(
            non_connector_owner_symbols or [],
            key=lambda entry: (
                entry["behavior_id"],
                entry["path"],
                entry["symbol"],
            ),
        ),
        "types": [
            {
                "name": entry["name"],
                "kind": entry["kind"],
                "visibility": entry["visibility"],
                "path": entry["path"],
                "dependencies": entry["dependencies"],
                "variants": [variant["name"] for variant in entry["variants"]],
                "owned_from": entry["owned_from"],
                "ownership_witness": compact_witness(entry),
            }
            for entry in sorted(
                [*types, *unreachable_types],
                key=lambda entry: (entry["path"], entry["name"]),
            )
        ],
    }
    return {
        "roots": roots,
        "ownership_graph_scope": graph_scope,
        "necessity_sha256": canonical_sha256(necessity_payload),
        "ownership_entrypoints": [
            {
                "kind": seed.kind,
                "label": seed.label,
                "path": seed.path,
                "line": seed.line,
                "symbol": seed.symbol,
            }
            for seed in all_seeds
        ],
        "non_connector_owner_symbols": non_connector_owner_symbols or [],
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
        "connector_functions": function_inventory,
        "unreached_connector_functions": unreachable_functions,
    }


def derive(args: argparse.Namespace) -> tuple[dict[str, object], list[str]]:
    try:
        contract = json.loads(CONTRACT.read_text())
    except (OSError, json.JSONDecodeError) as error:
        return {}, [f"cannot load architecture contract: {error}"]
    refinement = contract.get("refinement_inventory")
    if not isinstance(refinement, dict) or refinement.get("schema_version") != 2:
        return {}, ["architecture contract refinement_inventory schema must be 2"]

    source_role_census, source_role_errors = derive_source_role_census(refinement)
    if source_role_census is None:
        return {}, source_role_errors or ["source-role census was not generated"]
    compile_roles = source_role_census["compile_roles"]
    model_paths = [
        repo_path(path)
        for path in compile_roles["test_model_only"]["files"]
    ]
    test_support_paths = [
        repo_path(path)
        for path in compile_roles["test_support_only"]["files"]
    ]
    model_reference_paths = [*model_paths, *test_support_paths]
    production_paths = [
        repo_path(path)
        for path in compile_roles["default_production"]["files"]
    ]
    production_reference_paths = production_paths
    model_environment = CfgEnvironment(
        True,
        frozenset(source_role_census["cargo"]["internal_features"]),
    )
    production_environment = CfgEnvironment(
        False,
        frozenset(source_role_census["cargo"]["default_features"]),
    )
    model_declarations, model_declaration_errors = declarations(
        model_paths,
        model_environment,
    )
    production_declarations, production_declaration_errors = declarations(
        production_paths,
        production_environment,
    )
    model_functions, model_function_errors = functions(
        model_reference_paths, model_environment
    )
    production_functions, production_function_errors = functions(
        production_paths, production_environment
    )
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
    model_seeds, model_seed_errors = model_evidence_seeds(
        registry,
        model_functions,
        {
            path.relative_to(REPO_ROOT).as_posix()
            for path in model_reference_paths
        },
    )
    production_seeds, non_connector_production_owners, production_seed_errors = (
        production_behavior_seeds(
            registry,
            production_declarations,
            production_functions,
            {
                path.relative_to(REPO_ROOT).as_posix()
                for path in production_paths
            },
        )
    )
    bindings, model_role_bindings, production_role_bindings, binding_errors = (
        validate_bindings(
            refinement.get("semantic_bindings"),
            model_roots,
            production_roots,
            behavior_ids,
        )
    )
    errors = [
        *source_role_errors,
        *model_declaration_errors,
        *production_declaration_errors,
        *model_function_errors,
        *production_function_errors,
        *model_root_errors,
        *production_root_errors,
        *model_seed_errors,
        *production_seed_errors,
        *binding_errors,
    ]
    if errors:
        return {}, errors
    if source_role_census is None:
        return {}, ["source-role census was not generated"]
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
            model_reference_paths, model_declarations
        )
        production_variant_flow, production_flow_errors = variant_flow(
            production_reference_paths, production_declarations
        )
        model_construction_flow, model_construction_errors = construction_flow(
            model_reference_paths, model_declarations
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
                    # rustc's pretty-printed expansion attaches this lint-only
                    # attribute directly to expressions. That output is valid
                    # compiler AST but not round-trippable Rust for tree-sitter;
                    # removing the non-semantic lint marker restores an exact
                    # parse without changing any constructor or pattern.
                    expanded_source = EXPANDED_LINT_ATTRIBUTE.sub("", result.stdout)
                    temporary = tempfile.NamedTemporaryFile(
                        mode="w", suffix=".rs", delete=False
                    )
                    with temporary:
                        temporary.write(expanded_source)
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
    model_inventory = reachable_inventory(
        model_roots,
        model_declarations,
        model_functions,
        model_paths,
        model_reference_paths,
        model_role_bindings,
        model_seeds,
        source_variant_flow=model_variant_flow,
        source_construction_flow=model_construction_flow,
    )
    production_inventory = reachable_inventory(
        production_roots,
        production_declarations,
        production_functions,
        production_paths,
        production_reference_paths,
        production_role_bindings,
        production_seeds,
        non_connector_production_owners,
        production_variant_flow,
        expanded_variant_flow,
        production_construction_flow,
        expanded_construction_flow,
    )
    inventory = {
        "schema_version": 2,
        "authority": "tx-pool/architecture-contract.json#refinement_inventory",
        "semantic_bindings": bindings,
        "source_role_census": source_role_census,
        "necessity_census_sha256": canonical_sha256(
            {
                "source_role_census_sha256": source_role_census["census_sha256"],
                "model": model_inventory["necessity_sha256"],
                "production": production_inventory["necessity_sha256"],
            }
        ),
        "model": model_inventory,
        "production": production_inventory,
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
    source_roles = inventory["source_role_census"]["compile_roles"]
    if model["unreachable_types"] or production["unreachable_types"]:
        print(
            "error: generated state-bearing ownership graph retains unowned types",
            file=sys.stderr,
        )
        return 1
    if (
        model["unreached_connector_functions"]
        or production["unreached_connector_functions"]
    ):
        print(
            "error: generated state-bearing ownership graph retains unowned "
            "connector functions",
            file=sys.stderr,
        )
        return 1
    if inventory["source_role_census"]["unreferenced_rust_sources"]:
        print(
            "error: generated compile-role census retains unreferenced Rust sources",
            file=sys.stderr,
        )
        return 1
    flow_suffix = ""
    if args.variant_flow:
        model_variants = [
            (type_entry, variant)
            for type_entry in model["types"]
            for variant in type_entry["variants"]
        ]
        production_variants = [
            (type_entry, variant)
            for type_entry in production["types"]
            for variant in type_entry["variants"]
        ]
        source_constructorless = sum(
            requires_internal_constructor(type_entry["visibility"])
            and not variant["source_flow"]["producers"]
            for type_entry, variant in production_variants
        )
        expanded_constructorless = (
            sum(
                requires_internal_constructor(type_entry["visibility"])
                and not variant["expanded_flow"]["producers"]
                for type_entry, variant in production_variants
            )
            if args.expanded_production is not None or args.cargo_expand_production
            else source_constructorless
        )
        model_constructorless = sum(
            not variant["source_flow"]["producers"]
            for _, variant in model_variants
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
        "validated mechanically derived state-bearing refinement frontier: "
        f"model={len(model['types'])} owned/"
        f"{len(model['unreachable_types'])} outside ownership graph/"
        f"{len(model['source_files'])} files, production={len(production['types'])} "
        f"owned/{len(production['unreachable_types'])} outside ownership graph/"
        f"{len(production['source_files'])} files; compile roles="
        f"default:{source_roles['default_production']['file_count']},"
        f"internal:{source_roles['internal_feature_only']['file_count']},"
        f"model:{source_roles['test_model_only']['file_count']},"
        f"test-support:{source_roles['test_support_only']['file_count']}"
        f"{flow_suffix}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
