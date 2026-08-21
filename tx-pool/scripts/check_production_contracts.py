#!/usr/bin/env python3
"""Validate cross-crate production contracts that Rust types cannot seal."""

from __future__ import annotations

import copy
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys


REPO_ROOT = Path(__file__).resolve().parents[2]
TX_POOL_SRC = REPO_ROOT / "tx-pool" / "src"
TX_POOL_ARCHITECTURE_CONTRACT = REPO_ROOT / "tx-pool" / "architecture-contract.json"
TX_POOL_BEHAVIOR_REGISTRY = REPO_ROOT / "tx-pool" / "review-behaviors.json"
TX_POOL_SECURITY_MANIFEST = REPO_ROOT / "tx-pool" / "security-regression-manifest.json"
CHAIN_VERIFY = REPO_ROOT / "chain" / "src" / "verify.rs"
CHAIN_SERVICE = REPO_ROOT / "chain" / "src" / "chain_service.rs"
SHARED_BUILDER = REPO_ROOT / "shared" / "src" / "shared_builder.rs"
CKB_SETUP = REPO_ROOT / "ckb-bin" / "src" / "setup.rs"
CKB_REPLAY = REPO_ROOT / "ckb-bin" / "src" / "subcommand" / "replay.rs"
PROPOSAL_TABLE = REPO_ROOT / "util" / "proposal-table" / "src" / "lib.rs"
BLOCK_VERIFIER = REPO_ROOT / "verification" / "src" / "block_verifier.rs"
VERIFICATION_TRAITS = REPO_ROOT / "verification" / "traits" / "src" / "lib.rs"
CONTEXTUAL_BLOCK_VERIFIER = (
    REPO_ROOT / "verification" / "contextual" / "src" / "contextual_block_verifier.rs"
)
CONTEXTUAL_UNCLES_VERIFIER = (
    REPO_ROOT / "verification" / "contextual" / "src" / "uncles_verifier.rs"
)
TX_POOL_SERVICE = REPO_ROOT / "tx-pool" / "src" / "service.rs"
TX_POOL_CONTROLLER = REPO_ROOT / "tx-pool" / "src" / "service" / "controller.rs"
TX_POOL_BUILDER = REPO_ROOT / "tx-pool" / "src" / "service" / "builder.rs"
TX_POOL_DISPATCH = REPO_ROOT / "tx-pool" / "src" / "service" / "dispatch.rs"
TX_POOL_MESSAGE = REPO_ROOT / "tx-pool" / "src" / "service" / "message.rs"
SYNC_GET_TRANSACTIONS = (
    REPO_ROOT / "sync" / "src" / "relayer" / "get_transactions_process.rs"
)
TX_POOL_AUTHORITY_SERVICE = (
    REPO_ROOT / "tx-pool" / "src" / "authority" / "service.rs"
)
TX_POOL_AUTHORITY_RUNTIME = (
    REPO_ROOT / "tx-pool" / "src" / "authority" / "runtime.rs"
)
TX_POOL_AUTHORITY_CHAIN_BOUNDARY = (
    REPO_ROOT / "tx-pool" / "src" / "authority" / "chain_boundary.rs"
)
TX_POOL_AUTHORITY_INGRESS = (
    REPO_ROOT / "tx-pool" / "src" / "authority" / "ingress.rs"
)
TX_POOL_AUTHORITY_BAN = REPO_ROOT / "tx-pool" / "src" / "authority" / "ban.rs"
TX_POOL_AUTHORITY_PUBLISHER = (
    REPO_ROOT / "tx-pool" / "src" / "authority" / "publisher.rs"
)
TX_POOL_AUTHORITY_EFFECT = (
    REPO_ROOT / "tx-pool" / "src" / "authority" / "effect.rs"
)
TX_POOL_AUTHORITY_INDEXES = (
    REPO_ROOT / "tx-pool" / "src" / "authority" / "indexes.rs"
)
TX_POOL_AUTHORITY_COMPUTE_COORDINATOR = (
    REPO_ROOT / "tx-pool" / "src" / "authority" / "compute_coordinator.rs"
)
TX_POOL_AUTHORITY_SCHEDULER = (
    REPO_ROOT / "tx-pool" / "src" / "authority" / "scheduler.rs"
)
TX_POOL_AUTHORITY_TOPOLOGY = (
    REPO_ROOT / "tx-pool" / "src" / "authority" / "topology.rs"
)
TX_POOL_AUTHORITY_TEMPLATE_DRIVER = (
    REPO_ROOT / "tx-pool" / "src" / "authority" / "template_driver.rs"
)
TX_POOL_AUTHORITY_TEMPLATE = (
    REPO_ROOT / "tx-pool" / "src" / "authority" / "template.rs"
)
TX_POOL_AUTHORITY_PACKING = (
    REPO_ROOT / "tx-pool" / "src" / "authority" / "packing.rs"
)
TX_POOL_BLOCK_ASSEMBLER_NOTIFY = (
    REPO_ROOT / "tx-pool" / "src" / "block_assembler" / "notify.rs"
)
TX_POOL_BLOCKING_TEST_SERVICE = (
    REPO_ROOT / "tx-pool" / "src" / "tests" / "blocking_service.rs"
)
SYNC_RELAYER_TEST_HELPER = (
    REPO_ROOT / "sync" / "src" / "relayer" / "tests" / "helper.rs"
)
RPC_TEST_SETUP = REPO_ROOT / "rpc" / "src" / "tests" / "setup.rs"
RPC_TEST_MOD = REPO_ROOT / "rpc" / "src" / "tests" / "mod.rs"
TX_POOL_MODEL_PROTOCOL = (
    REPO_ROOT / "tx-pool" / "src" / "tests" / "model" / "protocol.rs"
)
TX_POOL_BLOCK_ASSEMBLER = REPO_ROOT / "tx-pool" / "src" / "block_assembler" / "mod.rs"
TX_POOL_CANDIDATE_UNCLES = (
    REPO_ROOT / "tx-pool" / "src" / "block_assembler" / "candidate_uncles.rs"
)
TX_POOL_AUTHORITY_WORKER = (
    REPO_ROOT / "tx-pool" / "src" / "authority" / "worker.rs"
)
TX_POOL_BENCHMARK = REPO_ROOT / "tx-pool" / "src" / "benchmark.rs"
TX_POOL_AUTHORITY_PLAN = REPO_ROOT / "tx-pool" / "src" / "authority" / "plan.rs"
TX_POOL_AUTHORITY_DEPENDENCY = (
    REPO_ROOT / "tx-pool" / "src" / "authority" / "dependency.rs"
)
TX_POOL_AUTHORITY_SETTLEMENT = (
    REPO_ROOT / "tx-pool" / "src" / "authority" / "plan" / "settlement.rs"
)
TX_POOL_AUTHORITY_COMPUTE_EXCHANGE = (
    REPO_ROOT / "tx-pool" / "src" / "authority" / "plan" / "compute_exchange.rs"
)
TX_POOL_AUTHORITY_CHAIN_TRANSITION = (
    REPO_ROOT / "tx-pool" / "src" / "authority" / "plan" / "chain_transition.rs"
)
TX_POOL_AUTHORITY_INGRESS_PLAN = (
    REPO_ROOT / "tx-pool" / "src" / "authority" / "plan" / "ingress.rs"
)
TX_POOL_AUTHORITY_QUERY = REPO_ROOT / "tx-pool" / "src" / "authority" / "query.rs"
TX_POOL_AUTHORITY_READ = REPO_ROOT / "tx-pool" / "src" / "authority" / "read.rs"
TX_POOL_AUTHORITY_RESOURCES = (
    REPO_ROOT / "tx-pool" / "src" / "authority" / "resources.rs"
)
TX_POOL_AUTHORITY_STATE = REPO_ROOT / "tx-pool" / "src" / "authority" / "state.rs"
TX_POOL_MODEL_BOUNDARIES = (
    REPO_ROOT / "tx-pool" / "src" / "tests" / "model" / "boundaries.rs"
)
TX_POOL_AUTHORITY_WORK = REPO_ROOT / "tx-pool" / "src" / "authority" / "work.rs"
TX_POOL_AUTHORITY_CHAIN = REPO_ROOT / "tx-pool" / "src" / "authority" / "chain.rs"
TX_POOL_AUTHORITY_VALIDATION = (
    REPO_ROOT / "tx-pool" / "src" / "authority" / "validation.rs"
)
TX_POOL_AUTHORITY_REJECTION = (
    REPO_ROOT / "tx-pool" / "src" / "authority" / "rejection.rs"
)
TX_POOL_AUTHORITY_RESOLVER = (
    REPO_ROOT / "tx-pool" / "src" / "authority" / "resolver.rs"
)
TX_POOL_AUTHORITY_RESIDENCY = (
    REPO_ROOT / "tx-pool" / "src" / "authority" / "residency.rs"
)
TX_POOL_UTIL = REPO_ROOT / "tx-pool" / "src" / "util.rs"
TX_POOL_AUTHORITY_MEMBERSHIP = (
    REPO_ROOT / "tx-pool" / "src" / "authority" / "plan" / "membership.rs"
)
TX_POOL_AUTHORITY_MEMBERSHIP_EVICTION = (
    REPO_ROOT
    / "tx-pool"
    / "src"
    / "authority"
    / "plan"
    / "membership"
    / "eviction.rs"
)
RUST_CHAR_LITERAL = re.compile(
    r"'(?:[^'\\\r\n]|\\(?:[nrt0\\'\"]|x[0-9A-Fa-f]{2}|u\{[0-9A-Fa-f_]{1,6}\}))'"
)
RUST_RAW_STRING = re.compile(r'(?:br|cr|r)(?P<hashes>#{0,255})"')
AUTHORITY_MUTATION = re.compile(r"\.\s*apply(?:_[a-z][A-Za-z0-9_]*)?\s*\(")
POST_COMMIT_PUBLICATION = re.compile(
    r"\.\s*(?:publish_committed|publish_post_commit(?:_pair)?)\s*\("
)
EARLY_EXIT = re.compile(r"\b(?:return|break|continue)\b|\?")
RUST_FILE_MODULE = re.compile(
    r"\b(?:pub(?:\s*\([^)]*\))?\s+)?mod\s+"
    r"(?P<name>[A-Za-z_][A-Za-z0-9_]*)\s*;"
)
RUST_PATH_ATTRIBUTE = re.compile(
    r'#\s*\[\s*path\s*=\s*"(?P<path>[^"]+)"\s*\]'
)
INFALLIBLE_SCRATCH_CONSTRUCTION = (
    (
        "iterator collect",
        re.compile(r"\.\s*collect\s*(?:::\s*<[^;{}]*?>)?\s*\("),
    ),
    ("vec! construction", re.compile(r"\bvec\s*!\s*\[")),
    (
        "infallible capacity construction",
        re.compile(r"\b(?:HashMap|HashSet|Vec|VecDeque)::with_capacity\s*\("),
    ),
)
RETIRED_ALLOCATION_RETRY_VOCABULARY = (
    "TEMPLATE_ALLOCATION_RETRY",
    "TRANSIENT_RETRY_DELAY",
    "allocation_backoff_or_cancel",
    "backoff_until",
    "wait_template_retry",
    "TemplateRetryWake",
)


def mask_rust_non_code(source: str) -> str:
    """Mask comments and literals while preserving byte offsets and newlines."""

    masked = list(source)

    def blank(start: int, end: int) -> None:
        for index in range(start, end):
            if source[index] != "\n":
                masked[index] = " "

    cursor = 0
    while cursor < len(source):
        if source.startswith("//", cursor):
            end = source.find("\n", cursor + 2)
            end = len(source) if end < 0 else end
            blank(cursor, end)
            cursor = end
            continue
        if source.startswith("/*", cursor):
            start = cursor
            cursor += 2
            depth = 1
            while cursor < len(source) and depth:
                if source.startswith("/*", cursor):
                    depth += 1
                    cursor += 2
                elif source.startswith("*/", cursor):
                    depth -= 1
                    cursor += 2
                else:
                    cursor += 1
            if depth:
                raise ValueError("unterminated Rust block comment")
            blank(start, cursor)
            continue

        raw = RUST_RAW_STRING.match(source, cursor)
        if raw is not None and (
            cursor == 0
            or not (source[cursor - 1].isalnum() or source[cursor - 1] == "_")
        ):
            closing = '"' + raw.group("hashes")
            end = source.find(closing, raw.end())
            if end < 0:
                raise ValueError("unterminated Rust raw string")
            end += len(closing)
            blank(cursor, end)
            cursor = end
            continue

        if source[cursor] == '"':
            start = cursor
            cursor += 1
            while cursor < len(source):
                if source[cursor] == "\\":
                    cursor += 2
                elif source[cursor] == '"':
                    cursor += 1
                    break
                else:
                    cursor += 1
            else:
                raise ValueError("unterminated Rust string")
            blank(start, min(cursor, len(source)))
            continue

        character = RUST_CHAR_LITERAL.match(source, cursor)
        if character is not None:
            blank(cursor, character.end())
            cursor = character.end()
            continue
        cursor += 1
    return "".join(masked)


def matching_brace(masked: str, opening: int) -> int | None:
    if opening < 0 or opening >= len(masked) or masked[opening] != "{":
        return None
    depth = 1
    for cursor in range(opening + 1, len(masked)):
        if masked[cursor] == "{":
            depth += 1
        elif masked[cursor] == "}":
            depth -= 1
            if depth == 0:
                return cursor
    return None


def rust_impl_methods(
    source: str, impl_name: str, *, allow_multiple: bool = False
) -> list[tuple[str, str, int]]:
    """Return concrete inherent-impl method bodies as masked source."""

    masked = mask_rust_non_code(source)
    declarations = list(
        re.finditer(
            rf"\bimpl(?:\s*<[^{{}}]*>)?\s+{re.escape(impl_name)}"
            rf"(?:\s*<[^{{}}]*>)?\s*\{{",
            masked,
        )
    )
    if not declarations or (not allow_multiple and len(declarations) != 1):
        raise ValueError(
            f"expected {'one or more' if allow_multiple else 'one'} inherent impl "
            f"{impl_name}, found {len(declarations)}"
        )

    methods: list[tuple[str, str, int]] = []
    for declaration in declarations:
        opening = masked.find("{", declaration.start())
        closing = matching_brace(masked, opening)
        if closing is None:
            raise ValueError(f"inherent impl {impl_name} has no closing brace")
        cursor = opening + 1
        depth = 1
        while cursor < closing:
            if masked[cursor] == "{":
                depth += 1
                cursor += 1
                continue
            if masked[cursor] == "}":
                depth -= 1
                cursor += 1
                continue
            if depth == 1:
                method = re.match(r"fn\s+([A-Za-z_][A-Za-z0-9_]*)\b", masked[cursor:])
                if method is not None:
                    name = method.group(1)
                    body_opening = masked.find("{", cursor + method.end())
                    if body_opening < 0 or body_opening >= closing:
                        raise ValueError(f"method {impl_name}::{name} has no body")
                    body_closing = matching_brace(masked, body_opening)
                    if body_closing is None or body_closing > closing:
                        raise ValueError(f"method {impl_name}::{name} has no closing brace")
                    line = source.count("\n", 0, cursor) + 1
                    methods.append((name, masked[body_opening + 1 : body_closing], line))
                    cursor = body_closing + 1
                    continue
            cursor += 1
    return methods


def function_body(source: str, name: str) -> str | None:
    masked = mask_rust_non_code(source)
    declaration = re.search(rf"\bfn\s+{re.escape(name)}\b", masked)
    if declaration is None:
        return None
    opening = masked.find("{", declaration.end())
    if opening < 0:
        return None
    closing = matching_brace(masked, opening)
    return None if closing is None else source[opening + 1 : closing]


def rust_type_body(source: str, kind: str, name: str) -> str | None:
    """Return one named Rust struct/enum body with comments and literals masked."""

    masked = mask_rust_non_code(source)
    declaration = re.search(
        rf"\b{re.escape(kind)}\s+{re.escape(name)}\b", masked
    )
    if declaration is None:
        return None
    opening = masked.find("{", declaration.end())
    if opening < 0:
        return None
    closing = matching_brace(masked, opening)
    return None if closing is None else masked[opening + 1 : closing]


def rust_enum_variants(source: str, name: str) -> list[str]:
    """Return top-level variant names from one ordinary Rust enum."""

    body = rust_type_body(source, "enum", name)
    if body is None:
        raise ValueError(f"required enum {name} is absent")
    fragments: list[str] = []
    start = 0
    depth = 0
    for index, character in enumerate(body):
        if character in "({[":
            depth += 1
        elif character in ")}]":
            depth -= 1
        elif character == "," and depth == 0:
            fragments.append(body[start:index])
            start = index + 1
    fragments.append(body[start:])
    variants: list[str] = []
    for fragment in fragments:
        match = re.search(r"\b([A-Z][A-Za-z0-9_]*)\b", fragment)
        if match is not None:
            variants.append(match.group(1))
    return variants


def rust_from_impl_body(source: str, source_name: str, target_name: str) -> str:
    """Return the masked body of one exact `From<S> for T` implementation."""

    masked = mask_rust_non_code(source)
    declarations = list(
        re.finditer(
            rf"\bimpl\s+From\s*<\s*{re.escape(source_name)}\s*>\s+for\s+"
            rf"{re.escape(target_name)}\s*\{{",
            masked,
        )
    )
    if len(declarations) != 1:
        raise ValueError(
            f"expected one From<{source_name}> for {target_name} impl, "
            f"found {len(declarations)}"
        )
    opening = masked.find("{", declarations[0].start())
    closing = matching_brace(masked, opening)
    if closing is None:
        raise ValueError(
            f"From<{source_name}> for {target_name} has no closing brace"
        )
    return masked[opening + 1 : closing]


def enum_bijection_errors(
    owner: str,
    production: list[str],
    model: list[str],
    *,
    ordered: bool = True,
) -> list[str]:
    """Require a duplicate-free total constructor bijection."""

    errors: list[str] = []
    for side, variants in (("production", production), ("model", model)):
        duplicates = sorted(
            variant for variant in set(variants) if variants.count(variant) != 1
        )
        if duplicates:
            errors.append(f"{owner} has duplicate {side} variants {duplicates}")
    missing = sorted(set(model) - set(production))
    extra = sorted(set(production) - set(model))
    if missing or extra:
        errors.append(
            f"{owner} is not a total constructor bijection: "
            f"missing production variants {missing}, extra production variants {extra}"
        )
    elif ordered and production != model:
        errors.append(
            f"{owner} constructor order differs: production {production}, model {model}"
        )
    return errors


def required_function_body(source: str, name: str) -> str:
    body = function_body(source, name)
    if body is None:
        raise ValueError(f"required function {name} has no body")
    return body


def impl_method_body(source: str, impl_name: str, method: str) -> str:
    matches = [body for name, body, _line in rust_impl_methods(source, impl_name) if name == method]
    if len(matches) != 1:
        raise ValueError(
            f"expected one {impl_name}::{method} method, found {len(matches)}"
        )
    return matches[0]


def require_ordered_fragments(
    body: str, owner: str, fragments: tuple[str, ...]
) -> list[str]:
    positions = [body.find(fragment) for fragment in fragments]
    if any(position < 0 for position in positions):
        missing = [
            fragment for fragment, position in zip(fragments, positions) if position < 0
        ]
        return [f"{owner} lost ordered topology fragment(s) {missing}"]
    if positions != sorted(positions):
        return [f"{owner} changed required topology order {fragments}"]
    return []


def validate_typed_adjacent_uniqueness(
    body: str,
    owner: str,
    ordering: str,
    collection: str,
    equality: str,
) -> list[str]:
    """Bind canonical uniqueness to one total, length-two representation."""

    compact = "".join(mask_rust_non_code(body).split())
    typed_relation = (
        f"{collection}.array_windows::<2>().any(|[left,right]|{equality})"
    )
    errors = require_ordered_fragments(
        compact,
        owner,
        (ordering, typed_relation),
    )
    if f"{collection}.windows(2)" in compact:
        errors.append(f"{owner} revived a partial adjacent-pair representation")
    return errors


def brace_depth(masked: str, offset: int) -> int:
    return masked[:offset].count("{") - masked[:offset].count("}")


def top_level_statement_end(masked: str, offset: int) -> int | None:
    """Find the semicolon that commits the top-level expression containing offset."""

    braces = 0
    parentheses = 0
    brackets = 0
    for cursor, character in enumerate(masked):
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
        elif (
            cursor >= offset
            and character == ";"
            and braces == 0
            and parentheses == 0
            and brackets == 0
        ):
            return cursor + 1
    return None


def validate_authority_mutation_publication() -> list[str]:
    """Prove every runtime mutation consumes one lock-external post-commit receipt."""

    try:
        source = TX_POOL_AUTHORITY_RUNTIME.read_text()
        masked = mask_rust_non_code(source)
        methods = rust_impl_methods(source, "AuthorityRuntime")
    except (OSError, ValueError) as error:
        return [f"cannot inspect AuthorityRuntime mutation publication: {error}"]

    errors: list[str] = []
    method_mutations = 0
    method_publications = 0
    for name, body, line in methods:
        mutations = list(AUTHORITY_MUTATION.finditer(body))
        publications = list(POST_COMMIT_PUBLICATION.finditer(body))
        method_mutations += len(mutations)
        method_publications += len(publications)
        if name == "publish_committed":
            if mutations or len(publications) != 1 or "into_post_commit" not in body:
                errors.append(
                    "AuthorityRuntime::publish_committed must only convert and publish "
                    f"one post-commit receipt near runtime.rs:{line}"
                )
            continue
        if name == "settle_effect":
            closed_settlement = re.search(
                r"match\s+commit\s*\{\s*"
                r"EffectSettlementCommit::Applied\(retirement\)\s*=>\s*"
                r"self\.publish_committed\(retirement\),\s*"
                r"EffectSettlementCommit::Superseded\(settlement\)\s*=>\s*"
                r"drop\(settlement\),\s*\}",
                body,
            )
            if len(mutations) != 1 or len(publications) != 1 or closed_settlement is None:
                errors.append(
                    "AuthorityRuntime::settle_effect must keep the closed Applied -> "
                    "post-commit publication / Superseded -> mutation-free retirement algebra "
                    f"near runtime.rs:{line}"
                )
            elif publications[0].start() <= mutations[0].end():
                errors.append(
                    "AuthorityRuntime::settle_effect publishes before its possible Apply "
                    f"near runtime.rs:{line}"
                )
            continue
        if name == "exchange_compute":
            optional_exchange = re.search(
                r"if\s+let\s+Some\(retirement\)\s*=\s*retirement\s*\{\s*"
                r"self\.publish_committed\(retirement\);\s*\}",
                body,
            )
            if len(mutations) != 1 or len(publications) != 1 or optional_exchange is None:
                errors.append(
                    "AuthorityRuntime::exchange_compute must keep the closed "
                    "Applied -> post-commit publication / Unchanged -> mutation-free "
                    f"algebra near runtime.rs:{line}"
                )
            elif publications[0].start() <= mutations[0].end():
                errors.append(
                    "AuthorityRuntime::exchange_compute publishes before its possible "
                    f"Apply near runtime.rs:{line}"
                )
            continue
        if not mutations:
            if publications:
                errors.append(
                    f"AuthorityRuntime::{name} publishes a post-commit receipt without Apply "
                    f"near runtime.rs:{line}"
                )
            continue
        if len(publications) != 1:
            errors.append(
                f"AuthorityRuntime::{name} must publish exactly once after mutation, "
                f"found {len(publications)} near runtime.rs:{line}"
            )
            continue
        publication = publications[0]
        if brace_depth(body, publication.start()) != 0:
            errors.append(
                f"AuthorityRuntime::{name} post-commit publication must be a top-level "
                f"post-guard operation near runtime.rs:{line}"
            )
        last_mutation = mutations[-1]
        if publication.start() <= last_mutation.end():
            errors.append(
                f"AuthorityRuntime::{name} publishes before its final mutation near "
                f"runtime.rs:{line}"
            )
            continue
        commit_end = top_level_statement_end(body, last_mutation.start())
        if commit_end is None or commit_end > publication.start():
            errors.append(
                f"AuthorityRuntime::{name} mutation must finish one top-level committed "
                f"statement before publication near runtime.rs:{line}"
            )
            continue
        escaping = EARLY_EXIT.search(body[commit_end : publication.start()])
        if escaping is not None:
            escape = "return early via ?" if escaping.group(0) == "?" else escaping.group(0)
            errors.append(
                f"AuthorityRuntime::{name} can {escape} between mutation and "
                f"post-commit publication near runtime.rs:{line}"
            )

    all_mutations = len(AUTHORITY_MUTATION.findall(masked))
    all_publications = len(POST_COMMIT_PUBLICATION.findall(masked))
    if method_mutations != all_mutations:
        errors.append(
            "authority mutation must remain directly inside an AuthorityRuntime method: "
            f"found {all_mutations - method_mutations} outside the impl"
        )
    if method_publications != all_publications:
        errors.append(
            "post-commit publication must remain directly inside an AuthorityRuntime method: "
            f"found {all_publications - method_publications} outside the impl"
        )
    return errors


def validate_authority_profiling_seams() -> list[str]:
    """Keep profiling complete, centralized and absent from default behavior."""

    try:
        runtime = TX_POOL_AUTHORITY_RUNTIME.read_text()
        publisher = TX_POOL_AUTHORITY_PUBLISHER.read_text()
        benchmark = TX_POOL_BENCHMARK.read_text()
    except OSError as error:
        return [f"cannot inspect authority profiling seams: {error}"]

    errors: list[str] = []
    if runtime.count("struct AuthorityStoreLock") != 1:
        errors.append("the authority lock must have one centralized profiling wrapper")
    if runtime.count("Arc<AuthorityStoreLock>") != 2:
        errors.append(
            "AuthorityRuntime and AuthorityRelayParentReader must share the profiled lock type"
        )
    if "Arc<RwLock<AuthorityStore>>" in runtime:
        errors.append("authority callers must not bypass AuthorityStoreLock")
    for required in (
        '#[cfg(not(feature = "profiling"))]\ntype AuthorityStoreGuard<G> = G;',
        "inner: RwLock<AuthorityStore>",
        "AuthorityStoreLock::upgrade(store)",
    ):
        if required not in runtime:
            errors.append(f"authority profiling boundary lost {required!r}")

    expected_lock_spans = {
        "tx_pool.authority.read_wait": 1,
        "tx_pool.authority.read_hold": 1,
        "tx_pool.authority.write_wait": 1,
        # Direct writes and upgradable-read promotion share one write-hold
        # coordinate; the acquisition coordinates remain distinct.
        "tx_pool.authority.write_hold": 2,
        "tx_pool.authority.upgradable_read_wait": 1,
        "tx_pool.authority.upgradable_read_hold": 1,
        "tx_pool.authority.upgrade_wait": 1,
    }
    for span, expected in expected_lock_spans.items():
        actual = runtime.count(f'"{span}"')
        if actual != expected:
            errors.append(
                f"authority profiling span {span} must have {expected} centralized "
                f"producer(s), found {actual}"
            )

    def validate_instrumented_function(
        source: str, function: str, span: str, owner: str
    ) -> None:
        declaration = re.search(rf"\b(?:async\s+)?fn\s+{re.escape(function)}\b", source)
        if declaration is None:
            errors.append(f"profiling owner {owner}::{function} disappeared")
            return
        attributes = source[max(0, declaration.start() - 600) : declaration.start()]
        if (
            f'name = "{span}"' not in attributes
            or 'target = "ckb_tx_pool_profile"' not in attributes
            or 'feature = "profiling"' not in attributes
        ):
            errors.append(
                f"profiling owner {owner}::{function} must produce feature-gated span {span}"
            )

    for function, span in (
        ("execute_resolution", "tx_pool.stage.resolve"),
        ("execute_verification", "tx_pool.stage.verify"),
        ("try_drive_ready", "tx_pool.stage.ready_attempt"),
    ):
        validate_instrumented_function(runtime, function, span, "AuthorityRuntime")
    ready_body = function_body(runtime, "try_drive_ready")
    if ready_body is None:
        errors.append("AuthorityRuntime::try_drive_ready disappeared")
    else:
        capture = ready_body.find("let Some(work)")
        active_span = ready_body.find('"tx_pool.stage.ready_work"')
        preparation = ready_body.find("let prepared")
        if min(capture, active_span, preparation) < 0 or not capture < active_span < preparation:
            errors.append(
                "Ready profiling must distinguish every driver attempt from non-idle work"
            )
    validate_instrumented_function(
        publisher,
        "publish_committed_effect_batch",
        "tx_pool.effects.publish",
        "authority publisher",
    )
    if publisher.count('"tx_pool.effects.publish"') != 1:
        errors.append(
            "effect profiling must cover one committed batch, not the permanent publisher task"
        )

    expected_counter_spans = sorted(
        {
            *expected_lock_spans,
            "tx_pool.stage.resolve",
            "tx_pool.stage.verify",
            "tx_pool.stage.ready_attempt",
            "tx_pool.stage.ready_work",
            "tx_pool.effects.publish",
        }
    )
    counter_registry = re.search(
        r"const\s+PROFILE_SPAN_NAMES\s*:\s*\[&str;\s*\d+\]\s*=\s*"
        r"\[(?P<body>.*?)\];",
        benchmark,
        re.S,
    )
    if counter_registry is None:
        errors.append("profiling benchmark lost the derived span counter registry")
    else:
        counter_spans = re.findall(r'"(tx_pool\.[^"]+)"', counter_registry.group("body"))
        if counter_spans != expected_counter_spans:
            errors.append(
                "profiling counter registry differs from the semantic producers: "
                f"expected {expected_counter_spans}, found {counter_spans}"
            )
    for required in (
        "fn on_new_span(",
        '"span_starts_during_target_work"',
        "serde_json::to_writer(&mut self.output, &record)",
    ):
        if required not in benchmark:
            errors.append(f"profiling counter boundary lost {required!r}")
    if "tracing_subscriber::fmt::layer()" in benchmark:
        errors.append(
            "benchmark profiling must not format or write one record per authority span"
        )
    return errors


def validate_authority_failure_algebra() -> list[str]:
    """Bind the executable fault relation to sealed production routes."""

    try:
        service = TX_POOL_AUTHORITY_SERVICE.read_text()
        plan = TX_POOL_AUTHORITY_PLAN.read_text()
        ingress = TX_POOL_AUTHORITY_INGRESS.read_text()
        state = TX_POOL_AUTHORITY_STATE.read_text()
        runtime = TX_POOL_AUTHORITY_RUNTIME.read_text()
        chain_transition = TX_POOL_AUTHORITY_CHAIN_TRANSITION.read_text()
        model = TX_POOL_MODEL_BOUNDARIES.read_text()
        masked_service = mask_rust_non_code(service)
        masked_ingress = mask_rust_non_code(ingress)
        masked_state = mask_rust_non_code(state)
        masked_model = mask_rust_non_code(model)

        service_variants = rust_enum_variants(service, "AuthorityServiceError")
        integrity_variants = rust_enum_variants(service, "AuthorityIntegrityFault")
        authority_faults = rust_enum_variants(plan, "AuthorityFault")
        retained_outcomes = rust_enum_variants(ingress, "RetainedIngressAttempt")
        recovery_failures = rust_enum_variants(state, "RecoveryAdmissionError")
        model_service_variants = rust_enum_variants(model, "ModelServiceFailure")
        model_operational = rust_enum_variants(model, "ModelOperationalFailure")
        model_structural = rust_enum_variants(model, "ModelStructuralFault")
        model_retained = rust_enum_variants(model, "ModelRetainedIngressOutcome")
        model_recovery = rust_enum_variants(model, "ModelRecoveryAdmissionFailure")
    except (OSError, ValueError) as error:
        return [f"cannot inspect authority failure algebra: {error}"]

    errors: list[str] = []

    if service_variants[-1:] != ["Integrity"]:
        errors.append(
            "AuthorityServiceError must place its sole typed Integrity boundary after "
            f"all ordinary variants, found {service_variants}"
        )
    errors.extend(
        enum_bijection_errors(
            "AuthorityServiceError ordinary failure algebra",
            [variant for variant in service_variants if variant != "Integrity"],
            model_operational,
        )
    )
    if model_service_variants != ["Operational", "Integrity"]:
        errors.append(
            "ModelServiceFailure must remain the closed Operational/Integrity sum, "
            f"found {model_service_variants}"
        )

    service_error_body = rust_type_body(service, "enum", "AuthorityServiceError")
    if service_error_body is None or not re.search(
        r"\bIntegrity\s*\(\s*AuthorityGenerationInvalidity\s*\)", service_error_body
    ):
        errors.append(
            "AuthorityServiceError must carry the sealed move-only "
            "Integrity(AuthorityGenerationInvalidity) boundary"
        )

    chain_error_body = rust_type_body(service, "enum", "AuthorityChainUpdateError")
    if chain_error_body is None or not re.search(
        r"\bIntegrity\s*\(\s*AuthorityGenerationInvalidity\s*\)",
        chain_error_body,
    ):
        errors.append(
            "AuthorityChainUpdateError must carry the same sealed generation-invalidity "
            "capability"
        )

    if not re.search(
        r"pub\s*\(\s*in\s+crate::authority\s*\)\s+enum\s+AuthorityIntegrityFault\b",
        masked_service,
    ):
        errors.append(
            "AuthorityIntegrityFault visibility must remain confined to crate::authority"
        )
    if not re.search(
        r"pub\s*\(\s*in\s+crate::authority\s*\)\s+enum\s+AuthorityFault\b",
        mask_rust_non_code(plan),
    ):
        errors.append("AuthorityFault visibility must remain confined to crate::authority")

    expected_integrity_variants = [
        "InvalidChainEvidence",
        "EffectLifecycleClosed",
        "Authority",
    ]
    if integrity_variants != expected_integrity_variants:
        errors.append(
            "AuthorityIntegrityFault must contain only the two chain-only premises and "
            "one AuthorityFault carrier, found "
            f"{integrity_variants}"
        )
    integrity_body = rust_type_body(service, "enum", "AuthorityIntegrityFault")
    if integrity_body is None or not re.search(
        r"\bAuthority\s*\(\s*AuthorityFault\s*\)", integrity_body
    ):
        errors.append(
            "AuthorityIntegrityFault::Authority must carry AuthorityFault without a "
            "duplicate projection enum"
        )

    flattened_production_faults = [
        "InvalidChainEvidence",
        "EffectLifecycleClosed",
        *authority_faults,
    ]
    errors.extend(
        enum_bijection_errors(
            "flattened structural fault algebra",
            flattened_production_faults,
            model_structural,
            ordered=False,
        )
    )
    errors.extend(
        enum_bijection_errors(
            "retained-ingress terminal algebra",
            retained_outcomes,
            model_retained,
        )
    )
    errors.extend(
        enum_bijection_errors(
            "recovery-admission failure algebra",
            recovery_failures,
            model_recovery,
        )
    )

    model_evidence = rust_type_body(model, "struct", "ModelStructuralEvidence")
    if model_evidence is None or "".join(model_evidence.split()) != (
        "fault:ModelStructuralFault,"
    ):
        errors.append(
            "ModelStructuralEvidence must seal exactly one ModelStructuralFault"
        )
    for model_capability in ("ModelStructuralEvidence", "ModelServiceFailure"):
        declaration = re.search(
            r"(?P<attributes>(?:#\s*\[[^\]]*\]\s*)*)"
            rf"pub\s*\(super\)\s+(?:struct|enum)\s+{model_capability}\b",
            masked_model,
        )
        attributes = "" if declaration is None else declaration.group("attributes")
        derives = re.findall(
            r"#\s*\[\s*derive\s*\((?P<body>[^)]*)\)\s*\]", attributes
        )
        if declaration is None or any(
            re.search(r"\b(?:Clone|Copy)\b", derive) for derive in derives
        ):
            errors.append(
                f"{model_capability} must remain a move-only sealed model capability"
            )
    model_service_body = rust_type_body(model, "enum", "ModelServiceFailure")
    if model_service_body is None:
        errors.append("ModelServiceFailure declaration disappeared")
    else:
        compact_model_service = "".join(model_service_body.split())
        for required in (
            "Operational(ModelOperationalFailure)",
            "Integrity(ModelStructuralEvidence)",
        ):
            if compact_model_service.count(required) != 1:
                errors.append(
                    f"ModelServiceFailure lost its exact sealed route {required}"
                )

    expected_from_implementations = (
        (
            "AuthorityFault",
            "AuthorityIntegrityFault",
            "fnfrom(fault:AuthorityFault)->Self{Self::Authority(fault)}",
        ),
        (
            "AuthorityIntegrityFault",
            "AuthorityServiceError",
            "fnfrom(fault:AuthorityIntegrityFault)->Self{Self::Integrity(AuthorityGenerationInvalidity::from_integrity(fault))}",
        ),
        (
            "AuthorityFault",
            "AuthorityServiceError",
            "fnfrom(fault:AuthorityFault)->Self{Self::from(AuthorityIntegrityFault::from(fault))}",
        ),
    )
    for source_name, target_name, expected in expected_from_implementations:
        try:
            implementation = rust_from_impl_body(service, source_name, target_name)
        except ValueError as error:
            errors.append(str(error))
            continue
        compact = "".join(implementation.split())
        if compact != expected:
            errors.append(
                f"From<{source_name}> for {target_name} must remain the unique exact "
                f"structural route, found {compact!r}"
            )

    negative_canary = enum_bijection_errors(
        "fault-route negative canary", ["Ordinary"], ["Ordinary", "Dangling"]
    )
    if not negative_canary:
        errors.append(
            "fault-route constructor-bijection gate failed its missing-route negative canary"
        )

    invalidity = re.search(
        r"\bstruct\s+AuthorityGenerationInvalidity\s*\(AuthorityIntegrityFault\)\s*;",
        masked_service,
    )
    if invalidity is None:
        errors.append(
            "AuthorityGenerationInvalidity must own AuthorityIntegrityFault directly"
        )
        constructor_source = masked_service
    else:
        constructor_source = (
            masked_service[: invalidity.start()]
            + " " * (invalidity.end() - invalidity.start())
            + masked_service[invalidity.end() :]
        )
        capability_declaration = re.search(
            r"(?P<attributes>(?:#\s*\[[^\]]*\]\s*)*)"
            r"pub\s*\(crate\)\s+struct\s+AuthorityGenerationInvalidity\s*"
            r"\(AuthorityIntegrityFault\)\s*;",
            masked_service,
        )
        attributes = (
            "" if capability_declaration is None else capability_declaration.group("attributes")
        )
        if "must_use" not in attributes:
            errors.append("AuthorityGenerationInvalidity must remain must_use")
        derive = re.findall(
            r"#\s*\[\s*derive\s*\((?P<body>[^)]*)\)\s*\]", attributes
        )
        if any(re.search(r"\b(?:Clone|Copy)\b", body) for body in derive):
            errors.append(
                "AuthorityGenerationInvalidity is a move-only capability and must not "
                "derive Clone or Copy"
            )
    constructors = re.findall(r"\bAuthorityGenerationInvalidity\s*\(", constructor_source)
    if constructors:
        errors.append(
            "AuthorityGenerationInvalidity's private field must not be constructed by name "
            f"outside its declaration, found {len(constructors)} sites"
        )
    try:
        invalidity_constructor = "".join(
            impl_method_body(
                service, "AuthorityGenerationInvalidity", "from_integrity"
            ).split()
        )
    except ValueError as error:
        errors.append(str(error))
    else:
        if invalidity_constructor != "Self(fault)":
            errors.append(
                "AuthorityGenerationInvalidity::from_integrity must be the sole exact "
                f"private constructor, found {invalidity_constructor!r}"
            )
    try:
        chain_integrity_constructor = "".join(
            impl_method_body(
                service, "AuthorityChainUpdateError", "integrity"
            ).split()
        )
    except ValueError as error:
        errors.append(str(error))
    else:
        expected = (
            "Self::Integrity(AuthorityGenerationInvalidity::from_integrity(fault))"
        )
        if chain_integrity_constructor != expected:
            errors.append(
                "AuthorityChainUpdateError::integrity must consume the same private "
                f"invalidity constructor, found {chain_integrity_constructor!r}"
            )

    settlement = function_body(service, "settle_operation_error")
    if settlement is None:
        errors.append("AuthorityService::settle_operation_error disappeared")
    else:
        settlement = mask_rust_non_code(settlement)
        if settlement.count("AuthorityServiceError::Integrity(invalidity)") != 1:
            errors.append(
                "settle_operation_error must classify the typed Integrity variant exactly once"
            )
        if settlement.count("Err(invalidity)") != 1:
            errors.append(
                "settle_operation_error must consume and return the sealed invalidity "
                "capability exactly once"
            )

    production_sources: dict[Path, str] = {}
    try:
        for path in sorted(TX_POOL_SRC.rglob("*.rs")):
            if "tests" in path.parts:
                continue
            production_sources[path] = mask_rust_non_code(path.read_text())
    except (OSError, ValueError) as error:
        errors.append(f"cannot scan production fault routes: {error}")
        return errors

    integrity_sites: list[Path] = []
    service_from_sites: list[Path] = []
    integrity_constructor_sites: list[Path] = []
    authority_fault_outside_owner: list[Path] = []
    retired_failure_types = (
        "AuthorityProjectionFault",
        "RetainedIngressError",
        "AdmissionValidationError",
        "AdmissionValidationFailure",
    )
    retired_sites: list[tuple[Path, str]] = []
    authority_root = TX_POOL_AUTHORITY_SERVICE.parent
    for path, source in production_sources.items():
        integrity_sites.extend(
            [path] * len(re.findall(r"\bAuthorityServiceError::Integrity\s*\(", source))
        )
        service_from_sites.extend(
            [path] * len(re.findall(r"\bAuthorityServiceError::from\s*\(", source))
        )
        integrity_constructor_sites.extend(
            [path] * len(re.findall(r"\bAuthorityIntegrityFault::", source))
        )
        if authority_root not in path.parents and "AuthorityFault::" in source:
            authority_fault_outside_owner.append(path)
        for retired in retired_failure_types:
            if re.search(rf"\b{re.escape(retired)}\b", source):
                retired_sites.append((path, retired))

    if len(integrity_sites) != 1 or integrity_sites[0] != TX_POOL_AUTHORITY_SERVICE:
        errors.append(
            "AuthorityServiceError::Integrity must occur exactly once at service "
            "settlement, found "
            f"{[str(path.relative_to(REPO_ROOT)) for path in integrity_sites]}"
        )
    escaped_service_adapters = sorted(
        {path.relative_to(REPO_ROOT) for path in service_from_sites if path != TX_POOL_AUTHORITY_SERVICE}
    )
    if escaped_service_adapters:
        errors.append(
            "AuthorityServiceError::from escaped the sole service boundary into "
            f"{escaped_service_adapters}"
        )
    escaped_integrity_constructors = sorted(
        {
            path.relative_to(REPO_ROOT)
            for path in integrity_constructor_sites
            if path != TX_POOL_AUTHORITY_SERVICE
        }
    )
    if escaped_integrity_constructors:
        errors.append(
            "AuthorityIntegrityFault constructors escaped the sole service boundary into "
            f"{escaped_integrity_constructors}"
        )
    if authority_fault_outside_owner:
        errors.append(
            "AuthorityFault constructors escaped authority ownership into "
            f"{[str(path.relative_to(REPO_ROOT)) for path in authority_fault_outside_owner]}"
        )
    for path, retired in retired_sites:
        errors.append(
            f"retired duplicate failure type {retired} remains in "
            f"{path.relative_to(REPO_ROOT)}"
        )

    bounded_body = rust_type_body(ingress, "struct", "BoundedTransaction")
    if bounded_body is None or "".join(bounded_body.split()) != (
        "transaction:Arc<TransactionView>,payload_bytes:usize,encoded_edges:usize,"
    ):
        errors.append(
            "BoundedTransaction must seal exactly transaction, payload bytes and encoded "
            "edge count"
        )
    try:
        bounded_new = "".join(
            impl_method_body(ingress, "BoundedTransaction", "try_new").split()
        )
    except ValueError as error:
        errors.append(str(error))
    else:
        errors.extend(
            require_ordered_fragments(
                bounded_new,
                "BoundedTransaction::try_new",
                (
                    "u64::try_from(serialized_bytes)",
                    "ifserialized_bytes_u64>TRANSACTION_SIZE_LIMIT",
                    "letpayload_bytes=transaction.data().total_size()",
                    "letencoded_edges=transaction.inputs().len()",
                    ".checked_add(transaction.cell_deps().len())",
                    ".and_then(|count|count.checked_add(transaction.header_deps().len()))",
                    "transaction.try_into_compact()",
                    "Ok(Self{transaction:Arc::new(transaction),payload_bytes,encoded_edges,})",
                ),
            )
        )

    allocation_body = rust_type_body(state, "struct", "RetainedAdmissionAllocation")
    if allocation_body is None or "".join(allocation_body.split()) != (
        "transaction:Arc<TransactionView>,"
    ):
        errors.append(
            "RetainedAdmissionAllocation must return exactly the retained transaction"
        )

    try:
        dependency_construction = "".join(
            impl_method_body(
                state, "KnownDependencies", "from_bounded_transaction"
            ).split()
        )
        admission_construction = "".join(
            impl_method_body(state, "ValidatedAdmission", "new").split()
        )
        recovery_construction = "".join(
            impl_method_body(state, "ValidatedAdmission", "recovery").split()
        )
    except ValueError as error:
        errors.append(str(error))
    else:
        errors.extend(
            require_ordered_fragments(
                dependency_construction,
                "KnownDependencies::from_bounded_transaction",
                (
                    "keys.try_reserve_exact(encoded_edges)?",
                    "tx.input_pts_iter()",
                    "tx.cell_deps()",
                    "tx.header_deps()",
                    "keys.sort_unstable()",
                    "keys.dedup()",
                    "Ok(Self(Arc::new(keys)))",
                ),
            )
        )
        for forbidden in ("checked_add", "DependencySetError", ".try_reserve("):
            if forbidden in dependency_construction:
                errors.append(
                    "KnownDependencies::from_bounded_transaction must consume sealed edge "
                    f"evidence without {forbidden}"
                )
        errors.extend(
            require_ordered_fragments(
                admission_construction,
                "ValidatedAdmission::new",
                (
                    "tx.into_admission_parts()",
                    "KnownDependencies::from_bounded_transaction(&tx,encoded_edges)",
                    "Err(_)=>{returnErr(RetainedAdmissionAllocation{transaction:tx});}",
                    "Ok(Self{identity:TxIdentity::from_transaction(&tx),tx,source,dependencies,payload_bytes,encoded_edges,})",
                ),
            )
        )
        for forbidden in (".total_size(", "checked_add", "DependencySetError"):
            if forbidden in admission_construction:
                errors.append(
                    "ValidatedAdmission::new must not recompute sealed ingress evidence "
                    f"through {forbidden}"
                )
        for required in (
            "BoundedTransactionError::Allocation=>{RecoveryAdmissionError::ResourceUnavailable}",
            "BoundedTransactionError::TooLarge{..}=>{RecoveryAdmissionError::InvalidTransaction}",
            ".map_err(|_|RecoveryAdmissionError::ResourceUnavailable)",
        ):
            if required not in recovery_construction:
                errors.append(
                    f"ValidatedAdmission::recovery lost exact failure route {required}"
                )

    for function_name in ("remote", "remote_at", "proposal"):
        signature = re.search(
            rf"\bfn\s+{function_name}\b(?P<body>[^{{;]*)\{{", masked_ingress, re.S
        )
        body = function_body(ingress, function_name)
        if signature is None or body is None:
            errors.append(f"retained-ingress producer {function_name} disappeared")
            continue
        compact_signature = "".join(signature.group("body").split())
        if "->RetainedIngressAttempt" not in compact_signature or "Result<" in compact_signature:
            errors.append(
                f"retained-ingress producer {function_name} must return the total "
                "RetainedIngressAttempt algebra"
            )
        compact_body = "".join(mask_rust_non_code(body).split())
        for forbidden in (
            "AuthorityFault",
            "AuthorityIntegrityFault",
            "AuthorityServiceError",
            "Integrity(",
        ):
            if forbidden in compact_body:
                errors.append(
                    f"retained-ingress producer {function_name} can reach structural "
                    f"failure through {forbidden}"
                )

    for function_name, route in (
        (
            "submit_remote_batch",
            "attempts.push_back(remote(tx,declared_cycles,peer,&consensus));",
        ),
        (
            "submit_proposal_batch",
            "attempts.push_back(proposal(tx,&consensus));",
        ),
    ):
        body = function_body(service, function_name)
        if body is None:
            errors.append(f"AuthorityService::{function_name} disappeared")
            continue
        compact = "".join(mask_rust_non_code(body).split())
        if compact.count(route) != 1:
            errors.append(
                f"AuthorityService::{function_name} must enqueue the total ingress "
                f"outcome directly through {route}"
            )

    runtime_recovery = function_body(runtime, "apply_chain_update")
    plan_recovery = function_body(
        chain_transition, "plan_chain_generation_replacement"
    )
    if runtime_recovery is None:
        errors.append("AuthorityStore::apply_chain_update disappeared")
    else:
        compact = "".join(mask_rust_non_code(runtime_recovery).split())
        for required in (
            "RecoveryAdmissionError::ResourceUnavailable,)=>ChainBoundaryError::Allocation",
            "RecoveryAdmissionError::InvalidTransaction,)=>ChainBoundaryError::InvalidFacts",
        ):
            if required not in compact:
                errors.append(
                    "chain-evidence recovery lost its exact ordinary/invalid-evidence "
                    f"route {required}"
                )
    if plan_recovery is None:
        errors.append("Authority::plan_chain_generation_replacement disappeared")
    else:
        compact = "".join(mask_rust_non_code(plan_recovery).split())
        for required in (
            "RecoveryAdmissionError::ResourceUnavailable=>{PlanError::Backpressure(Backpressure::Allocation)}",
            "RecoveryAdmissionError::InvalidTransaction=>{PlanError::Fault(AuthorityFault::ResourceProjection)}",
        ):
            if required not in compact:
                errors.append(
                    "authority-rebuild recovery lost its exact ordinary/structural "
                    f"route {required}"
                )
    return errors


def validate_transaction_query_failure_domains() -> list[str]:
    """Keep status lookup independent of fallible optional detail derivation."""

    try:
        query = TX_POOL_AUTHORITY_QUERY.read_text()
        dispatch = TX_POOL_DISPATCH.read_text()
    except OSError as error:
        return [f"cannot inspect transaction query failure domains: {error}"]

    errors: list[str] = []
    for function in ("transaction_status_lookup", "transaction_status"):
        status_function = function_body(query, function)
        if status_function is None:
            errors.append(f"authority {function} disappeared")
            continue
        masked = mask_rust_non_code(status_function)
        if "transaction_lookup(" in masked or "minimum_replacement_fee(" in masked:
            errors.append(
                f"authority {function} must not evaluate optional detail arithmetic"
            )

    status_handler = function_body(dispatch, "handle_get_tx_status")
    if status_handler is None:
        errors.append("get_tx_status dispatch handler disappeared")
    else:
        masked = mask_rust_non_code(status_handler)
        if "transaction_status_lookup(&hash)" not in masked:
            errors.append("get_tx_status must consume the status-only authority product")
        if "transaction_lookup(&hash)" in masked:
            errors.append("get_tx_status must not consume the detailed authority product")

    detail_handler = function_body(dispatch, "handle_get_transaction_with_status")
    if detail_handler is None:
        errors.append("get_transaction_with_status dispatch handler disappeared")
    else:
        masked = mask_rust_non_code(detail_handler)
        if "transaction_lookup(&hash)" not in masked:
            errors.append(
                "get_transaction_with_status must consume the detailed authority product"
            )
    return errors


def validate_prepared_full_query() -> list[str]:
    """Bind the complete public full-scan class to one bounded prepared protocol."""

    try:
        contract = json.loads(TX_POOL_ARCHITECTURE_CONTRACT.read_text())
        runtime = TX_POOL_AUTHORITY_RUNTIME.read_text()
        query = TX_POOL_AUTHORITY_QUERY.read_text()
        read = TX_POOL_AUTHORITY_READ.read_text()
        resources = TX_POOL_AUTHORITY_RESOURCES.read_text()
        runtime_methods = {
            name: body for name, body, _line in rust_impl_methods(runtime, "AuthorityRuntime")
        }
        permit_methods = {
            name: body for name, body, _line in rust_impl_methods(query, "FullQueryPermit")
        }
    except (OSError, ValueError, json.JSONDecodeError) as error:
        return [f"cannot inspect prepared full-query protocol: {error}"]

    components = contract.get("selected_topology", {}).get("components", [])
    matches = [
        component
        for component in components
        if isinstance(component, dict) and component.get("id") == "prepared_full_query"
    ]
    if len(matches) != 1:
        return [
            "architecture contract must own exactly one prepared_full_query component"
        ]
    component = matches[0]
    errors: list[str] = []

    def declared_methods(field: str) -> list[str]:
        value = component.get(field)
        if (
            not isinstance(value, list)
            or not value
            or any(not isinstance(item, str) or not item for item in value)
            or len(value) != len(set(value))
        ):
            errors.append(f"prepared_full_query has an invalid {field} registry")
            return []
        return value

    full_scan = declared_methods("full_scan_runtime_methods")
    row_materializing = declared_methods("row_materializing_runtime_methods")
    fixed_output = declared_methods("fixed_output_runtime_methods")
    concurrent = declared_methods("concurrent_runtime_methods")
    captures = declared_methods("capture_methods")
    runtime_captures = component.get("runtime_capture_methods")
    if (
        not isinstance(runtime_captures, dict)
        or set(runtime_captures) != set(full_scan)
        or any(
            not isinstance(capture, str) or not capture
            for capture in runtime_captures.values()
        )
    ):
        errors.append(
            "prepared_full_query runtime_capture_methods must total the full-scan class"
        )
        runtime_captures = {}
    elif set(runtime_captures.values()) != set(captures):
        errors.append(
            "prepared_full_query capture registry must equal its runtime capture range"
        )

    if set(row_materializing).intersection(fixed_output):
        errors.append("prepared full-query row and fixed-output classes must be disjoint")
    if set(row_materializing).union(fixed_output) != set(full_scan):
        errors.append("prepared full-query row and fixed-output classes must total Q")
    if set(full_scan).intersection(concurrent):
        errors.append("full-scan and explicitly concurrent query classes must be disjoint")

    masked_query = mask_rust_non_code(query)
    shared_gate = re.search(
        r"#\s*\[\s*derive\s*\([^]]*\bClone\b[^]]*\)\s*\]\s*"
        r"pub\s*\(super\)\s+struct\s+AuthorityQueryScratch\s*\{\s*"
        r"state\s*:\s*Arc\s*<\s*Mutex\s*<\s*AuthorityQueryScratchState\s*>\s*>",
        masked_query,
    )
    if shared_gate is None:
        errors.append(
            "every cloneable AuthorityRuntime handle must share one Arc-owned full-query gate"
        )
    acquire = impl_method_body(query, "AuthorityQueryScratch", "acquire")
    if "Arc::clone(&self.state).lock_owned().await" not in acquire:
        errors.append("full-query acquisition must consume the shared gate through an owned guard")
    if runtime.count("full_query: AuthorityQueryScratch") != 1:
        errors.append("AuthorityRuntime must own exactly one full-query gate")
    if runtime.count("AuthorityQueryScratch::new(runtime.full_query_max_rows)") != 1:
        errors.append("AuthorityRuntime must construct exactly one full-query gate")

    actual_gated = {
        name
        for name, body in runtime_methods.items()
        if "self.full_query.acquire().await" in body
    }
    if actual_gated != set(full_scan):
        errors.append(
            "the architecture-owned full-scan class must exactly equal the runtime gate "
            f"users: declared={sorted(full_scan)}, observed={sorted(actual_gated)}"
        )

    for method in full_scan:
        body = runtime_methods.get(method)
        if body is None:
            errors.append(f"AuthorityRuntime::{method} disappeared")
            continue
        if body.count("self.full_query.acquire().await") != 1:
            errors.append(f"AuthorityRuntime::{method} must acquire the sole query gate once")
        if body.count(".await") != 1:
            errors.append(
                f"AuthorityRuntime::{method} must not await after acquiring full-query ownership"
            )
        capture = runtime_captures.get(method)
        if capture is not None and re.search(
            rf"\bpermit\s*\.\s*{re.escape(capture)}\s*\(", body
        ) is None:
            errors.append(
                f"AuthorityRuntime::{method} lost its declared {capture} coherent capture"
            )

    for method in concurrent:
        body = runtime_methods.get(method)
        if body is None:
            errors.append(f"AuthorityRuntime::{method} disappeared")
        elif "full_query" in body:
            errors.append(
                f"AuthorityRuntime::{method} must remain independent of the full-query gate"
            )

    for method in row_materializing:
        body = runtime_methods.get(method)
        if body is None:
            continue
        first_drop = body.find("drop(store)")
        growth = body.find("permit.grow(observed_rows)?")
        capture = runtime_captures.get(method)
        capture_match = (
            None
            if capture is None
            else re.search(rf"\bpermit\s*\.\s*{re.escape(capture)}\s*\(", body)
        )
        capture_position = -1 if capture_match is None else capture_match.start()
        final_drop = body.rfind("drop(store)")
        finish = body.find(".finish()")
        if min(first_drop, growth, capture_position, final_drop, finish) < 0:
            errors.append(
                f"AuthorityRuntime::{method} lost prepared growth/capture/finish topology"
            )
        elif not first_drop < growth < capture_position < final_drop < finish:
            errors.append(
                f"AuthorityRuntime::{method} must grow and finish outside the authority guard"
            )
        for fragment in (
            "let observed_rows = view.owner_count()",
            "!permit.is_prepared(observed_rows)?",
            "continue;",
        ):
            if fragment not in body:
                errors.append(
                    f"AuthorityRuntime::{method} lost finite-rank fragment {fragment!r}"
                )

    for method in fixed_output:
        body = runtime_methods.get(method)
        if body is None:
            continue
        for forbidden in ("permit.grow(", "permit.is_prepared(", "owner_count()"):
            if forbidden in body:
                errors.append(
                    f"AuthorityRuntime::{method} must not grow row scratch for fixed output"
                )
        if method == "pool_detail":
            released = body.rfind("drop(store)")
            finish = body.find(".finish()")
            if released < 0 or finish < 0 or released >= finish:
                errors.append(
                    "AuthorityRuntime::pool_detail must build its response after the guard opens"
                )

    forbidden_capture_work = (
        ".await",
        "try_reserve",
        ".reserve(",
        ".sort(",
        ".sort_",
        "String::",
        "HashMap::",
    )
    for method in captures:
        body = permit_methods.get(method)
        if body is None:
            errors.append(f"FullQueryPermit::{method} disappeared")
            continue
        for forbidden in forbidden_capture_work:
            if forbidden in body:
                errors.append(
                    f"FullQueryPermit::{method} performs forbidden guard-held work "
                    f"{forbidden!r}"
                )

    grow = permit_methods.get("grow")
    if grow is None:
        errors.append("FullQueryPermit::grow disappeared")
    else:
        errors.extend(
            require_ordered_fragments(
                grow,
                "FullQueryPermit::grow",
                (
                    "observed_rows > self.state.max_rows",
                    "let additional = target.saturating_sub(self.state.rows.len())",
                    ".try_reserve_exact(additional)",
                    "self.state.rows.capacity() <= capacity",
                ),
            )
        )

    max_owners = impl_method_body(resources, "ResourceLimits", "max_owner_entries")
    if "self.preaccepted.entries.checked_add(self.accepted.entries)" not in max_owners:
        errors.append(
            "full-query owner bound must remain the checked retained-plus-accepted ledger sum"
        )

    masked_read = mask_rust_non_code(read)
    for retired in ("pool_ids", "replacement_history_hashes"):
        if re.search(rf"\bfn\s+{retired}\s*\(", masked_read):
            errors.append(f"AuthorityReadView::{retired} must remain retired")
    for retired in ("pool_ids", "all_entry_info", "pool_detail"):
        if re.search(rf"(?m)^\s*pub\s*\(super\)\s+fn\s+{retired}\s*\(", masked_query):
            errors.append(f"legacy guarded query::{retired} must remain retired")
    try:
        fee_methods = rust_impl_methods(query, "FeeEstimateReadReceipt")
    except ValueError as error:
        errors.append(str(error))
    else:
        if any(name == "capture" for name, _body, _line in fee_methods):
            errors.append("FeeEstimateReadReceipt::capture must remain retired")
    return errors


def validate_atomic_apply_construction() -> list[str]:
    """Keep clocks and required controls behind one sealed Apply capability."""

    try:
        plan = TX_POOL_AUTHORITY_PLAN.read_text()
        settlement = TX_POOL_AUTHORITY_SETTLEMENT.read_text()
        ingress = (
            TX_POOL_AUTHORITY_PLAN.parent / "plan" / "ingress.rs"
        ).read_text()
        begin = impl_method_body(plan, "ApplyClockReservation", "begin")
        plan_begin = impl_method_body(plan, "ClockPlanReservation", "begin")
        plan_commit = impl_method_body(plan, "ClockPlanReservation", "commit")
        replacement = impl_method_body(plan, "ClockPlanReservation", "replacement")
        insertion = impl_method_body(plan, "ClockPlanReservation", "insertion")
        replacements = impl_method_body(plan, "ClockPlanReservation", "replacements")
        plan_owner_branch = impl_method_body(
            plan, "ClockPlanReservation", "owner_branch"
        )
        branch_replacement = impl_method_body(plan, "OwnerClockBranch", "replacement")
        branch_insertion = impl_method_body(plan, "OwnerClockBranch", "insertion")
        branch_replacements = impl_method_body(plan, "OwnerClockBranch", "replacements")
        branch_adopt = impl_method_body(plan, "OwnerClockBranch", "adopt")
        apply_owner_branch = impl_method_body(
            plan, "ApplyClockReservation", "owner_branch"
        )
        adopt_owner_progress = impl_method_body(
            plan, "ClockPlanReservation", "adopt_owner_progress"
        )
        settlement_plan = function_body(settlement, "plan_settlement")
        ingress_methods = dict(
            (name, body)
            for name, body, _line in rust_impl_methods(
                ingress, "TxPoolAuthority", allow_multiple=True
            )
        )
    except (OSError, ValueError) as error:
        return [f"cannot inspect atomic Apply construction: {error}"]

    errors: list[str] = []
    if settlement_plan is None:
        return ["TxPoolAuthority::plan_settlement disappeared"]
    masked_settlement = mask_rust_non_code(settlement_plan)
    if masked_settlement.count("ApplyClockReservation::begin") != 1:
        errors.append("Ready settlement must begin exactly one sealed Apply clock reservation")
    if masked_settlement.count("clocks.replacements(member_count)") != 1:
        errors.append("Ready settlement must reserve its complete member range exactly once")
    for forbidden in ("next_sequence(", "next_version(", "AuthorityClocks {"):
        if forbidden in masked_settlement:
            errors.append(
                "Ready settlement must not allocate clocks per member; found "
                f"{forbidden!r} outside ApplyClockReservation"
            )
    if "facts.into_iter().zip(versions)" not in masked_settlement:
        errors.append(
            "Ready settlement must consume the exact reserved version range with its sealed facts"
        )

    masked_ingress = mask_rust_non_code(ingress)
    plan_start = masked_ingress.find("ClockPlanReservation::begin(self.clocks)")
    no_apply = masked_ingress.find("if !has_apply", plan_start)
    plan_commit_call = masked_ingress.find(".commit()?", no_apply)
    if min(plan_start, no_apply, plan_commit_call) < 0 or not (
        plan_start < no_apply < plan_commit_call
    ):
        errors.append(
            "retained ingress must keep owner clocks discardable until a nonempty Apply is proven"
        )
    if masked_ingress.count("ClockPlanReservation::begin(self.clocks)") != 1:
        errors.append("retained ingress must own exactly one discardable clock Plan")

    required_fragments = {
        "Plan::begin": (plan_begin, ("Self{clocks}",)),
        "Plan::commit": (
            plan_commit,
            (
                "self.clocks.next_sequence",
                "checked_add(1)",
                "Ok(ApplyClockReservation{sequence,plan:self,})",
            ),
        ),
        "Apply::begin": (
            begin,
            ("ClockPlanReservation::begin(clocks).commit()",),
        ),
        "replacement": (
            replacement,
            ("self.owner_branch().replacement()?", "branch.adopt()", "Ok((version,self))"),
        ),
        "insertion": (
            insertion,
            (
                "self.owner_branch().insertion()?",
                "branch.adopt()",
                "Ok((version,arrival,self))",
            ),
        ),
        "replacements": (
            replacements,
            (
                "self.owner_branch().replacements(members)?",
                "branch.adopt()",
                "Ok((versions,self))",
            ),
        ),
        "Plan::owner_branch": (
            plan_owner_branch,
            (
                "next_version:self.clocks.next_version",
                "next_arrival:self.clocks.next_arrival",
                "parent:self",
            ),
        ),
        "branch::replacement": (
            branch_replacement,
            (
                "self.next_version",
                "checked_add(1)",
                "self.next_version=next_version",
                "Ok((version,self))",
            ),
        ),
        "branch::insertion": (
            branch_insertion,
            (
                "version.0.checked_add(1)",
                "arrival.0.checked_add(1)",
                "self.next_version=next_version",
                "self.next_arrival=next_arrival",
                "Ok((version,arrival,self))",
            ),
        ),
        "branch::replacements": (
            branch_replacements,
            (
                "checked_add(member_count)",
                "self.next_version=next_version",
                "Ok(((first_version..next_version.0).map(EntryVersion),self))",
            ),
        ),
        "branch::adopt": (
            branch_adopt,
            (
                "self.parent.clocks.next_version=self.next_version",
                "self.parent.clocks.next_arrival=self.next_arrival",
            ),
        ),
        "Apply::owner_branch": (
            apply_owner_branch,
            ("self.plan.owner_branch()",),
        ),
        "adopt_owner_progress": (
            adopt_owner_progress,
            (
                "checked_sub(self.clocks.next_version.0)",
                "checked_sub(self.clocks.next_arrival.0)",
                "arrival_advance>version_advance",
                "Ok(self)",
            ),
        ),
    }
    for method, (body, fragments) in required_fragments.items():
        masked = "".join(mask_rust_non_code(body).split())
        for fragment in fragments:
            if fragment not in masked:
                errors.append(
                    f"sealed clock {method} lost fragment {fragment!r}"
                )

    compact_plan = "".join(mask_rust_non_code(plan).split())
    for retired in (
        "OwnerClockCheckpoint",
        "owner_checkpoint(",
        "restore_owner_checkpoint(",
    ):
        if retired in compact_plan:
            errors.append(f"manual owner-clock rollback surface {retired!r} remains")
    branch_declaration = re.search(
        r"struct\s+OwnerClockBranch\s*<[^{}]*>\s*\{(?P<fields>[^{}]*)\}",
        mask_rust_non_code(plan),
    )
    if branch_declaration is None:
        errors.append("OwnerClockBranch must remain one private borrowed stack capability")
    else:
        fields = "".join(branch_declaration.group("fields").split())
        required_fields = (
            "parent:&'parentmutClockPlanReservation",
            "next_version:EntryVersion",
            "next_arrival:Arrival",
        )
        if any(field not in fields for field in required_fields):
            errors.append(
                "OwnerClockBranch lost its exclusive parent borrow or exact owner counters"
            )

    for method_name in ("plan_new_retained_owner", "plan_proposal_owner"):
        body = ingress_methods.get(method_name, "")
        compact_body = "".join(mask_rust_non_code(body).split())
        fragments = (
            "scratch.clocks.owner_branch()",
            "scratch.resources.replace(",
            "scratch.owners.replace(",
            "clock_branch.adopt()",
        )
        positions = [compact_body.find(fragment) for fragment in fragments]
        if any(position < 0 for position in positions) or positions != sorted(positions):
            errors.append(
                f"retained ingress {method_name} must validate resource and owner projections "
                "before adopting its borrowed clock branch"
            )

    membership_compile = function_body(plan, "compile_membership_delta") or ""
    compact_membership = "".join(mask_rust_non_code(membership_compile).split())
    membership_fragments = (
        "lethistory_clocks=clocks.owner_branch()",
        "self.retain_replacement_history(&accepted,&mutremovals,sequence,history_clocks)?",
        "self.plan_membership_resources(",
        "ifretained_history{history_clocks.adopt();}",
    )
    membership_positions = [
        compact_membership.find(fragment) for fragment in membership_fragments
    ]
    if any(position < 0 for position in membership_positions) or membership_positions != sorted(
        membership_positions
    ):
        errors.append(
            "membership must adopt optional-history clocks only after resource fallback closes"
        )

    plan_root = TX_POOL_AUTHORITY_PLAN.parent
    production_plan_sources = [TX_POOL_AUTHORITY_PLAN, *sorted((plan_root / "plan").glob("*.rs"))]
    for path in production_plan_sources:
        try:
            source = mask_rust_non_code(path.read_text())
        except OSError as error:
            errors.append(f"cannot inspect atomic Apply producer {path}: {error}")
            continue
        relative = path.relative_to(REPO_ROOT)
        for retired in ("BatchClockReservation", "IngressClockCursor"):
            if retired in source:
                errors.append(f"retired duplicate clock constructor {retired} remains in {relative}")
        if re.search(r"\bnext_(?:version|arrival|sequence)\s*\(", source):
            errors.append(f"manual clock arithmetic remains outside the sealed clock protocol in {relative}")
        if path != TX_POOL_AUTHORITY_PLAN and "AuthorityClocks {" in source:
            errors.append(f"manual AuthorityClocks construction remains in {relative}")

    masked_plan = mask_rust_non_code(plan)
    reservation_start = masked_plan.find("struct ClockPlanReservation")
    reservation_end = masked_plan.find("impl TxPoolAuthority", reservation_start)
    outside_reservation = masked_plan[:reservation_start] + masked_plan[reservation_end:]
    if "AuthorityClocks {" in outside_reservation:
        errors.append("manual AuthorityClocks construction remains outside the sealed clock protocol")
    if re.search(r"\bimpl\s+Default\s+for\s+TransitionControls\b", masked_plan):
        errors.append("TransitionControls must not regain a default partial-construction path")
    if "TransitionControls::default" in masked_plan:
        errors.append("a transition producer still uses defaultable projection controls")
    controls = impl_method_body(plan, "TransitionControls", "dependency_and_effect")
    if not all(fragment in mask_rust_non_code(controls) for fragment in ("dependency", "effect")):
        errors.append("TransitionControls lost its closed dependency-and-effect constructor")
    return errors


def validate_dependency_maintenance_successor() -> list[str]:
    """Keep maintenance Apply construction behind one nonempty Plan value."""

    try:
        dependency = TX_POOL_AUTHORITY_DEPENDENCY.read_text()
        plan = TX_POOL_AUTHORITY_PLAN.read_text()
        maintenance = function_body(dependency, "plan_maintenance")
        compile_maintenance = function_body(plan, "plan_dependency_maintenance")
    except (OSError, ValueError) as error:
        return [f"cannot inspect dependency maintenance construction: {error}"]
    if maintenance is None or compile_maintenance is None:
        return ["dependency maintenance producer or atomic compiler disappeared"]

    errors: list[str] = []
    signature = re.search(
        r"\bfn\s+plan_maintenance\s*\([^;{]*\)\s*"
        r"->\s*Result\s*<\s*DependencyMaintenancePlan\s*,\s*DependencyError\s*>",
        mask_rust_non_code(dependency),
        re.S,
    )
    if signature is None:
        errors.append("dependency maintenance must produce a concrete nonempty Plan")
    compact_maintenance = "".join(mask_rust_non_code(maintenance).split())
    if "Ok(DependencyMaintenancePlan(step))" not in compact_maintenance:
        errors.append("dependency maintenance lost its sealed successor construction")
    if "DependencyControlDelta::None" in compact_maintenance:
        errors.append("dependency maintenance may not construct an empty control successor")
    compact_compile = "".join(mask_rust_non_code(compile_maintenance).split())
    if ".plan_maintenance(ticket)?.into_control()" not in compact_compile:
        errors.append("the atomic compiler must consume the sealed successor directly")
    return errors


def validate_dependency_maintenance_producers() -> list[str]:
    """Bind maintenance decisions to the sole legal cut and projection producers."""

    paths = (
        TX_POOL_AUTHORITY_PLAN,
        TX_POOL_AUTHORITY_DEPENDENCY,
        TX_POOL_AUTHORITY_COMPUTE_EXCHANGE,
        TX_POOL_AUTHORITY_CHAIN_TRANSITION,
        TX_POOL_AUTHORITY_SETTLEMENT,
        TX_POOL_AUTHORITY_INGRESS_PLAN,
    )
    try:
        sources = {path: path.read_text() for path in paths}
        dependency = sources[TX_POOL_AUTHORITY_DEPENDENCY]
        plan = sources[TX_POOL_AUTHORITY_PLAN]
        dependency_methods = {
            name: body
            for name, body, _line in rust_impl_methods(dependency, "DependencyFrontier")
        }
        authority_methods = {
            path: {
                name: body
                for name, body, _line in rust_impl_methods(
                    source, "TxPoolAuthority", allow_multiple=True
                )
            }
            for path, source in sources.items()
            if path != TX_POOL_AUTHORITY_DEPENDENCY
        }
        prepared_apply_methods = {
            name: body
            for name, body, _line in rust_impl_methods(plan, "PreparedApply")
        }
        retained_ingress_apply = required_function_body(
            sources[TX_POOL_AUTHORITY_INGRESS_PLAN], "apply_retained_ingress"
        )
        compute_exchange_apply = required_function_body(
            sources[TX_POOL_AUTHORITY_COMPUTE_EXCHANGE], "apply_compute_exchange"
        )
    except (OSError, ValueError) as error:
        return [f"cannot inspect dependency maintenance producers: {error}"]

    errors: list[str] = []

    def compact(body: str) -> str:
        return "".join(mask_rust_non_code(body).split())

    def require_fragments(body: str, owner: str, fragments: tuple[str, ...]) -> None:
        compact_body = compact(body)
        for fragment in fragments:
            if fragment not in compact_body:
                errors.append(f"{owner} lost sealed producer fragment {fragment!r}")

    plan_source_paths = [
        TX_POOL_AUTHORITY_PLAN,
        *sorted((TX_POOL_AUTHORITY_PLAN.parent / "plan").glob("*.rs")),
    ]

    def closed_plan_method_surface(
        pattern: re.Pattern[str],
        expected: dict[Path, set[str]],
        label: str,
    ) -> None:
        """Reject a new call site outside the complete named Plan surface."""

        for path in plan_source_paths:
            try:
                source = sources.get(path, path.read_text())
                masked = mask_rust_non_code(source)
            except (OSError, ValueError) as error:
                errors.append(f"cannot inspect {label} source {path}: {error}")
                continue
            source_hits = len(pattern.findall(masked))
            methods = authority_methods.get(path)
            if methods is None:
                if source_hits == 0:
                    methods = {}
                else:
                    try:
                        methods = {
                            name: body
                            for name, body, _line in rust_impl_methods(
                                source, "TxPoolAuthority", allow_multiple=True
                            )
                        }
                    except ValueError as error:
                        errors.append(str(error))
                        continue
            actual = {
                name for name, body in methods.items() if pattern.search(body) is not None
            }
            wanted = expected.get(path, set())
            if actual != wanted:
                errors.append(
                    f"{label} callers changed in {path.relative_to(REPO_ROOT)}: "
                    f"expected {sorted(wanted)}, found {sorted(actual)}"
                )
            method_hits = sum(len(pattern.findall(body)) for body in methods.values())
            if method_hits != source_hits:
                errors.append(
                    f"{label} call escaped a TxPoolAuthority method in "
                    f"{path.relative_to(REPO_ROOT)}"
                )

    # The ticket is a private value with one constructor. Its action and
    # successor are decided under the same authority borrow before any Apply
    # capability exists, so no external or intervening producer can alter the
    # dependency frontier between the two checks.
    masked_dependency = mask_rust_non_code(dependency)
    ticket_sites = list(
        re.finditer(r"\bDependencyMaintenanceTicket\s*\{", masked_dependency)
    )
    ticket_declarations = [
        site
        for site in ticket_sites
        if re.search(
            r"\b(?:struct|impl)\s+$",
            masked_dependency[max(0, site.start() - 24) : site.start()],
        )
    ]
    ticket_definitions = [
        site
        for site in ticket_declarations
        if re.search(r"\bstruct\s+$", masked_dependency[max(0, site.start() - 24) : site.start()])
    ]
    ticket_initializers = [site for site in ticket_sites if site not in ticket_declarations]
    if (
        len(ticket_definitions) != 1
        or len(ticket_declarations) != 2
        or len(ticket_initializers) != 1
    ):
        errors.append(
            "DependencyMaintenanceTicket must retain one private declaration and one constructor"
        )
    elif compact(dependency_methods.get("next_maintenance", "")).count(
        "DependencyMaintenanceTicket{"
    ) != 1:
        errors.append("DependencyMaintenanceTicket must be constructed only by next_maintenance")
    if ticket_definitions:
        opening = masked_dependency.find("{", ticket_definitions[0].start())
        closing = matching_brace(masked_dependency, opening)
        if closing is None or re.search(r"\bpub\b", masked_dependency[opening + 1 : closing]):
            errors.append("DependencyMaintenanceTicket fields must remain private")

    maintenance_compile = authority_methods[TX_POOL_AUTHORITY_PLAN].get(
        "plan_dependency_maintenance", ""
    )
    compact_maintenance_compile = compact(maintenance_compile)
    ordered_ticket_fragments = (
        "self.dependencies.next_maintenance()?",
        "ticket.action(&self.dependencies,hash.as_ref().and_then(|hash|self.entries.get(hash)),)?",
        "self.dependencies.plan_maintenance(ticket)?.into_control()",
        "ApplyClockReservation::begin(self.clocks)?",
    )
    positions = [
        compact_maintenance_compile.find(fragment) for fragment in ordered_ticket_fragments
    ]
    if any(position < 0 for position in positions) or positions != sorted(positions):
        errors.append(
            "plan_dependency_maintenance must select, decide and consume one ticket before Apply"
        )
    for fragment in (
        "self.dependencies.next_maintenance()",
        "ticket.action(",
        "self.dependencies.plan_maintenance(ticket)",
    ):
        if compact_maintenance_compile.count(fragment) != 1:
            errors.append(
                "plan_dependency_maintenance must use exactly one same-guard ticket operation "
                f"{fragment!r}"
            )
    if ".await" in maintenance_compile:
        errors.append("dependency maintenance may not suspend while its ticket is live")

    # Every raw DependencyCut constructor is closed over a named production
    # method. This prevents a new equal-cut producer from bypassing the clock
    # theorem or the exact observation boundary without updating this contract.
    cut_constructor = re.compile(r"\bDependencyCut\s*\(")
    expected_cut_methods = {
        TX_POOL_AUTHORITY_PLAN: {
            "dependency_observation_cut",
            "plan_membership_dependency_delta",
            "retain_replacement_history",
            "plan_dependency_loss",
            "plan_owner_removal_batch",
        },
        TX_POOL_AUTHORITY_COMPUTE_EXCHANGE: {"compile_compute_exchange_state"},
        TX_POOL_AUTHORITY_CHAIN_TRANSITION: {"plan_chain_transition"},
        TX_POOL_AUTHORITY_SETTLEMENT: {"plan_settlement"},
        TX_POOL_AUTHORITY_INGRESS_PLAN: set(),
    }
    for path, expected in expected_cut_methods.items():
        methods = authority_methods[path]
        actual = {
            name
            for name, body in methods.items()
            if cut_constructor.search(body) is not None
        }
        relative = path.relative_to(REPO_ROOT)
        if actual != expected:
            errors.append(
                f"dependency cut constructors changed in {relative}: "
                f"expected {sorted(expected)}, found {sorted(actual)}"
            )
        method_site_count = sum(
            len(cut_constructor.findall(body)) for body in methods.values()
        )
        source_site_count = len(cut_constructor.findall(mask_rust_non_code(sources[path])))
        if method_site_count != source_site_count:
            errors.append(f"dependency cut constructor escaped TxPoolAuthority in {relative}")
        for name in actual:
            if len(cut_constructor.findall(methods[name])) != 1:
                errors.append(f"{name} must own exactly one DependencyCut constructor")

    expected_cut_paths = set(expected_cut_methods)
    authority_root = TX_POOL_AUTHORITY_DEPENDENCY.parent
    state_path = authority_root / "state.rs"
    for path in sorted(authority_root.rglob("*.rs")):
        if "tests" in path.relative_to(authority_root).parts:
            continue
        try:
            masked = mask_rust_non_code(path.read_text())
        except (OSError, ValueError) as error:
            errors.append(f"cannot inspect complete dependency cut surface {path}: {error}")
            continue
        hits = len(cut_constructor.findall(masked))
        if path == state_path:
            if hits != 1 or "struct DependencyCut(" not in masked:
                errors.append("DependencyCut must retain one sealed newtype declaration")
        elif path not in expected_cut_paths and hits:
            errors.append(
                "dependency cut constructor escaped the closed producer surface in "
                f"{path.relative_to(REPO_ROOT)}"
            )

    closed_plan_method_surface(
        re.compile(r"\.dependencies\s*\.plan_events\s*\("),
        {
            TX_POOL_AUTHORITY_PLAN: {
                "plan_membership_dependency_delta",
                "plan_dependency_loss",
                "plan_owner_removal_batch",
            },
            TX_POOL_AUTHORITY_CHAIN_TRANSITION: {"plan_chain_transition"},
            TX_POOL_AUTHORITY_SETTLEMENT: {"plan_settlement"},
        },
        "dependency event",
    )

    require_fragments(
        authority_methods[TX_POOL_AUTHORITY_PLAN].get("dependency_observation_cut", ""),
        "TxPoolAuthority::dependency_observation_cut",
        (
            "DependencyCut(ApplySequence(self.clocks.next_sequence.0.saturating_sub(1)))",
        ),
    )
    require_fragments(
        authority_methods[TX_POOL_AUTHORITY_COMPUTE_EXCHANGE].get(
            "compile_compute_exchange_state", ""
        ),
        "TxPoolAuthority::compile_compute_exchange_state",
        (
            "ApplyClockReservation::begin(self.clocks)?",
            "letsequence=clocks.sequence()",
            "crate::authority::state::DependencyCut(sequence)",
            "clocks:clocks.finish()",
        ),
    )
    require_fragments(
        authority_methods[TX_POOL_AUTHORITY_PLAN].get("compile_membership_delta", ""),
        "TxPoolAuthority::compile_membership_delta",
        (
            "letsequence=clocks.sequence()",
            "lethistory_clocks=clocks.owner_branch()",
            "self.retain_replacement_history(&accepted,&mutremovals,sequence,history_clocks)?",
            "ifretained_history{history_clocks.adopt();}",
            "self.plan_membership_dependency_delta(existing.as_ref(),&after,&removals,sequence)?",
            "clocks:clocks.finish()",
        ),
    )
    for path, method, sequence_name in (
        (TX_POOL_AUTHORITY_PLAN, "plan_membership_dependency_delta", "sequence"),
        (TX_POOL_AUTHORITY_PLAN, "plan_dependency_loss", "sequence"),
        (TX_POOL_AUTHORITY_PLAN, "plan_owner_removal_batch", "sequence"),
        (TX_POOL_AUTHORITY_CHAIN_TRANSITION, "plan_chain_transition", "sequence"),
        (TX_POOL_AUTHORITY_SETTLEMENT, "plan_settlement", "source_sequence"),
    ):
        body = authority_methods[path].get(method, "")
        compact_body = compact(body)
        if compact_body.count(".plan_events(") != 1 or compact_body.count(
            f"DependencyCut({sequence_name})"
        ) != 1:
            errors.append(
                f"TxPoolAuthority::{method} must publish one event at its sealed Apply cut"
            )

    # Replacement history can be created in the same membership Apply as a
    # dependency level. Its trigger construction and final-owner availability
    # projection are deliberately disjoint: candidate-spent inputs and inputs
    # whose backing producer is removed cannot be published available. A
    # same-cut level is therefore a definitive loss, for which the second
    # strict availability conjunct remains false.
    require_fragments(
        authority_methods[TX_POOL_AUTHORITY_PLAN].get("retain_replacement_history", ""),
        "TxPoolAuthority::retain_replacement_history",
        (
            "candidate_inputs.binary_search(input).is_ok()||(producer_removed&&!accepted.proof.is_chain_input(input))",
            "removed.contains(&RawTxHash(dependency.tx_hash()))&&!accepted.proof.is_chain_dependency(dependency)",
            "DependencyCut(sequence)",
        ),
    )
    require_fragments(
        authority_methods[TX_POOL_AUTHORITY_PLAN].get(
            "collect_released_replacement_inputs", ""
        ),
        "TxPoolAuthority::collect_released_replacement_inputs",
        (
            "ProjectedRemovalSet::Replacement(&removed)",
            "ReleasedInputContext::Replacement{candidate_inputs:&candidate_inputs,}",
        ),
    )
    require_fragments(
        authority_methods[TX_POOL_AUTHORITY_PLAN].get(
            "released_input_survives_final_owner_set", ""
        ),
        "TxPoolAuthority::released_input_survives_final_owner_set",
        (
            "ReleasedInputContext::Replacement{candidate_inputs}=>{ifcandidate_inputs.contains(input){returnOk(false);}",
            "iffinal_owners.contains_removed(&parent){returnOk(false);}",
        ),
    )

    # Accepted loss is coupled to the complete owner replacement and becomes
    # visible only after every removed dependency slot has detached. This is
    # the static half of the real lifecycle closure refinement.
    apply_batch = dependency_methods.get("apply_batch", "")
    compact_apply_batch = compact(apply_batch)
    ordered_apply_fragments = (
        "forslotin&delta.removed{self.detach(slot);}",
        "forslotin&delta.added{self.attach(slot);}",
        "forslotin&delta.removed{self.prune_orphaned(slot);}",
        "self.apply_control(delta.control)",
    )
    apply_positions = [compact_apply_batch.find(fragment) for fragment in ordered_apply_fragments]
    if any(position < 0 for position in apply_positions) or apply_positions != sorted(
        apply_positions
    ):
        errors.append(
            "DependencyFrontier::apply_batch must detach the owner closure before event publication"
        )
    require_fragments(
        authority_methods[TX_POOL_AUTHORITY_PLAN].get(
            "plan_membership_dependency_delta", ""
        ),
        "TxPoolAuthority::plan_membership_dependency_delta",
        (
            "letremoved=self.entries.get(&removal.hash)",
            "changes.push((Some(removed),removal.after()))",
            "letlost=self.collect_dependency_loss_keys(removed_entries)?.keys",
            "self.dependencies.plan_replacements(changes)?",
            "Ok(delta.with_control(control))",
        ),
    )
    require_fragments(
        authority_methods[TX_POOL_AUTHORITY_PLAN].get("plan_owner_removal_batch", ""),
        "TxPoolAuthority::plan_owner_removal_batch",
        (
            "accepted_removals.extend(hashes.iter().filter(|hash|matches!(self.entries.get(*hash),Some(OwnedTx::Accepted(_)))).cloned(),)",
            "letmembership=self.prepare_chain_projection(&accepted_removals,&HashMap::new())?",
            "self.collect_dependency_loss_keys(owner_refs.iter().copied())?.keys",
            "self.dependencies.plan_replacements(owner_refs.iter().copied().map(|owner|(Some(owner),None)))?.with_control(dependency_control)",
        ),
    )
    chain_transition_plan = authority_methods[TX_POOL_AUTHORITY_CHAIN_TRANSITION].get(
        "plan_chain_transition", ""
    )
    require_fragments(
        chain_transition_plan,
        "TxPoolAuthority::plan_chain_transition",
        (
            "letmembership=self.prepare_chain_projection(&accepted_removals,&status_after)?",
            "self.dependencies.plan_primary_replacements(changes.iter().map(|change|(change.before.as_ref(),change.after.as_ref())),)?.with_control(control)",
        ),
    )
    errors.extend(
        validate_typed_adjacent_uniqueness(
            chain_transition_plan,
            "TxPoolAuthority::plan_chain_transition owner-key uniqueness",
            "changes.sort_unstable_by(|left,right|left.key.cmp(&right.key))",
            "changes",
            "left.key==right.key",
        )
    )
    settlement_compact = compact(
        authority_methods[TX_POOL_AUTHORITY_SETTLEMENT].get("plan_settlement", "")
    )
    if "Vec::new(),super::super::state::DependencyCut(source_sequence)" not in settlement_compact:
        errors.append("independent settlement must not publish a definitive dependency loss")

    direct_loss_callers = {
        name
        for name, body in authority_methods[TX_POOL_AUTHORITY_PLAN].items()
        if "self.plan_dependency_loss(" in body
    }
    if direct_loss_callers != {
        "plan_preaccepted_terminalization",
        "prepare_compute_rejection",
    }:
        errors.append(
            "direct dependency-loss caller set changed: "
            f"{sorted(direct_loss_callers)}"
        )
    for caller in direct_loss_callers:
        require_fragments(
            authority_methods[TX_POOL_AUTHORITY_PLAN][caller],
            f"TxPoolAuthority::{caller}",
            (
                "letOwnedTx::PreAccepted(",
                "self.plan_dependency_loss(std::iter::once(&existing),sequence)?",
                "TransitionControls::dependency_and_effect(",
            ),
        )

    # The projection compiler receives current owners through a finite closed
    # caller set. Overlay positions and typed owner-removal keys make duplicate
    # before identities impossible, while every before slot comes from the sole
    # entries map whose reverse dependency projection is checked after Apply.
    batch_call = re.compile(r"\.dependencies\s*\.plan_(?:primary_)?replacements\s*\(")
    expected_batch_callers = {
        TX_POOL_AUTHORITY_PLAN: {
            "plan_membership_dependency_delta",
            "plan_owner_removal_batch",
        },
        TX_POOL_AUTHORITY_INGRESS_PLAN: {"plan_retained_admission_batch"},
        TX_POOL_AUTHORITY_SETTLEMENT: {"plan_settlement"},
        TX_POOL_AUTHORITY_CHAIN_TRANSITION: {"plan_chain_transition"},
        TX_POOL_AUTHORITY_COMPUTE_EXCHANGE: {"compile_compute_exchange_state"},
    }
    closed_plan_method_surface(
        batch_call, expected_batch_callers, "dependency batch projection"
    )

    for path, owner_name in (
        (TX_POOL_AUTHORITY_INGRESS_PLAN, "retained ingress OwnerOverlay"),
        (TX_POOL_AUTHORITY_COMPUTE_EXCHANGE, "compute exchange OwnerOverlay"),
    ):
        try:
            overlay_replace = impl_method_body(sources[path], "OwnerOverlay", "replace")
        except ValueError as error:
            errors.append(str(error))
            continue
        require_fragments(
            overlay_replace,
            owner_name,
            (
                "self.positions.get(&key).copied()",
                "authority.entries.get(&key)",
                "self.positions.insert(key.clone(),position)",
                "self.changes.push(OwnerChange{key,before,after})",
            ),
        )

    closed_plan_method_surface(
        re.compile(r"\.dependencies\s*\.plan_stable_replace\s*\("),
        {TX_POOL_AUTHORITY_PLAN: {"apply_compute_cancellation"}},
        "stable dependency replacement",
    )
    require_fragments(
        authority_methods[TX_POOL_AUTHORITY_PLAN].get("apply_compute_cancellation", ""),
        "TxPoolAuthority::apply_compute_cancellation",
        (
            "letexisting=self.entries.get(&token.hash).ok_or(ComputeCancellationError::Obsolete(StalePlan::Missing))?.clone()",
            "PreAcceptedPhase::Computing(_)",
            "with_preaccepted_phase(PreAcceptedPhase::Queued(QueuedWork::Resolve)",
            "self.dependencies.plan_stable_replace(&existing,&after)",
        ),
    )
    require_fragments(
        dependency_methods.get("contains", ""),
        "DependencyFrontier::contains",
        (
            "self.consumers.get(key).is_some_and(|consumers|consumers.contains(&slot.hash))",
            "self.keys_by_origin.get(&key.origin()).is_some_and(|keys|keys.contains(key))",
            "self.waiters.get(key).is_some_and(|waiters|waiters.contains(&slot.hash))",
        ),
    )

    # Generate the complete primary-owner mutation surface independently from
    # the Plan-call inventory above. Every direct entries-map mutation must
    # publish its derived dependency projection in the same Apply body; whole
    # generation replacement must replace both structures in that same body.
    entry_mutation = re.compile(
        r"\bauthority\.entries\s*\.\s*(?:insert|remove)\s*\("
        r"|\bstd::mem::replace\s*\(\s*&mut\s+authority\.entries\b"
    )
    dependency_mutation = re.compile(
        r"\bauthority\.dependencies\s*\.\s*apply(?:_batch)?\s*\("
        r"|\bstd::mem::replace\s*\(\s*&mut\s+authority\.dependencies\b"
    )
    owner_apply_bodies = {
        f"PreparedApply::{name}": body
        for name, body in prepared_apply_methods.items()
        if entry_mutation.search(mask_rust_non_code(body)) is not None
    }
    owner_apply_bodies.update(
        {
            "apply_retained_ingress": retained_ingress_apply,
            "apply_compute_exchange": compute_exchange_apply,
        }
    )
    expected_owner_applies = {
        "PreparedApply::apply_entry",
        "PreparedApply::apply_membership",
        "PreparedApply::apply_independent",
        "PreparedApply::apply_owner_removal",
        "PreparedApply::apply_chain",
        "PreparedApply::apply_clear_pool",
        "apply_retained_ingress",
        "apply_compute_exchange",
    }
    if set(owner_apply_bodies) != expected_owner_applies:
        errors.append(
            "primary-owner Apply surface changed: "
            f"expected {sorted(expected_owner_applies)}, "
            f"found {sorted(owner_apply_bodies)}"
        )
    owned_mutation_sites = 0
    for owner, body in owner_apply_bodies.items():
        masked = mask_rust_non_code(body)
        entry_sites = len(entry_mutation.findall(masked))
        dependency_sites = len(dependency_mutation.findall(masked))
        owned_mutation_sites += entry_sites
        if entry_sites == 0 or dependency_sites != 1:
            errors.append(
                f"{owner} must pair its primary-owner mutation with exactly one "
                "dependency projection mutation"
            )
    discovered_mutation_sites = 0
    for path in sorted(TX_POOL_AUTHORITY_PLAN.parent.rglob("*.rs")):
        if "tests" in path.relative_to(TX_POOL_AUTHORITY_PLAN.parent).parts:
            continue
        try:
            discovered_mutation_sites += len(
                entry_mutation.findall(mask_rust_non_code(path.read_text()))
            )
        except (OSError, ValueError) as error:
            errors.append(f"cannot inspect primary-owner mutation source {path}: {error}")
    if discovered_mutation_sites != owned_mutation_sites:
        errors.append(
            "a primary-owner mutation escaped the closed same-Apply dependency "
            f"surface: discovered {discovered_mutation_sites}, owned {owned_mutation_sites}"
        )
    return errors


def validate_sparse_resource_set_transition() -> list[str]:
    """Bind resource updates to change-local work without a ledger recount."""

    try:
        resources = TX_POOL_AUTHORITY_RESOURCES.read_text()
        resource_batch = impl_method_body(
            resources, "ResourceLedger", "plan_batch"
        )
    except (OSError, ValueError) as error:
        return [f"cannot inspect sparse resource set transition: {error}"]

    errors: list[str] = []
    compact_resource = "".join(mask_rust_non_code(resource_batch).split())
    remove = compact_resource.find("for(_,expected,_)in&changes")
    add = compact_resource.find("for(_,_,after)in&changes")
    limits = compact_resource.find("if!preaccepted.fits(self.limits.preaccepted)")
    if min(remove, add, limits) < 0 or not remove < add < limits:
        errors.append(
            "ResourceLedger::plan_batch must remove the complete old set, add the complete "
            "new set and only then validate aggregate limits"
        )
    for required in (
        "letpeer_capacity=changes.len().checked_mul(2)",
        "keys.try_reserve(changes.len())",
        "peer_updates.try_reserve(peer_capacity)",
    ):
        if required not in compact_resource:
            errors.append(
                f"sparse resource batch lost checked change-local bound {required!r}"
            )
    if "self.charges.values()" in compact_resource:
        errors.append("ordinary resource batches must not regain a full-ledger recount")
    return errors


def validate_finite_scheduler_owner_ring() -> list[str]:
    """Bind scheduler projection and search to one finite owner union."""

    try:
        scheduler = TX_POOL_AUTHORITY_SCHEDULER.read_text()
        compute_exchange = (
            TX_POOL_AUTHORITY_PLAN.parent / "plan" / "compute_exchange.rs"
        ).read_text()
        scheduler_slot = impl_method_body(scheduler, "FairFrontier", "slot")
        ring_next = impl_method_body(
            scheduler, "FairLane", "next_excluding_with_overlay"
        )
        ring_eligibility = impl_method_body(
            scheduler, "FairLane", "overlay_owner_is_eligible"
        )
        wave_owner_count = impl_method_body(
            scheduler, "SchedulerExchangeWave", "owner_count"
        )
        search = function_body(compute_exchange, "search_exchange_permit")
    except (OSError, ValueError) as error:
        return [f"cannot inspect finite scheduler owner ring: {error}"]

    errors: list[str] = []
    if search is None:
        return ["TxPoolAuthority::search_exchange_permit disappeared"]

    compact_slot = "".join(mask_rust_non_code(scheduler_slot).split())
    for required in (
        "PreAcceptedPhase::Ready(_)=>SchedulerSlot::Ready",
        "PreAcceptedPhase::Computing(_)|PreAcceptedPhase::Waiting(_)=>returnOk(None)",
    ):
        if required not in compact_slot:
            errors.append(
                f"the complete scheduler set projection lost production arm {required!r}"
            )

    compact_eligibility = "".join(mask_rust_non_code(ring_eligibility).split())
    if "self.owner_is_eligible(lane,capability,owner)||overlay.owner_is_eligible(lane,capability,owner)" not in compact_eligibility:
        errors.append("scheduler overlay eligibility must remain the owner union")

    compact_ring = "".join(mask_rust_non_code(ring_next).split())
    for required in (
        "self.owner_count(lane,capability).checked_add(overlay.owner_count(lane,capability))?",
        "for_in0..owner_count",
    ):
        if required not in compact_ring:
            errors.append(f"scheduler ring lost finite probe fragment {required!r}")
    if "while" in compact_ring or "loop{" in compact_ring:
        errors.append("scheduler ring traversal must remain a finite owner-count loop")

    compact_owner_count = "".join(mask_rust_non_code(wave_owner_count).split())
    if "frontier.owner_count(lane,capability).checked_add(overlay.owner_count(lane,capability)).ok_or(SchedulerError::Arithmetic)" not in compact_owner_count:
        errors.append(
            "SchedulerExchangeWave::owner_count must remain the checked committed-plus-overlay sum"
        )

    compact_search = "".join(mask_rust_non_code(search).split())
    for required in (
        "for_in0..wave.owner_count(permit)?",
        "Some(owner)=>wave.next_after(permit,owner)",
        "None=>wave.next(permit)",
        "cursor=Some(ticket.owner())",
    ):
        if required not in compact_search:
            errors.append(
                f"compute resource-eligibility search lost finite ring fragment {required!r}"
            )
    if "while" in compact_search or "loop{" in compact_search:
        errors.append("compute resource-eligibility search must remain a finite owner-count loop")
    return errors


def validate_ready_priority_progress() -> list[str]:
    """Bind Ready OCC to the exact strict-priority prefix and one fair round."""

    try:
        scheduler = TX_POOL_AUTHORITY_SCHEDULER.read_text()
        runtime = TX_POOL_AUTHORITY_RUNTIME.read_text()
        worker = TX_POOL_AUTHORITY_WORKER.read_text()
        prefix = impl_method_body(
            scheduler, "FairFrontier", "ready_common_prefix_len"
        )
        recheck = impl_method_body(runtime, "AuthorityStore", "complete_ready_batch")
        finish = impl_method_body(runtime, "ReadyRecheckOutcome", "finish")
        drive = impl_method_body(runtime, "AuthorityRuntime", "try_drive_ready")
        ready_loop = required_function_body(worker, "run_ready_driver_loop")
    except (OSError, ValueError) as error:
        return [f"cannot inspect Ready strict-priority progress: {error}"]

    errors: list[str] = []

    def prefix_relation_errors(body: str) -> list[str]:
        dense = "".join(mask_rust_non_code(body).split())
        missing = []
        for fragment in (
            "self.ready.iter().rev().take(MAX_READY_BATCH).zip(captured)",
            ".take_while(|(current,captured)|{current.hash()==captured.0&&current.version()==captured.1})",
            ".count()",
        ):
            if fragment not in dense:
                missing.append(fragment)
        if any(token in dense for token in ("collect::<", "Vec::", "vec![")):
            missing.append("allocation-free bounded frontier comparison")
        return missing

    missing_prefix = prefix_relation_errors(prefix)
    if missing_prefix:
        errors.append(
            "Ready common-prefix relation lost exact hash/version order or its linear bound: "
            f"{missing_prefix}"
        )
    prefix_canary = prefix.replace("current.version() == captured.1", "true", 1)
    if not prefix_relation_errors(prefix_canary):
        errors.append("Ready common-prefix gate failed its missing-version negative canary")

    dense_recheck = "".join(mask_rust_non_code(recheck).split())
    recheck_fragments = (
        "self.authority.ready_common_prefix_len(",
        "ifprefix_len==0{returnOk(ReadyRecheckOutcome::HeadChanged(batch));}",
        "letmuttail=tail.into_iter();",
        "intail.by_ref().take(prefix_len.saturating_sub(1))",
        "ReadyRecheckOutcome::UnchangedPrefix",
        "discarded_tail:tail",
    )
    positions = [dense_recheck.find(fragment) for fragment in recheck_fragments]
    if any(position < 0 for position in positions) or positions != sorted(positions):
        errors.append(
            "Ready recheck must classify a changed head before completing exactly the "
            "longest unchanged prefix and carrying weaker scratch outside the guard"
        )
    dense_finish = "".join(mask_rust_non_code(finish).split())
    for fragment in (
        "Self::HeadChanged(stale)=>{drop(stale);None}",
        "drop(discarded_tail);Some(batch)",
    ):
        if fragment not in dense_finish:
            errors.append(
                "Ready recheck scratch must retire outside the authority guard; "
                f"missing {fragment!r}"
            )

    dense_drive = "".join(mask_rust_non_code(drive).split())
    drive_fragments = (
        "store.complete_ready_batch(prepared)",
        "letSome(batch)=rechecked.finish()else{returnErr(AuthorityDriverError::Stale);}",
        "batch.validate()",
    )
    positions = [dense_drive.find(fragment) for fragment in drive_fragments]
    if any(position < 0 for position in positions) or positions != sorted(positions):
        errors.append(
            "Ready runtime must release its second read guard before retiring stale scratch "
            "and validating the unchanged prefix"
        )

    dense_loop = "".join(mask_rust_non_code(ready_loop).split())
    loop_fragments = (
        "ifcancel.is_cancelled(){returnOk(());}",
        "observe_attempt();",
        "runtime.try_drive_ready()",
        "ifstep==WorkerStep::Progress{",
        "tokio::task::yield_now().await;",
        "continue;",
    )
    positions = [dense_loop.find(fragment) for fragment in loop_fragments]
    if any(position < 0 for position in positions) or positions != sorted(positions):
        errors.append(
            "Ready driver must check cancellation and yield after each bounded progress attempt"
        )
    if dense_loop.count("tokio::task::yield_now().await;") != 1:
        errors.append("Ready driver must own exactly one cooperative progress handoff")
    return errors


def validate_expiry_index_producers() -> list[str]:
    """Bind bounded expiry producers to explicit wall and monotonic clock domains."""

    try:
        plan = TX_POOL_AUTHORITY_PLAN.read_text()
        indexes = TX_POOL_AUTHORITY_INDEXES.read_text()
        ingress = TX_POOL_AUTHORITY_INGRESS.read_text()
        validation = TX_POOL_AUTHORITY_VALIDATION.read_text()
        runtime = TX_POOL_AUTHORITY_RUNTIME.read_text()
        ban = TX_POOL_AUTHORITY_BAN.read_text()
        remote_index = required_function_body(indexes, "due_remote")
        accepted_index = required_function_body(indexes, "due_accepted")
        remote_plan = required_function_body(plan, "plan_remote_expiry")
        accepted_plan = required_function_body(plan, "plan_accepted_expiry")
        compiler = required_function_body(plan, "compile_administrative_removal")
        remote = required_function_body(ingress, "remote")
        remote_at = required_function_body(ingress, "remote_at")
        membership = required_function_body(validation, "validate_membership")
        expire_remote = impl_method_body(runtime, "AuthorityRuntime", "expire_remote_due")
        expire_accepted = impl_method_body(
            runtime, "AuthorityRuntime", "expire_accepted_due"
        )
        ban_deadline = impl_method_body(
            ban, "PeerBanDeadline", "after_malformed_ban"
        )
        ban_active = impl_method_body(ban, "PeerBanDeadline", "is_active_at")
    except (OSError, ValueError) as error:
        return [f"cannot inspect expiry index producers: {error}"]

    errors: list[str] = []
    dense_remote_index = "".join(mask_rust_non_code(remote_index).split())
    dense_accepted_index = "".join(mask_rust_non_code(accepted_index).split())
    dense_remote_plan = "".join(mask_rust_non_code(remote_plan).split())
    dense_accepted_plan = "".join(mask_rust_non_code(accepted_plan).split())
    dense_compiler = "".join(mask_rust_non_code(compiler).split())
    if ".take_while(|deadline|deadline.expires_at<=now).take(limit)" not in dense_remote_index:
        errors.append("due_remote must retain the bounded deadline <= cutoff prefix")
    if (
        ".take_while(|deadline|deadline.accepted_at<=cutoff).take(limit)"
        not in dense_accepted_index
    ):
        errors.append("due_accepted must retain the bounded accepted-at <= cutoff prefix")

    clock_fragments = {
        "Remote ingress": (remote, "ckb_systemtime::unix_time().as_secs()"),
        "Remote deadline": (
            remote_at,
            "admitted_at_secs.saturating_add(REMOTE_RESIDENCY_BLOCKS.saturating_mul(MAX_BLOCK_INTERVAL))",
        ),
        "Accepted membership": (
            membership,
            "AcceptedAtMillis(ckb_systemtime::unix_time_as_millis())",
        ),
        "Remote expiry": (
            expire_remote,
            "RemoteDeadline(ckb_systemtime::unix_time().as_secs())",
        ),
        "Accepted expiry": (
            expire_accepted,
            "ckb_systemtime::unix_time_as_millis().checked_sub(self.expiry_policy.accepted_residency_millis)",
        ),
        "Peer-ban deadline": (
            ban_deadline,
            "now.checked_add(Duration::from_secs(MALFORMED_TX_BAN_SECONDS))",
        ),
        "Peer-ban activity": (ban_active, "Self::At(deadline)=>deadline>now"),
    }
    for owner, (body, required) in clock_fragments.items():
        if required not in "".join(mask_rust_non_code(body).split()):
            errors.append(f"{owner} lost clock-domain fragment {required!r}")
    if any(
        "Instant::" in mask_rust_non_code(body)
        for body in (remote, remote_at, membership, expire_remote, expire_accepted)
    ):
        errors.append("wall-clock ownership must not be replaced by process Instant")
    if any(
        "ckb_systemtime::" in mask_rust_non_code(body)
        for body in (ban_deadline, ban_active)
    ):
        errors.append("process-lifetime peer-ban leases must not read the Unix clock")

    remote_fragments = (
        "self.indexes.due_remote(cutoff,limit.get())?",
        "remote.residency.expires_at!=candidate.expires_at",
        "hashes.push(candidate.hash)",
        "self.plan_administrative_removal(hashes,AdminPlan::RemoteExpiry{cutoff})",
    )
    positions = [dense_remote_plan.find(fragment) for fragment in remote_fragments]
    if any(position < 0 for position in positions) or positions != sorted(positions):
        errors.append(
            "plan_remote_expiry must consume the exact bounded due-index cut before "
            "constructing RemoteExpiry"
        )
    accepted_fragments = (
        "self.indexes.due_accepted(cutoff,1)?.pop()",
        "entry.accepted_at!=due.accepted_at||entry.accepted_at>cutoff",
        "self.administrative_descendant_closure(&due.hash)?",
        "AdminPlan::AcceptedExpiry{root:due.hash,cutoff,}",
    )
    positions = [dense_accepted_plan.find(fragment) for fragment in accepted_fragments]
    if any(position < 0 for position in positions) or positions != sorted(positions):
        errors.append(
            "plan_accepted_expiry must consume one exact current due-index cut before "
            "constructing AcceptedExpiry"
        )

    for variant, producer_body in (
        ("RemoteExpiry", remote_plan),
        ("AcceptedExpiry", accepted_plan),
    ):
        token = f"AdminPlan::{variant}"
        outside_owned_pair = mask_rust_non_code(plan).count(token) - (
            mask_rust_non_code(producer_body).count(token)
            + mask_rust_non_code(compiler).count(token)
        )
        if outside_owned_pair != 0:
            errors.append(
                f"AdminPlan::{variant} must occur only in its indexed producer and common compiler"
            )
        compiler_occurrences = dense_compiler.count(token)
        compiler_arms = len(
            re.findall(rf"{re.escape(token)}\{{[^{{}}]*\}}=>", dense_compiler)
        )
        if compiler_occurrences != compiler_arms:
            errors.append(f"the common compiler may only consume AdminPlan::{variant}")
    return errors


def validate_compute_capability_identity() -> list[str]:
    """Keep EntryVersion as the sole numeric identity for active computation."""

    try:
        state = TX_POOL_AUTHORITY_STATE.read_text()
        work = TX_POOL_AUTHORITY_WORK.read_text()
        plan = TX_POOL_AUTHORITY_PLAN.read_text()
    except OSError as error:
        return [f"cannot inspect compute capability identity: {error}"]

    errors: list[str] = []
    production = "\n".join((state, work, plan))
    for retired in ("ComputeLeaseId", "next_lease", "StalePlan::Lease"):
        if retired in production:
            errors.append(
                f"active computation must not restore redundant numeric identity {retired}"
            )

    settlement = re.search(
        r"struct\s+SettlementToken\s*\{(?P<body>.*?)\n\}", work, re.S
    )
    if settlement is None:
        errors.append("SettlementToken declaration disappeared")
    else:
        body = settlement.group("body")
        if "version: EntryVersion" not in body:
            errors.append("SettlementToken must bind active work to EntryVersion")
        if re.search(r"\b(?:lease|generation|revision)\s*:", body):
            errors.append(
                "SettlementToken must not duplicate the owner version with another "
                "numeric identity"
            )
    return errors


def validate_ordered_chain_error_domain() -> list[str]:
    """Keep the sole ordered chain-control task free of droppable errors."""

    try:
        source = TX_POOL_AUTHORITY_SERVICE.read_text()
        masked = mask_rust_non_code(source)
    except (OSError, ValueError) as error:
        return [f"cannot inspect ordered chain error domain: {error}"]

    errors: list[str] = []
    error_enum = re.search(
        r"\benum\s+AuthorityChainUpdateError\s*\{(?P<body>.*?)\n\}",
        masked,
        re.S,
    )
    if error_enum is None:
        errors.append("ordered chain updates need a closed AuthorityChainUpdateError")
    else:
        variants = re.findall(
            r"(?m)^\s*([A-Z][A-Za-z0-9_]*)\s*(?:\([^\n]*\))?\s*,?\s*$",
            error_enum.group("body"),
        )
        if variants != ["Cancelled", "Integrity"]:
            errors.append(
                "AuthorityChainUpdateError must expose only Cancelled and Integrity, "
                f"found {variants}"
            )

    signature = re.search(
        r"fn\s+commit_chain_update\b.*?"
        r"->\s*Result\s*<\s*CommittedChainUpdate\s*,\s*AuthorityChainUpdateError\s*>",
        masked,
        re.S,
    )
    if signature is None:
        errors.append(
            "AuthorityService::commit_chain_update must return the exact committed cut in the "
            "closed chain error domain"
        )

    driver = function_body(source, "run_ordered_chain_control_driver")
    if driver is None:
        errors.append("run_ordered_chain_control_driver disappeared")
    else:
        for required in (
            "Err(AuthorityChainUpdateError::Cancelled)",
            "Err(AuthorityChainUpdateError::Integrity(invalidity))",
            "return Err(invalidity)",
        ):
            if driver.count(required) != 1:
                errors.append(
                    "ordered chain-control driver must exhaustively settle " f"{required}"
                )
        if re.search(r"Err\s*\(\s*(?:error|other|operational)\s*\)", driver):
            errors.append(
                "ordered chain-control driver must not regain a broad fallback error arm"
            )
        for required in (
            "ChainControl::Reconcile(Request {",
            "ChainControl::ClearPool(",
            "ChainControl::ClearPipeline(",
        ):
            if driver.count(required) != 1:
                errors.append(
                    "ordered chain-control driver must own exactly one " f"{required} arm"
                )

    mapping = function_body(source, "map_chain_integrity")
    if mapping is None:
        errors.append("map_chain_integrity disappeared")
    else:
        required_mappings = (
            "ChainBoundaryError::Allocation => None",
            "ChainBoundaryError::LifecycleClosed => Some(AuthorityIntegrityFault::EffectLifecycleClosed)",
            "ChainBoundaryError::CounterExhausted => Some(AuthorityIntegrityFault::from(AuthorityFault::CounterExhausted,))",
            "ChainBoundaryError::InvalidFacts | ChainBoundaryError::InvalidSnapshotEvidence => { Some(AuthorityIntegrityFault::InvalidChainEvidence) }",
            "ChainBoundaryError::Fault(fault) => Some(AuthorityIntegrityFault::from(fault))",
        )
        compact = "".join(mapping.split())
        for required in required_mappings:
            if "".join(required.split()) not in compact:
                errors.append(f"ordered chain error mapping lost {required}")
    return errors


def validate_production_vocabulary() -> list[str]:
    """Reject migration-era names from the surviving production model."""

    retired = (
        "G5.3c",
        "P9.7g",
        "production cutover",
        "cutover facade",
        "compute lease",
        "versioned lease",
        "version/phase/lease",
        "legacy service journal",
        "legacy in-task deferral",
    )
    errors: list[str] = []
    root = REPO_ROOT / "tx-pool" / "src"
    for source in sorted(root.rglob("*.rs")):
        if "tests" in source.parts:
            continue
        text = source.read_text()
        for term in retired:
            if term in text:
                errors.append(
                    f"retired production vocabulary {term!r} in "
                    f"{source.relative_to(REPO_ROOT)}"
                )
    return errors


def infallible_scratch_sites(source: str) -> list[tuple[str, int]]:
    """Return production scratch syntax that cannot report allocation failure."""

    masked = mask_rust_non_code(source)
    sites: list[tuple[str, int]] = []
    for label, pattern in INFALLIBLE_SCRATCH_CONSTRUCTION:
        sites.extend((label, match.start()) for match in pattern.finditer(masked))
    return sorted(sites, key=lambda site: site[1])


def validate_fallible_scratch_construction() -> list[str]:
    """Keep variable production scratch explicit, fallible and auditable."""

    canary = """
        let _ = input.iter().collect::<Vec<_>>();
        let _ = input.iter().collect();
        let _ = vec![item];
        let _ = Vec::with_capacity(input.len());
        let _ = ".collect(), vec![ignored] and Vec::with_capacity(7)";
        // let _ = input.iter().collect::<Vec<_>>();
    """
    try:
        canary_labels = [label for label, _ in infallible_scratch_sites(canary)]
    except ValueError as error:
        return [f"fallible-scratch negative canary could not be parsed: {error}"]
    if canary_labels != [
        "iterator collect",
        "iterator collect",
        "vec! construction",
        "infallible capacity construction",
    ]:
        return [
            "fallible-scratch negative canary no longer detects collect, vec! and infallible "
            f"capacity construction while masking comments/literals: {canary_labels}"
        ]

    roots = (TX_POOL_SRC / "authority", TX_POOL_SRC / "service")
    sources = {
        TX_POOL_SRC / "service.rs",
        TX_POOL_SRC / "util.rs",
        TX_POOL_CANDIDATE_UNCLES,
    }
    for root in roots:
        sources.update(
            path for path in root.rglob("*.rs") if "tests" not in path.parts
        )

    errors: list[str] = []
    for path in sorted(sources):
        try:
            source = path.read_text()
            sites = infallible_scratch_sites(source)
        except (OSError, ValueError) as error:
            errors.append(
                f"cannot inspect fallible scratch in {path.relative_to(REPO_ROOT)}: {error}"
            )
            continue
        for label, offset in sites:
            line = source.count("\n", 0, offset) + 1
            errors.append(
                f"infallible {label} at {path.relative_to(REPO_ROOT)}:{line}; "
                "reserve the bounded carrier/scratch explicitly and handle its terminal "
                "allocation outcome before consuming input"
            )
    return errors


def validate_shared_variable_residency() -> list[str]:
    """Keep hostile-sized materialization fallible and shared without recopying."""

    try:
        util = mask_rust_non_code(TX_POOL_UTIL.read_text())
        residency = mask_rust_non_code(TX_POOL_AUTHORITY_RESIDENCY.read_text())
        resolver = mask_rust_non_code(TX_POOL_AUTHORITY_RESOLVER.read_text())
        state = mask_rust_non_code(TX_POOL_AUTHORITY_STATE.read_text())
        chain = mask_rust_non_code(TX_POOL_AUTHORITY_CHAIN.read_text())
        validation = mask_rust_non_code(TX_POOL_AUTHORITY_VALIDATION.read_text())
    except (OSError, ValueError) as error:
        return [f"cannot inspect shared variable residency: {error}"]

    errors: list[str] = []

    def compact(source: str) -> str:
        return "".join(source.split())

    util_compact = compact(util)
    marker_impls = sorted(
        re.findall(r"\bimpl\s+FixedSizePackedEntity\s+for\s+([A-Za-z0-9_]+)", util)
    )
    if marker_impls != ["Byte32", "OutPoint", "ProposalShortId"]:
        errors.append(
            "FixedSizePackedEntity must remain the exact audited fixed-length Molecule set, "
            f"found {marker_impls}"
        )
    if "fncompact_packed<T:FixedSizePackedEntity>" not in util_compact:
        errors.append(
            "compact_packed must reject variable-length Molecule entities at compile time"
        )
    for fragment in (
        "fntry_compact_packed<T:Entity>",
        "owned.try_reserve_exact(value.as_slice().len())?;",
        "fntry_compact_bytes(value:&Bytes)",
        "owned.try_reserve_exact(value.len())?;",
    ):
        if fragment not in util_compact:
            errors.append(f"fallible residency helper lost {fragment!r}")

    residency_compact = compact(residency)
    resolution_fragments = (
        "mutresolved:ResolvedTransaction",
        "try_compact_packed(&cell.cell_output)",
        "try_compact_packed(&cell.out_point)",
        "try_compact_packed(&info.block_hash)",
        "try_compact_bytes(data)",
        "try_compact_packed(hash)",
        "compact_cell(cell)?;",
        "Ok(Arc::new(resolved))",
    )
    positions = [residency_compact.find(fragment) for fragment in resolution_fragments]
    if any(position < 0 for position in positions) or positions != sorted(positions):
        errors.append(
            "compact_after_resolution must detach the owned hostile payload through one "
            "fallible ordered materialization boundary"
        )
    refresh_fragments = (
        "fntry_clone_for_location_refresh(",
        "try_reserve_exact(cells.len())",
        "cloned.extend(cells.iter().cloned());",
        "resolved_cell_deps:try_clone_cells(&resolved.resolved_cell_deps)?",
        "resolved_inputs:try_clone_cells(&resolved.resolved_inputs)?",
        "resolved_dep_groups:try_clone_cells(&resolved.resolved_dep_groups)?",
    )
    positions = [residency_compact.find(fragment) for fragment in refresh_fragments]
    if any(position < 0 for position in positions) or positions != sorted(positions):
        errors.append(
            "location refresh must clone all variable cell vectors only after fallible exact "
            "reservation"
        )
    verified_fragments = (
        "fncompact_after_verification(",
        "matchArc::try_unwrap(resolved)",
        "Err(shared)=>returnshared",
        "std::mem::take(&mutcell.out_point)",
        "cell.transaction_info.take()",
    )
    positions = [residency_compact.find(fragment) for fragment in verified_fragments]
    if any(position < 0 for position in positions) or positions != sorted(positions):
        errors.append(
            "verified residency must compact uniquely owned cells by move and preserve a shared "
            "bounded representation without cloning"
        )
    if "Bytes::copy_from_slice" in residency or "(*shared).clone()" in residency_compact:
        errors.append("residency reintroduced an infallible hostile-sized copy")

    state_compact = compact(state)
    for fragment in (
        "structKnownDependencies(Arc<Vec<DependencyKey>>);",
        "Ok(Self(Arc::new(keys)))",
        "Ok(Arc::new(parents))",
        "pub(super)footprint:Arc<ExpandedFootprint>",
        "footprint:Arc::clone(&self.footprint)",
        "Err(shared)=>{letaccepted_resident_bytes=accepted_transaction_charge_bytes(",
        "return(shared,accepted_resident_bytes);",
    ):
        if fragment not in state_compact:
            errors.append(f"shared variable residency lost representation law {fragment!r}")
    if re.search(r"#\s*\[\s*derive\([^]]*Clone[^]]*\)\s*\]\s*pub\(super\)\s+struct\s+ExpandedFootprint", state):
        errors.append("ExpandedFootprint may not expose an infallible variable deep clone")

    chain_compact = compact(chain)
    for fragment in (
        "chain_inputs:Arc<Vec<OutPoint>>",
        "chain_dependencies:Arc<Vec<OutPoint>>",
        "chain_inputs:Arc::new(chain_inputs)",
        "chain_dependencies:Arc::new(chain_dependencies)",
    ):
        if fragment not in chain_compact:
            errors.append(f"chain evidence lost pre-reserved Vec sharing {fragment!r}")

    production_paths = [
        path
        for path in sorted((TX_POOL_SRC / "authority").rglob("*.rs"))
        if "tests" not in path.parts
    ]
    for path in production_paths:
        try:
            source = mask_rust_non_code(path.read_text())
        except (OSError, ValueError) as error:
            errors.append(
                f"cannot inspect shared representation in {path.relative_to(REPO_ROOT)}: {error}"
            )
            continue
        if re.search(r"\bArc\s*<\s*\[", source):
            errors.append(
                f"{path.relative_to(REPO_ROOT)} reintroduced Arc<[T]>; preserve the fallibly "
                "allocated Vec backing behind Arc<Vec<T>>"
            )

    resolver_compact = compact(resolver)
    for fragment in (
        "super::residency::compact_after_resolution(resolved)",
        ".map_err(|_|FinishResolutionError::ResourceUnavailable)?;",
        "Err(FinishResolutionError::ResourceUnavailable)=>{returnErr(ResolutionExecutionFailure",
        "Err(FinishResolutionError::ResourceUnavailable)=>{returnErr(DirectComputationError::ResourceUnavailable);}",
    ):
        if fragment not in resolver_compact:
            errors.append(f"resolution allocation terminal lost {fragment!r}")

    validation_compact = compact(validation)
    if (
        "super::residency::try_clone_for_location_refresh(resolved)"
        ".map_err(|_|FinalAdmissionValidationError::Allocation)?;"
        not in validation_compact
        or "resolved.as_ref().clone()" in validation_compact
    ):
        errors.append(
            "final location refresh must use the fallible resolved materialization boundary"
        )
    return errors


def validate_bounded_external_residency() -> list[str]:
    """Bind every variable service/chain/template payload to one sealed bound."""

    try:
        ingress = TX_POOL_AUTHORITY_INGRESS.read_text()
        message = TX_POOL_MESSAGE.read_text()
        controller = TX_POOL_CONTROLLER.read_text()
        service = TX_POOL_SERVICE.read_text()
        builder = TX_POOL_BUILDER.read_text()
        candidate = TX_POOL_CANDIDATE_UNCLES.read_text()
        assembler = TX_POOL_BLOCK_ASSEMBLER.read_text()
        effect = TX_POOL_AUTHORITY_EFFECT.read_text()
        ingress_methods = {
            name: body
            for name, body, _line in rust_impl_methods(ingress, "BoundedTransaction")
        }
        proposal_methods = {
            name: body
            for name, body, _line in rust_impl_methods(message, "BoundedProposalIds")
        }
        transaction_hash_methods = {
            name: body
            for name, body, _line in rust_impl_methods(
                message, "BoundedTransactionHashes"
            )
        }
        batch_methods = {
            name: body
            for name, body, _line in rust_impl_methods(message, "NotifyTxBatch")
        }
        candidate_methods = {
            name: body
            for name, body, _line in rust_impl_methods(candidate, "BoundedCandidateUncle")
        }
        candidate_cache_methods = {
            name: body
            for name, body, _line in rust_impl_methods(candidate, "CandidateUncles")
        }
        candidate_snapshot_methods = {
            name: body
            for name, body, _line in rust_impl_methods(candidate, "CandidateUncleSnapshot")
        }
        assembler_methods = {
            name: body
            for name, body, _line in rust_impl_methods(assembler, "BlockAssembler")
        }
        reorg_limit_methods = {
            name: body
            for name, body, _line in rust_impl_methods(service, "ChainReorgPayloadLimit")
        }
        reorg_methods = {
            name: body
            for name, body, _line in rust_impl_methods(service, "ChainReorgArgs")
        }
    except (OSError, ValueError) as error:
        return [f"cannot inspect bounded external residency: {error}"]

    errors: list[str] = []

    def compact(body: str) -> str:
        return "".join(mask_rust_non_code(body).split())

    def require_sequence(owner: str, body: str, fragments: tuple[str, ...]) -> None:
        errors.extend(require_ordered_fragments(compact(body), owner, fragments))

    require_sequence(
        "BoundedTransaction::try_new",
        ingress_methods.get("try_new", ""),
        (
            "serialized_size_in_block()",
            "u64::try_from(serialized_bytes)",
            "ifserialized_bytes_u64>TRANSACTION_SIZE_LIMIT",
            ".try_into_compact()",
            "Arc::new(transaction)",
        ),
    )
    require_sequence(
        "BoundedProposalIds::try_from_iter_with_limit",
        proposal_methods.get("try_from_iter_with_limit", ""),
        (
            "letactual=ids.len();",
            "ifactual>maximum",
            "try_compact_proposal_ids(ids)",
            "Ok(Self(normalized))",
        ),
    )
    require_sequence(
        "BoundedTransactionHashes::try_from_iter_with_limit",
        transaction_hash_methods.get("try_from_iter_with_limit", ""),
        (
            "letactual=hashes.len();",
            "ifactual>maximum",
            "try_compact_transaction_hashes(hashes)",
            "normalized.sort_unstable();",
            "Ok(Self(normalized))",
        ),
    )
    require_sequence(
        "NotifyTxBatch::try_new_with_limits",
        batch_methods.get("try_new_with_limits", ""),
        (
            "iftxs.len()>max_count",
            ".checked_add(tx.data().total_size())",
            "ifbytes>max_bytes",
            ".try_reserve_exact(txs.len())",
            "BoundedTransaction::try_new(tx)",
        ),
    )
    require_sequence(
        "BoundedCandidateUncle::try_new",
        candidate_methods.get("try_new", ""),
        (
            ".total_size()",
            ".checked_add(uncle.hash().as_slice().len())",
            "ifactual>maximum",
            ".try_into_compact()",
        ),
    )
    require_sequence(
        "CandidateUncles::try_snapshot",
        candidate_cache_methods.get("try_snapshot", ""),
        (
            "letmutcandidates=Vec::new();",
            ".try_reserve_exact(self.len())",
            "candidates.extend(self.values().cloned());",
            "source:self.source_receipt()",
        ),
    )
    snapshot_prepare = compact(candidate_snapshot_methods.get("prepare_uncles", ""))
    for fragment in (
        "max_uncles_num.min(self.candidates.len())",
        "removed.try_reserve_exact(self.candidates.len())",
    ):
        if fragment not in snapshot_prepare:
            errors.append(
                "CandidateUncleSnapshot::prepare_uncles must bound both fallible scratch "
                f"collections by the captured candidate population; missing {fragment!r}"
            )
    live_prepare = compact(assembler_methods.get("prepare_uncles", ""))
    if (
        "self.candidate_uncles.lock().try_snapshot()?" not in live_prepare
        or "candidates.prepare_uncles(snapshot,current_epoch)" not in live_prepare
        or "self.candidate_uncles.lock().clone()" in live_prepare
    ):
        errors.append(
            "BlockAssembler::prepare_uncles must fallibly capture the bounded value/source "
            "snapshot under the cache lock and select only after releasing mutation authority"
        )

    message_body = rust_type_body(message, "enum", "Message")
    if message_body is None:
        errors.append("service Message enum disappeared")
    else:
        message_compact = compact(message_body)
        for fragment in (
            "SubmitLocalTx(SyncRequest<BoundedTransaction,SubmitTxResult>)",
            "TestAcceptTx(SyncRequest<BoundedTransaction,TestAcceptTxResult>)",
            "SubmitRemoteTx(AsyncRequest<RemoteTxSubmission,()>)",
            "NotifyTxs(Notify<NotifyTxBatch>)",
            "FreshProposalsFilter(AsyncRequest<BoundedProposalIds,Vec<ProposalShortId>>)",
            "FetchTxs(AsyncRequest<BoundedProposalIds,HashMap<ProposalShortId,TransactionView>>)",
            "FetchTxsWithCycles(AsyncRequest<BoundedTransactionHashes,FetchTxsWithCyclesResult>)",
            "NewUncle(Notify<BoundedCandidateUncle>)",
        ):
            if fragment not in message_compact:
                errors.append(
                    f"ordinary service payload lost sealed residency carrier {fragment!r}"
                )

    reorg_limit = compact(reorg_limit_methods.get("from_config", ""))
    if (
        ".checked_add(config.tx_pipeline_resident_size_budget())" not in reorg_limit
        or "saturating_add" in reorg_limit
    ):
        errors.append(
            "ChainReorgPayloadLimit::from_config must reject an unrepresentable combined "
            "accepted/pre-pool residency bound"
        )
    require_sequence(
        "ChainReorgArgs::bounded",
        reorg_methods.get("bounded", ""),
        (
            ".checked_add(std::mem::size_of::<BlockView>())",
            ".checked_add(block.data().total_size())",
            "ifcharge.is_none_or(|charge|charge>limit.0)",
            "normalized_detached.try_reserve_exact(detached_blocks.len())",
            "normalized_attached.try_reserve_exact(attached_blocks.len())",
            "Self::Detailed",
        ),
    )
    if "detached_proposal" in compact(reorg_methods.get("bounded", "")):
        errors.append(
            "ChainReorgArgs::bounded must not retain a caller-derived proposal subset"
        )
    builder_compact = compact(builder)
    for fragment in (
        ".checked_add(Byte32::default().as_slice().len())",
        "ChainReorgPayloadLimit::from_config(&tx_pool_config).ok_or_else",
    ):
        if fragment not in builder_compact:
            errors.append(f"service startup lost checked residency fragment {fragment!r}")

    effect_body = rust_type_body(effect, "struct", "EffectBatch")
    if effect_body is None or "effects:Vec<CommittedEffect>" not in compact(effect_body):
        errors.append("EffectBatch must retain the compiler's already reserved Vec allocation")
    if "Box<[CommittedEffect]>" in effect or ".into_boxed_slice()" in effect:
        errors.append("EffectBatch may not add a second boxed-slice allocation")

    compact_sites: list[tuple[str, int]] = []
    for path in sorted(TX_POOL_SRC.rglob("*.rs")):
        if "tests" in path.parts:
            continue
        masked = mask_rust_non_code(path.read_text())
        count = len(re.findall(r"\.\s*try_into_compact\s*\(", masked))
        if count:
            compact_sites.append((path.relative_to(TX_POOL_SRC).as_posix(), count))
    expected_compact_sites = [
        ("authority/ingress.rs", 1),
        ("block_assembler/candidate_uncles.rs", 1),
    ]
    if compact_sites != expected_compact_sites:
        errors.append(
            "external payload compaction must have exactly the transaction and candidate-uncle "
            f"sealed owners, found {compact_sites}"
        )
    return errors


def validate_relay_full_hash_query_identity() -> list[str]:
    """Keep relay lookup on complete raw identity across every boundary."""

    try:
        query = TX_POOL_AUTHORITY_QUERY.read_text()
        runtime = TX_POOL_AUTHORITY_RUNTIME.read_text()
        authority_service = TX_POOL_AUTHORITY_SERVICE.read_text()
        message = TX_POOL_MESSAGE.read_text()
        controller = TX_POOL_CONTROLLER.read_text()
        dispatch = TX_POOL_DISPATCH.read_text()
        sync = SYNC_GET_TRANSACTIONS.read_text()
    except OSError as error:
        return [f"cannot inspect relay full-hash query identity: {error}"]

    def compact(source: str) -> str:
        return "".join(mask_rust_non_code(source).split())

    def query_errors(source: str) -> list[str]:
        observed: list[str] = []
        body = function_body(source, "accepted_with_cycles")
        if body is None:
            return ["authority accepted_with_cycles query disappeared"]
        body_compact = compact(body)
        if "view.entry_by_raw(&RawTxHash(hash.clone()))" not in body_compact:
            observed.append("relay authority query lost exact RawTxHash owner lookup")
        for forbidden in ("entry_by_proposal", "ProposalId(", "ProposalShortId"):
            if forbidden in body_compact:
                observed.append(
                    f"relay authority query regained short-ID projection {forbidden!r}"
                )
        if re.search(
            r"fn\s+accepted_with_cycles\s*\(\s*view\s*:\s*&AuthorityReadView<'_>\s*,"
            r"\s*requested\s*:\s*&\[Byte32\]",
            mask_rust_non_code(source),
            re.S,
        ) is None:
            observed.append("relay authority query input is not a Byte32 slice")
        if (
            "typeAcceptedTransactionsWithCycles=Vec<(TransactionView,Cycle)>;"
            not in compact(source)
        ):
            observed.append(
                "relay authority result must carry transaction/cycle without a short-ID key"
            )
        return observed

    def sync_errors(source: str) -> list[str]:
        observed: list[str] = []
        body = function_body(source, "execute")
        if body is None:
            return ["GetTransactionsProcess::execute disappeared"]
        body_compact = compact(body)
        for required in (
            ".map(|tx_hash|tx_hash.to_entity())",
            "fetch_txs_with_cycles(tx_hashes_set).await",
            ".map(|(tx,cycles)|",
        ):
            if required not in body_compact:
                observed.append(f"relay wire lookup lost full-hash fragment {required!r}")
        if "ProposalShortId::from_tx_hash" in body_compact:
            observed.append("relay wire lookup narrows a raw hash to ProposalShortId")
        return observed

    errors = [*query_errors(query), *sync_errors(sync)]
    surfaces = {
        "controller": compact(controller),
        "runtime": compact(runtime),
        "authority service": compact(authority_service),
        "message": compact(message),
        "dispatch": compact(dispatch),
    }
    required = {
        "controller": (
            "pubasyncfnfetch_txs_with_cycles(&self,tx_hashes:HashSet<Byte32>,)",
            "BoundedTransactionHashes::try_from_set(tx_hashes)?",
        ),
        "runtime": (
            "pub(crate)fnaccepted_with_cycles(&self,mutrequested:Vec<ckb_types::packed::Byte32>,)",
            "super::query::accepted_with_cycles(&store.authority.read_view(),&requested)",
        ),
        "authority service": (
            "pub(crate)fnaccepted_with_cycles(&self,tx_hashes:Vec<ckb_types::packed::Byte32>,)",
            ".accepted_with_cycles(tx_hashes)",
        ),
        "message": (
            "structBoundedTransactionHashes(Vec<Byte32>);",
            "FetchTxsWithCycles(AsyncRequest<BoundedTransactionHashes,FetchTxsWithCyclesResult>)",
        ),
        "dispatch": (
            "Message::FetchTxsWithCycles(request)=>",
            "service.accepted_with_cycles(arguments.into_vec())",
        ),
    }
    for surface, fragments in required.items():
        for fragment in fragments:
            if fragment not in surfaces[surface]:
                errors.append(
                    f"relay full-hash {surface} refinement lost {fragment!r}"
                )

    query_canary = query.replace(
        "view.entry_by_raw(&RawTxHash(hash.clone()))",
        "view.entry_by_proposal(&super::state::ProposalId::from(hash.clone()))",
        1,
    )
    if query_canary == query or not any(
        "exact RawTxHash owner lookup" in error for error in query_errors(query_canary)
    ):
        errors.append("relay identity canary admitted proposal projection in authority query")
    sync_canary = sync.replace(
        ".map(|tx_hash| tx_hash.to_entity())",
        ".map(|tx_hash| packed::ProposalShortId::from_tx_hash(&tx_hash.to_entity()))",
        1,
    )
    if sync_canary == sync or not any(
        "narrows a raw hash" in error for error in sync_errors(sync_canary)
    ):
        errors.append("relay identity canary admitted wire-level short-ID narrowing")
    return errors


def validate_allocation_progress_protocol() -> list[str]:
    """Require allocation outcomes to retire work or await a monotonic releaser."""

    try:
        template = TX_POOL_AUTHORITY_TEMPLATE_DRIVER.read_text()
        worker = TX_POOL_AUTHORITY_WORKER.read_text()
        coordinator = TX_POOL_AUTHORITY_COMPUTE_COORDINATOR.read_text()
        service = TX_POOL_AUTHORITY_SERVICE.read_text()
        runtime = TX_POOL_AUTHORITY_RUNTIME.read_text()
        plan = TX_POOL_AUTHORITY_PLAN.read_text()
        chain_boundary = TX_POOL_AUTHORITY_CHAIN_BOUNDARY.read_text()
        template_methods = {
            name: body
            for name, body, _line in rust_impl_methods(
                template, "AuthorityBlockAssembler"
            )
        }
        coordinator_methods = {
            name: body
            for name, body, _line in rust_impl_methods(coordinator, "ComputeCoordinator")
        }
        service_methods = {
            name: body
            for name, body, _line in rust_impl_methods(service, "AuthorityService")
        }
        runtime_methods = {
            name: body
            for name, body, _line in rust_impl_methods(runtime, "AuthorityRuntime")
        }
        aftermath_methods = {
            name: body
            for name, body, _line in rust_impl_methods(
                runtime, "AuthorityComputeAftermath"
            )
        }
        plan_methods = {
            name: body
            for name, body, _line in rust_impl_methods(
                plan, "TxPoolAuthority", allow_multiple=True
            )
        }
        fresh_generation_methods = {
            name: body
            for name, body, _line in rust_impl_methods(plan, "FreshGeneration")
        }
        settlement_recovery_methods = {
            name: body
            for name, body, _line in rust_impl_methods(
                plan, "ComputeSettlementRecovery"
            )
        }
        settlement_failure_methods = {
            name: body
            for name, body, _line in rust_impl_methods(plan, "ComputeSettlementFailure")
        }
        request_methods = {
            name: body
            for name, body, _line in rust_impl_methods(
                chain_boundary, "ChainUpdateRequest"
            )
        }
        command_methods = {
            name: body
            for name, body, _line in rust_impl_methods(
                chain_boundary, "ChainUpdateCommand"
            )
        }
    except (OSError, ValueError) as error:
        return [f"cannot inspect allocation progress protocol: {error}"]

    errors: list[str] = []
    production = mask_rust_non_code(
        "\n".join(
            path.read_text()
            for path in sorted(TX_POOL_SRC.rglob("*.rs"))
            if "tests" not in path.parts
        )
    )
    for retired in RETIRED_ALLOCATION_RETRY_VOCABULARY:
        if retired in production:
            errors.append(
                f"allocation timer/retry vocabulary {retired!r} reintroduced into production"
            )

    for method, attempt in (
        ("run_replacement_lane", "attempt_replacement_once"),
        ("run_component_lane", "attempt_component_once"),
    ):
        compact = "".join(mask_rust_non_code(template_methods.get(method, "")).split())
        ordered = (
            f"matchself.{attempt}",
            "Err(FailedTemplateAttempt{source,error})",
            "self.next_template_source_after_failure(&cancel,source).await.is_none()",
        )
        positions = [compact.find(fragment) for fragment in ordered]
        if any(position < 0 for position in positions) or positions != sorted(positions):
            errors.append(
                f"AuthorityBlockAssembler::{method} must retain the attempt's fused monotonic "
                "cut and classify every failure through its sole source-advance gate"
            )
        if "TemplateReadError::Allocation" in compact or "tokio::time::sleep" in compact:
            errors.append(
                f"AuthorityBlockAssembler::{method} may not special-case allocation or time"
            )

    for method, capture, drive in (
        (
            "attempt_replacement_once",
            "replacement_attempt",
            "drive_replacement_attempt",
        ),
        ("attempt_component_once", "component_attempt", "drive_component_attempt"),
    ):
        compact = "".join(mask_rust_non_code(template_methods.get(method, "")).split())
        ordered = (
            f"letSome(attempt)=self.{capture}",
            "letsource=attempt.source;",
            f"self.{drive}",
            ".map_err(|error|FailedTemplateAttempt{source,error})",
        )
        positions = [compact.find(fragment) for fragment in ordered]
        if any(position < 0 for position in positions) or positions != sorted(positions):
            errors.append(
                f"AuthorityBlockAssembler::{method} must capture one gate/base/failure-cut "
                "attempt before construction and preserve that exact cut on failure"
            )

    source_gate = "".join(
        mask_rust_non_code(
            template_methods.get("next_template_source_after_failure", "")
        ).split()
    )
    source_fragments = (
        "letobserved=self.observed_retry_source(attempted).await;",
        "ifobserved!=attempted{returnSome(observed);}",
        "self.wait_template_source_change(cancel,observed).await",
    )
    positions = [source_gate.find(fragment) for fragment in source_fragments]
    if any(position < 0 for position in positions) or positions != sorted(positions):
        errors.append(
            "template failure progress must be exactly pre/post monotonic-cut validation "
            "followed by source-change wait"
        )

    for method in ("replacement_attempt", "component_attempt"):
        compact = "".join(mask_rust_non_code(template_methods.get(method, "")).split())
        if "template_input(" in compact:
            errors.append(
                f"AuthorityBlockAssembler::{method} must remain an O(1) gate/base/cut probe"
            )
    if "retry_source_cut(" in production:
        errors.append(
            "the retired all-source pre-attempt template retry cut was reintroduced"
        )

    settlement_recovery = "".join(
        mask_rust_non_code(settlement_recovery_methods.get("from_plan", "")).split()
    )
    for fragment in (
        "PlanError::Backpressure(Backpressure::Allocation)=>Self::CancelAfterAllocation",
        "PlanError::Backpressure(Backpressure::EffectCapacity)=>Self::WaitEffectCapacity",
    ):
        if settlement_recovery.count(fragment) != 1:
            errors.append(
                "ComputeSettlementRecovery::from_plan must keep allocation terminal and "
                f"effect-capacity wait disjoint: missing {fragment!r}"
            )
    if settlement_recovery.count("Self::WaitEffectCapacity") != 1:
        errors.append(
            "compute settlement may wait only on the one named effect-capacity releaser"
        )

    discard_result = "".join(
        mask_rust_non_code(
            settlement_failure_methods.get("discard_result_for_cancellation", "")
        ).split()
    )
    discard_fragments = (
        "letSelf{token,next,recovery:_,}=self;",
        "drop(next);",
        "ComputeCancellation{token}",
    )
    positions = [discard_result.find(fragment) for fragment in discard_fragments]
    if any(position < 0 for position in positions) or positions != sorted(positions):
        errors.append(
            "allocation cancellation must discard the expensive result while preserving the "
            "exact move-only settlement token"
        )

    compute_cancellation = "".join(
        mask_rust_non_code(plan_methods.get("apply_compute_cancellation", "")).split()
    )
    cancellation_fragments = (
        "self.entries.get(&token.hash)",
        "PreAcceptedPhase::Computing(_)",
        "preaccepted.charge.active_work!=1",
        "PreAcceptedPhase::Queued(QueuedWork::Resolve)",
        "self.resources.plan_compute_release",
        "self.scheduler.plan_replace",
        "self.dependencies.plan_stable_replace",
        "self.indexes.plan_stable_replace",
        "self.source_versions.plan_replacements",
        "EntryRetirement::InlineDrop",
        ".apply())",
    )
    positions = [compute_cancellation.find(fragment) for fragment in cancellation_fragments]
    if any(position < 0 for position in positions) or positions != sorted(positions):
        errors.append(
            "TxPoolAuthority::apply_compute_cancellation must consume one exact Computing "
            "capability into the allocation-free Queued transition and every derived owner"
        )
    if any(
        forbidden in compute_cancellation
        for forbidden in ("try_reserve", ".collect(", "Vec::with_capacity", "vec![")
    ):
        errors.append(
            "compute cancellation may not acquire population scratch after allocation failure"
        )

    worker_allocation = "".join(
        mask_rust_non_code(function_body(worker, "classify_driver_error") or "").split()
    )
    if (
        "AuthorityDriverError::Allocation=>{runtime."
        "replace_current_generation_after_allocation()" not in worker_allocation
    ):
        errors.append("maintenance allocation must replace the current authority generation")

    coordinator_new = "".join(
        mask_rust_non_code(coordinator_methods.get("new", "")).split()
    )
    for fragment in (
        "letbound=lanes.len();",
        "forbufferin[&mutexchange_pending,&mutexact_pending,&mutexchange_after_effect,&mutexact_after_effect,]",
        "buffer.try_reserve(bound)",
        "eligible_slots.try_reserve(bound)",
    ):
        if fragment not in coordinator_new:
            errors.append(
                "ComputeCoordinator::new must preallocate every completion/recovery buffer to "
                f"the exact worker-slot bound: missing {fragment!r}"
            )

    collect_grants = "".join(
        mask_rust_non_code(
            coordinator_methods.get("collect_immediate_grants", "")
        ).split()
    )
    collect_failure = (
        "ifgrants.try_reserve(bound.saturating_sub(grants.len())).is_err()"
        "{self.seed_grant=None;drop(grants);"
        "self.replace_generation_after_allocation()?;returnOk(Vec::new());}"
    )
    if collect_failure not in collect_grants:
        errors.append(
            "compute-grant scratch failure must release every grant and replace the generation "
            "without retrying the unchanged cut"
        )

    drive_exchange = "".join(
        mask_rust_non_code(coordinator_methods.get("drive_exchange", "")).split()
    )
    drive_failure = (
        "ifreplacement.try_reserve(self.lanes.len()).is_err()"
        "{drop(grants);self.replace_generation_after_allocation()?;returnOk(());}"
    )
    if drive_failure not in drive_exchange:
        errors.append(
            "compute exchange-buffer allocation must release unconsumed grants and converge "
            "through the generation terminal"
        )

    recover_exchange = "".join(
        mask_rust_non_code(
            coordinator_methods.get("recover_exchange_failure", "")
        ).split()
    )
    exchange_allocation_branches = (
        "AuthorityComputeExchangeFailure::Allocation{completions,grants,}=>{"
        "drop(grants);self.exact_pending.extend(completions);"
        "self.replace_generation_after_allocation()?;Ok(())}",
        "PlanError::Backpressure(Backpressure::Allocation)=>{"
        "let(_,recoveries)=failure.into_recovery();"
        "letresult=self.recover_plan_capabilities(recoveries,RecoveryRoute::Exact);"
        "result?;self.replace_generation_after_allocation()}",
        "PlanError::Backpressure(Backpressure::EffectCapacity)=>{"
        "let(_,recoveries)=failure.into_recovery();"
        "self.recover_plan_capabilities(recoveries,RecoveryRoute::AfterEffect)}",
    )
    if any(branch not in recover_exchange for branch in exchange_allocation_branches):
        errors.append(
            "compute exchange recovery must return exact completions, release grants, use the "
            "generation terminal for allocation and reserve waiting only for effect capacity"
        )

    drive_exact = "".join(
        mask_rust_non_code(coordinator_methods.get("drive_exact", "")).split()
    )
    exact_allocation_branch = (
        "ComputeSettlementRecovery::CancelAfterAllocation=>{"
        "letcancellation=failure.discard_result_for_cancellation();"
        "letresult=self.runtime.cancel_compute_after_allocation(cancellation);"
        "self.mark_idle(slot)?;replace_generation|="
        "self.consume_cancellation(result,aftermath)?;}"
    )
    if (
        exact_allocation_branch not in drive_exact
        or "ifreplace_generation{self.replace_generation_after_allocation()?;}"
        not in drive_exact
    ):
        errors.append(
            "exact compute settlement must cancel the one capability, retire its slot and only "
            "then replace the generation after allocation"
        )

    consume_cancellation = "".join(
        mask_rust_non_code(
            coordinator_methods.get("consume_cancellation", "")
        ).split()
    )
    if (
        "Ok(())=>matchaftermath.disposition(){"
        "AuthorityComputeAftermathDisposition::Progress|"
        "AuthorityComputeAftermathDisposition::ReplaceGeneration=>Ok(true)"
        not in consume_cancellation
    ):
        errors.append(
            "a successfully consumed allocation cancellation must select exactly one later "
            "generation replacement"
        )

    coordinator_replacement = "".join(
        mask_rust_non_code(
            coordinator_methods.get("replace_generation_after_allocation", "")
        ).split()
    )
    if "self.runtime.replace_current_generation_after_allocation()" not in coordinator_replacement:
        errors.append("compute allocation must converge through the same generation replacement")
    replacement_order = (
        "lettotal_finished=self.exact_pending.len().checked_add(self.exchange_pending.len())",
        "total_finished>self.lanes.len()||total_finished>self.exact_pending.capacity()",
        "self.runtime.replace_current_generation_after_allocation()",
        "self.exact_pending.append(&mutself.exchange_pending);",
        "self.exact_pending.append(&mutself.exchange_after_effect);",
        "self.exact_pending.append(&mutself.exact_after_effect);",
        "self.restart_probe_cycle();",
    )
    positions = [coordinator_replacement.find(fragment) for fragment in replacement_order]
    if any(position < 0 for position in positions) or positions != sorted(positions):
        errors.append(
            "compute allocation replacement must prove fixed buffer capacity, advance the "
            "generation, and preserve every completion for stale retirement in that order"
        )

    runtime_terminal = "".join(
        mask_rust_non_code(
            runtime_methods.get("replace_current_generation_after_allocation", "")
        ).split()
    )
    if runtime_terminal != "self.replace_generation(None)":
        errors.append(
            "the retained-allocation terminal must be exactly the current paired generation "
            "replacement"
        )
    terminal_calls = len(
        re.findall(r"\.replace_current_generation_after_allocation\s*\(", production)
    )
    if terminal_calls != 2:
        errors.append(
            "current-generation allocation replacement must have exactly the maintenance and "
            f"compute coordinator callers, found {terminal_calls}"
        )

    replacement = "".join(
        mask_rust_non_code(runtime_methods.get("replace_generation", "")).split()
    )
    replacement_fragments = (
        "letmutstore=self.store.write();",
        "store.authority.plan_clear_pool(tip_hash)",
        ".apply();",
        "self.signals.publish_post_commit(post_commit);",
    )
    positions = [replacement.find(fragment) for fragment in replacement_fragments]
    if any(position < 0 for position in positions) or positions != sorted(positions):
        errors.append(
            "generation replacement must bind the current/exact snapshot, apply one clear-pool "
            "transition and publish only after the authority cut"
        )

    clear_pool = "".join(
        mask_rust_non_code(plan_methods.get("plan_clear_pool", "")).split()
    )
    clear_fragments = (
        "letchain_view=ChainViewId::new(next_chain_revision(self.chain_revision())?,tip_hash);",
        "leteffect=self.effects.plan_generation_reset(sequence)?;",
        "letsources=self.source_versions.plan_generation_replacement(sequence);",
        "letfresh=FreshGeneration::empty(&self.resources,&self.scheduler);",
    )
    positions = [clear_pool.find(fragment) for fragment in clear_fragments]
    if any(position < 0 for position in positions) or positions != sorted(positions):
        errors.append(
            "plan_clear_pool lost its checked clocks, prebuilt reset, source replacement or "
            "fresh-generation construction"
        )
    if any(
        forbidden in clear_pool
        for forbidden in ("try_reserve", ".collect(", "vec![", "Vec::with_capacity")
    ):
        errors.append("plan_clear_pool reintroduced fallible or population-sized scratch")

    fresh = "".join(
        mask_rust_non_code(fresh_generation_methods.get("empty", "")).split()
    )
    for fragment in (
        "entries:HashMap::new()",
        "indexes:AuthorityIndexes::default()",
        "resources:ResourceLedger::new(resources.limits())",
        "membership:MembershipProjection::default()",
        "scheduler:FairFrontier::new(scheduler.verify_order())",
        "dependencies:DependencyFrontier::default()",
    ):
        if fragment not in fresh:
            errors.append(
                f"FreshGeneration::empty lost allocation-terminal fragment {fragment!r}"
            )

    aftermath = "".join(
        mask_rust_non_code(aftermath_methods.get("disposition", "")).split()
    )
    if (
        "SettlementOrigin::Capture(ResolutionExecutionKind::ResourceUnavailable)"
        "|SettlementOrigin::Resolution(ResolutionExecutionKind::ResourceUnavailable)=>"
        "{AuthorityComputeAftermathDisposition::ReplaceGeneration}" not in aftermath
    ):
        errors.append(
            "retained compute allocation must carry the generation-terminal disposition"
        )

    request_fallback = "".join(
        mask_rust_non_code(
            request_methods.get("into_generation_replacement", "")
        ).split()
    )
    command_fallback = "".join(
        mask_rust_non_code(
            command_methods.get("into_generation_replacement", "")
        ).split()
    )
    expected_chain_fallback = "ChainGenerationReplacement{snapshot:self.snapshot,}"
    if request_fallback != expected_chain_fallback:
        errors.append(
            "a failed raw chain request must reduce to only its exact ordered snapshot"
        )
    if command_fallback != expected_chain_fallback:
        errors.append(
            "a failed prepared chain command must reduce to only its exact ordered snapshot"
        )

    chain_update = "".join(
        mask_rust_non_code(service_methods.get("commit_chain_update", "")).split()
    )
    chain_fallback_fragments = (
        "matchrequest.prepare()",
        "Err(failure)=>{let(error,returned)=failure.into_parts();",
        "self.runtime.apply_chain_generation_replacement(returned.into_generation_replacement(),)",
    )
    for fragment in chain_fallback_fragments:
        expected_count = 2 if fragment != "matchrequest.prepare()" else 1
        if chain_update.count(fragment) != expected_count:
            errors.append(
                "ordered chain preparation and Apply failures must each consume the exact "
                f"returned carrier into one snapshot replacement: {fragment!r}"
            )
    if any(term in chain_update for term in ("loop{", "tokio::time", "sleep(")):
        errors.append(
            "ordered chain allocation may not retry or wait against an unchanged allocator cut"
        )

    chain_generation_replacement = "".join(
        mask_rust_non_code(
            runtime_methods.get("apply_chain_generation_replacement", "")
        ).split()
    )
    ordered_generation_fragments = (
        "letsnapshot=replacement.into_snapshot();",
        "letcommitted_snapshot=Arc::clone(&snapshot);",
        "self.replace_generation(Some(snapshot))?;",
        "snapshot:committed_snapshot",
    )
    positions = [
        chain_generation_replacement.find(fragment)
        for fragment in ordered_generation_fragments
    ]
    if any(position < 0 for position in positions) or positions != sorted(positions):
        errors.append(
            "ordered chain allocation fallback must consume the command, atomically install its "
            "exact snapshot and return that same committed observation"
        )

    administration = "".join(
        mask_rust_non_code(service_methods.get("run_administration", "")).split()
    )
    if "Err(AuthorityAdministrationError::Allocation)" in administration:
        errors.append("administration may wait only for effect capacity, never allocation")
    mapping = "".join(
        mask_rust_non_code(function_body(service, "map_administration_error") or "").split()
    )
    if (
        "AuthorityAdministrationError::Allocation=>"
        "AuthorityServiceError::ResourceUnavailable" not in mapping
    ):
        errors.append("administration allocation must remain an exact terminal service outcome")
    return errors


def validate_canonical_accepted_removal_set() -> list[str]:
    """Keep ordered traversal distinct from canonical set observations."""

    try:
        membership_source = TX_POOL_AUTHORITY_MEMBERSHIP.read_text()
        membership = "".join(mask_rust_non_code(membership_source).split())
        accepted_constructor = impl_method_body(
            membership_source, "AcceptedRemovalSet", "try_from_vec"
        )
        plan = "".join(mask_rust_non_code(TX_POOL_AUTHORITY_PLAN.read_text()).split())
        chain = "".join(
            mask_rust_non_code(TX_POOL_AUTHORITY_CHAIN_TRANSITION.read_text()).split()
        )
    except (OSError, ValueError) as error:
        return [f"cannot inspect canonical Accepted removal set: {error}"]

    errors: list[str] = []
    required_membership = (
        "structAcceptedRemovalSet{hashes:Vec<RawTxHash>,}",
        "self.hashes.binary_search(hash).is_ok()",
        "fnprepare_chain_projection(&mutself,removals:&AcceptedRemovalSet,",
        "ifstatus_changes.keys().any(|hash|removals.contains(hash))",
        "if!removals.contains(child)&&!affected.contains(child)",
    )
    for fragment in required_membership:
        if fragment not in membership:
            errors.append(
                "Accepted removal set lost its sealed sort/unique/set-observation fragment "
                f"{fragment!r}"
            )
    errors.extend(
        validate_typed_adjacent_uniqueness(
            accepted_constructor,
            "AcceptedRemovalSet::try_from_vec hash uniqueness",
            "hashes.sort_unstable()",
            "hashes",
            "left==right",
        )
    )
    if plan.count("AcceptedRemovalSet::try_from_vec(accepted_removals)?") != 1:
        errors.append("administrative removal must seal its Accepted subset exactly once")
    if chain.count("AcceptedRemovalSet::try_from_vec(accepted_removals)?") != 1:
        errors.append("chain removal must seal its Accepted subset exactly once")
    if "Administrative(&'set[RawTxHash])" in plan:
        errors.append("a raw traversal slice cannot represent the canonical removal set")
    return errors


def validate_effect_publication_authority() -> list[str]:
    """Keep effect acquisition read-only and settlement claim-bound."""

    runtime = TX_POOL_AUTHORITY_RUNTIME.read_text()
    publisher = TX_POOL_AUTHORITY_PUBLISHER.read_text()
    effect = (REPO_ROOT / "tx-pool" / "src" / "authority" / "effect.rs").read_text()
    errors: list[str] = []
    required_runtime = (
        "struct AuthorityEffectPublicationLease<'runtime, 'claim>",
        "_claim: &'claim mut AuthorityEffectPublisherClaim",
        "fn try_effect_publication(&self) -> EffectPublicationObservation",
        "runtime: self,",
        "receipt: Some(receipt),",
    )
    for fragment in required_runtime:
        if runtime.count(fragment) != 1:
            errors.append(f"effect publication capability lost {fragment!r}")
    if re.search(r"pub[^\n]*fn\s+settle_effect\s*\(", runtime):
        errors.append("core effect settlement must remain private to the claim-bound runtime receipt")
    if runtime.count("fn settle_effect(") != 1:
        errors.append("effect settlement must have one runtime implementation owner")
    if publisher.count("wait_effect_publication(&mut claim)") != 1:
        errors.append("the sole publisher must borrow every receipt from its mutable claim")
    for retired in (
        "PreparedEffectCheckout",
        "EffectCheckoutError",
        "plan_effect_checkout",
        "wait_effect_checkout",
        "EffectMutation::Checkout",
        "publish_checked_out_effect_batch",
    ):
        if retired in runtime or retired in publisher or retired in effect:
            errors.append(f"retired effect checkout protocol resurfaced as {retired!r}")
    return errors


def validate_effect_publication_observation() -> list[str]:
    """Keep publisher head, idle and terminal state under the sole effect log."""

    try:
        effect = TX_POOL_AUTHORITY_EFFECT.read_text()
        runtime = TX_POOL_AUTHORITY_RUNTIME.read_text()
        observation = impl_method_body(
            effect, "EffectLog", "publication_observation"
        )
        level = impl_method_body(effect, "EffectLog", "publication_level")
        wake = impl_method_body(effect, "EffectLog", "wake_projection")
        acquire = impl_method_body(
            runtime, "AuthorityRuntime", "try_effect_publication"
        )
        wait = impl_method_body(
            runtime, "AuthorityRuntime", "wait_effect_publication"
        )
    except (OSError, ValueError) as error:
        return [f"cannot inspect total effect publication observation: {error}"]

    errors: list[str] = []
    declaration = re.search(
        r"enum\s+EffectPublicationObservation\s*\{(?P<body>.*?)\n\}",
        mask_rust_non_code(effect),
        re.S,
    )
    if declaration is None:
        errors.append("the log-owned effect publication observation disappeared")
    else:
        variants = re.findall(
            r"(?m)^\s*([A-Z][A-Za-z0-9_]*)\b", declaration.group("body")
        )
        if variants != ["Receipt", "Idle", "ClosedAndDrained"]:
            errors.append(
                "effect publication must remain the closed Receipt | Idle | "
                f"ClosedAndDrained observation, found {variants}"
            )

    compact_observation = "".join(mask_rust_non_code(observation).split())
    for fragment in (
        "self.publication_record()",
        "EffectPublicationObservation::Receipt(EffectReceipt{",
        "EffectPublicationObservation::ClosedAndDrained",
        "EffectPublicationObservation::Idle",
    ):
        if compact_observation.count(fragment) != 1:
            errors.append(
                f"EffectLog::publication_observation lost one total-state fragment {fragment!r}"
            )
    if compact_observation.count("self.is_closed_and_drained()") != 1:
        errors.append(
            "EffectLog must decide terminal drain exactly once after finding no publication head"
        )

    compact_level = "".join(mask_rust_non_code(level).split())
    if compact_level.count("self.publication_record().is_some()") != 1:
        errors.append("effect wake level must derive availability from the same head selector")
    if compact_level.count("self.is_closed_and_drained()") != 1:
        errors.append("effect wake level must derive terminality from the log")

    compact_wake = "".join(mask_rust_non_code(wake).split())
    if compact_wake.count("publisher:self.publication_level()") != 1:
        errors.append("Apply wake projection must consume the non-cloning publication level")
    if "publication_observation" in compact_wake or "EffectReceipt" in compact_wake:
        errors.append("Apply wake projection must not clone a publisher receipt")

    compact_acquire = "".join(mask_rust_non_code(acquire).split())
    if compact_acquire.count("self.store.read().authority.effect_publication_observation()") != 1:
        errors.append("the publisher must acquire exactly one coherent log-owned observation")
    if "effects_closed_and_drained" in compact_acquire or "effect_publication_receipt" in compact_acquire:
        errors.append("the runtime must not reassemble publisher state from split reads")

    compact_wait = "".join(mask_rust_non_code(wait).split())
    for variant in ("Idle", "Receipt", "ClosedAndDrained"):
        if compact_wait.count(f"EffectPublicationObservation::{variant}") != 1:
            errors.append(
                f"the sole publisher wait loop must consume {variant} exactly once"
            )
    if "EffectPublicationState" in runtime or "publication_receipt(" in effect:
        errors.append("retired split effect-publication state resurfaced")
    return errors


def validate_post_commit_wake_wiring() -> list[str]:
    """Bind every atomic Apply arm to one before/after cut and six Notify edges."""

    try:
        plan = TX_POOL_AUTHORITY_PLAN.read_text()
        runtime = TX_POOL_AUTHORITY_RUNTIME.read_text()
        apply = impl_method_body(plan, "PreparedApply", "apply")
        publish = impl_method_body(runtime, "AuthoritySignals", "publish_wake")
    except (OSError, ValueError) as error:
        return [f"cannot inspect post-commit wake wiring: {error}"]

    errors: list[str] = []
    compact_apply = "".join(mask_rust_non_code(apply).split())
    before = compact_apply.find("letbefore=authority.wake_projection();")
    transition = compact_apply.find("letretirement=matchdelta{")
    after = compact_apply.find("letafter=authority.wake_projection();")
    if min(before, transition, after) < 0 or not before < transition < after:
        errors.append(
            "PreparedApply::apply must capture one wake cut before and one after the complete delta match"
        )
    if compact_apply.count("authority.wake_projection()") != 2:
        errors.append("PreparedApply::apply must own exactly two wake projection reads")
    if compact_apply.count("wake:AuthorityWakeTransition{before,after}") != 1:
        errors.append("CommittedDelta must carry the exact Apply before/after wake transition")

    delta = re.search(
        r"enum\s+AuthorityDelta\s*\{(?P<body>.*?)\n\}",
        mask_rust_non_code(plan),
        re.S,
    )
    if delta is None:
        errors.append("the closed AuthorityDelta enum disappeared")
    else:
        variants = re.findall(
            r"(?m)^\s*([A-Z][A-Za-z0-9_]*)\b", delta.group("body")
        )
        for variant in variants:
            if compact_apply.count(f"AuthorityDelta::{variant}(") != 1:
                errors.append(
                    f"PreparedApply::apply must observe AuthorityDelta::{variant} exactly once"
                )
        match_end = compact_apply.find(";letafter=authority.wake_projection();")
        match_body = compact_apply[transition:match_end] if match_end >= 0 else ""
        if "_=>" in match_body:
            errors.append("AuthorityDelta wake coverage must remain exhaustive without a wildcard")

    compact_publish = "".join(mask_rust_non_code(publish).split())
    mappings = (
        ("compute_advanced", "compute", "notify_one"),
        ("ready_advanced", "ready", "notify_one"),
        ("dependency_maintenance_activated", "maintenance", "notify_one"),
        ("effect_publisher_advanced", "effect_publisher", "notify_one"),
        ("effect_capacity_released", "effect_capacity", "notify_waiters"),
        ("template_source_advanced", "template", "notify_waiters"),
    )
    for predicate, signal, notify in mappings:
        fragment = f"ifwake.{predicate}(){{self.{signal}.{notify}();}}"
        if compact_publish.count(fragment) != 1:
            errors.append(
                f"wake edge {predicate} must map exactly once to {signal}.{notify}"
            )
    if compact_publish.count(".notify_one()") != 4 or compact_publish.count(
        ".notify_waiters()"
    ) != 2:
        errors.append("AuthoritySignals must retain exactly four wake-one and two wake-all edges")
    return errors


def validate_released_input_projection() -> list[str]:
    """Keep replacement and administration on one projected-final-owner law."""

    try:
        plan = TX_POOL_AUTHORITY_PLAN.read_text()
        resolver = TX_POOL_AUTHORITY_RESOLVER.read_text()
        membership = TX_POOL_AUTHORITY_MEMBERSHIP.read_text()
        eviction = TX_POOL_AUTHORITY_MEMBERSHIP_EVICTION.read_text()
        replacement = required_function_body(
            plan, "collect_released_replacement_inputs"
        )
        administrative = required_function_body(
            plan, "collect_released_administrative_inputs"
        )
        shared = required_function_body(
            plan, "released_input_survives_final_owner_set"
        )
        removal_batch = required_function_body(plan, "plan_owner_removal_batch")
        capture_cell = impl_method_body(resolver, "AcceptedOverlay", "capture_cell")
        provide_cell = required_function_body(resolver, "cell")
        surviving_parent = required_function_body(membership, "surviving_pool_parent")
        candidate_input = required_function_body(
            membership, "validate_candidate_input_evidence"
        )
        candidate_dependency = required_function_body(
            membership, "validate_candidate_dependency_evidence"
        )
        complete_removals = required_function_body(eviction, "complete_removals")
    except (OSError, ValueError) as error:
        return [f"cannot inspect projected released-input relation: {error}"]

    errors: list[str] = []
    compact_replacement = "".join(mask_rust_non_code(replacement).split())
    compact_administrative = "".join(mask_rust_non_code(administrative).split())
    compact_shared = "".join(mask_rust_non_code(shared).split())
    for name, body, removal, context in (
        (
            "replacement",
            compact_replacement,
            "ProjectedRemovalSet::Replacement(&removed)",
            "ReleasedInputContext::Replacement{candidate_inputs:&candidate_inputs,}",
        ),
        (
            "administrative",
            compact_administrative,
            "ProjectedRemovalSet::Administrative(removals)",
            "ReleasedInputContext::Administrative{victim:hash}",
        ),
    ):
        if body.count("self.released_input_survives_final_owner_set(") != 1:
            errors.append(f"the {name} collector must consume the shared input law once")
        if body.count(removal) != 1 or body.count(context) != 1:
            errors.append(f"the {name} collector lost its distinct closed context")
        for duplicate in (
            ".proof.is_chain_input(input)",
            "RawTxHash(input.tx_hash())",
            ".record.tx.outputs().len()",
            ".membership.spender(input)",
        ):
            if duplicate in body:
                errors.append(
                    f"the {name} collector duplicated shared final-owner policy {duplicate!r}"
                )

    if compact_shared.count("self.membership.spender(input)") != 2:
        errors.append(
            "the shared input law must own exactly the replacement and administrative spender premises"
        )
    for fragment in (
        "candidate_inputs.contains(input)",
        "final_owners.contains_removed(spender)",
        "self.membership.spender(input)!=Some(victim)",
        "removed_entry.proof.is_chain_input(input)",
        "final_owners.contains_removed(&parent)",
        "Some(OwnedTx::Accepted(parent))=self.entries.get(&parent)",
        "index<parent.record.tx.outputs().len()",
    ):
        if compact_shared.count(fragment) != 1:
            errors.append(
                f"the shared released-input relation lost exact semantic fragment {fragment!r}"
            )

    compact_removal = "".join(mask_rust_non_code(removal_batch).split())
    ordered_removal = (
        "letmembership=self.prepare_chain_projection(&accepted_removals,&HashMap::new())?",
        "letavailable=self.collect_released_administrative_inputs(&accepted_removals)?",
        "letlost=self.collect_dependency_loss_keys(owner_refs.iter().copied())?.keys",
        "self.dependencies.plan_events(available,lost,DependencyCut(sequence))?",
        ".plan_replacements(owner_refs.iter().copied().map(|owner|(Some(owner),None)))?",
        ".with_control(dependency_control)",
    )
    positions = [compact_removal.find(fragment) for fragment in ordered_removal]
    if any(position < 0 for position in positions) or positions != sorted(positions):
        errors.append(
            "administrative removal must derive final-owner availability and publish it "
            "with loss before replacing dependency slots"
        )

    compact_capture = "".join(mask_rust_non_code(capture_cell).split())
    for fragment in (
        "letSome(OwnedTx::Accepted(entry))=authority.entry(&query.producer)else{return;}",
        "ifindex<entry.record.tx.outputs().len(){",
        "version:entry.record.version",
        "tx:Arc::clone(&entry.record.tx)",
    ):
        if fragment not in compact_capture:
            errors.append(
                f"AcceptedOverlay::capture_cell lost strict pool-output producer {fragment!r}"
            )
    compact_provider = "".join(mask_rust_non_code(provide_cell).split())
    if (
        "letSome((output,data))=producer.tx.output_with_data(index)else{returnCellStatus::Unknown;}"
        not in compact_provider
    ):
        errors.append("SparsePoolCellProvider must reject every absent pool output")

    compact_parent = "".join(mask_rust_non_code(surviving_parent).split())
    for fragment in (
        "letSome(OwnedTx::Accepted(entry))=self.entries.get(&parent)else{returnOk(None);}",
        "index<entry.record.tx.data().raw().outputs().len()",
        "MembershipReject::MissingPoolOutput(out_point.clone())",
        "Ok(Some(parent))",
    ):
        if fragment not in compact_parent:
            errors.append(
                f"surviving_pool_parent lost sealed membership producer {fragment!r}"
            )
    compact_input = "".join(mask_rust_non_code(candidate_input).split())
    compact_dependency = "".join(mask_rust_non_code(candidate_dependency).split())
    for body, owner, fragments in (
        (
            compact_input,
            "validate_candidate_input_evidence",
            (
                "candidate.proof.is_chain_input(input)",
                "self.membership.spender(input).is_some_and(|spender|removed.contains(spender))",
                "self.surviving_pool_parent(input,removed)?.is_none()",
                "MembershipReject::MissingInputEvidence(input.clone())",
            ),
        ),
        (
            compact_dependency,
            "validate_candidate_dependency_evidence",
            (
                "candidate.proof.is_chain_dependency(dependency)",
                "self.surviving_pool_parent(dependency,removed)?.is_some()",
                "MembershipReject::MissingDependencyEvidence(dependency.clone())",
            ),
        ),
    ):
        for fragment in fragments:
            if fragment not in body:
                errors.append(f"{owner} lost positive evidence fragment {fragment!r}")

    compact_eviction = "".join(mask_rust_non_code(complete_removals).split())
    ordered_evidence = (
        "authority.validate_candidate_input_evidence(candidate,&removed)?",
        "authority.validate_candidate_dependency_evidence(candidate,&removed)?",
        "letcandidate_parents=authority.candidate_parents(candidate,&removed)?",
    )
    positions = [compact_eviction.find(fragment) for fragment in ordered_evidence]
    if any(position < 0 for position in positions) or positions != sorted(positions):
        errors.append(
            "membership eviction must seal input and dependency evidence before deriving parents"
        )

    authority_root = TX_POOL_AUTHORITY_PLAN.parent
    expected_occurrences = {
        "collect_released_administrative_inputs": {
            TX_POOL_AUTHORITY_PLAN: 2,
        },
        "released_input_survives_final_owner_set": {
            TX_POOL_AUTHORITY_PLAN: 3,
        },
        "surviving_pool_parent": {
            TX_POOL_AUTHORITY_MEMBERSHIP: 4,
        },
        "validate_candidate_input_evidence": {
            TX_POOL_AUTHORITY_MEMBERSHIP: 1,
            TX_POOL_AUTHORITY_MEMBERSHIP_EVICTION: 1,
        },
        "validate_candidate_dependency_evidence": {
            TX_POOL_AUTHORITY_MEMBERSHIP: 1,
            TX_POOL_AUTHORITY_MEMBERSHIP_EVICTION: 1,
        },
    }
    production_sources: dict[Path, str] = {}
    for path in sorted(authority_root.rglob("*.rs")):
        if "tests" in path.relative_to(authority_root).parts:
            continue
        try:
            production_sources[path] = mask_rust_non_code(path.read_text())
        except OSError as error:
            errors.append(f"cannot inspect released-input producer surface {path}: {error}")
    for symbol, expected in expected_occurrences.items():
        pattern = re.compile(rf"\b{re.escape(symbol)}\s*\(")
        observed = {
            path: len(pattern.findall(source))
            for path, source in production_sources.items()
            if pattern.search(source)
        }
        if observed != expected:
            rendered_expected = {
                str(path.relative_to(REPO_ROOT)): count for path, count in expected.items()
            }
            rendered_observed = {
                str(path.relative_to(REPO_ROOT)): count for path, count in observed.items()
            }
            errors.append(
                f"closed producer/caller surface for {symbol} changed: "
                f"expected {rendered_expected}, found {rendered_observed}"
            )
    return errors


def validate_direct_negative_evidence() -> list[str]:
    """Bind direct negative validity to its exact bounded Accepted read set."""

    try:
        resolver = TX_POOL_AUTHORITY_RESOLVER.read_text()
        rejection = TX_POOL_AUTHORITY_REJECTION.read_text()
        validation = TX_POOL_AUTHORITY_VALIDATION.read_text()
        plan = TX_POOL_AUTHORITY_PLAN.read_text()
        runtime = TX_POOL_AUTHORITY_RUNTIME.read_text()
        source = (TX_POOL_SRC / "authority" / "source.rs").read_text()
        overlay = rust_type_body(resolver, "struct", "AcceptedOverlay")
        owner = rust_type_body(resolver, "struct", "AcceptedOwnerObservation")
        producer = rust_type_body(resolver, "struct", "AcceptedProducerObservation")
        prepare = impl_method_body(resolver, "AcceptedOverlay", "prepare")
        prepare_resolved = impl_method_body(
            resolver, "AcceptedOverlay", "prepare_resolved"
        )
        prepare_refresh = impl_method_body(
            resolver, "AcceptedOverlay", "prepare_refresh"
        )
        is_current = impl_method_body(resolver, "AcceptedOverlay", "is_current")
        same_observations = impl_method_body(
            resolver, "AcceptedOverlay", "same_observations"
        )
        current_producer = impl_method_body(
            resolver, "AcceptedOverlay", "current_producer_version"
        )
        current_spender = impl_method_body(
            resolver, "AcceptedOverlay", "current_spender"
        )
        capture_cell = impl_method_body(resolver, "AcceptedOverlay", "capture_cell")
        prepare_probe = impl_method_body(
            resolver, "DirectResolutionProbe", "prepare_enrichment"
        )
        observe_probe = impl_method_body(
            resolver, "PreparedDirectResolutionProbe", "observe"
        )
        finish_probe = impl_method_body(
            resolver, "DirectResolutionProbeRecheck", "finish"
        )
        direct_resolution = impl_method_body(
            runtime, "AuthorityRuntime", "prepare_direct_resolution"
        )
        validity = rust_type_body(rejection, "enum", "DirectRejectionValidity")
        validate_validity = required_function_body(
            plan, "validate_direct_rejection_validity"
        )
        prepare_validation = required_function_body(
            validation, "prepare_accepted_overlay"
        )
        membership_validation = required_function_body(
            validation, "validate_membership"
        )
        source_versions = rust_type_body(source, "struct", "AuthoritySourceVersions")
    except (OSError, ValueError) as error:
        return [f"cannot inspect exact direct-negative evidence relation: {error}"]

    errors: list[str] = []
    if None in (overlay, owner, producer, validity, source_versions):
        return ["the exact direct-negative evidence type graph is incomplete"]

    def dense(body: str) -> str:
        return "".join(mask_rust_non_code(body).split())

    dense_overlay = dense(overlay or "")
    for fragment in (
        "producers:HashMap<RawTxHash,AcceptedProducerObservation>",
        "spent_inputs:HashMap<OutPoint,AcceptedOwnerObservation>",
        "queries:HashSet<CellQuery>",
    ):
        if dense_overlay.count(fragment) != 1:
            errors.append(f"AcceptedOverlay lost exact bounded-read field {fragment!r}")
    for body, owner_name, fragments in (
        (
            owner or "",
            "AcceptedOwnerObservation",
            ("key:RawTxHash", "version:EntryVersion"),
        ),
        (
            producer or "",
            "AcceptedProducerObservation",
            ("owner:AcceptedOwnerObservation", "tx:Arc<TransactionView>"),
        ),
    ):
        compact = dense(body)
        for fragment in fragments:
            if compact.count(fragment) != 1:
                errors.append(f"{owner_name} lost exact observation field {fragment!r}")

    try:
        cell_roles = rust_enum_variants(resolver, "CellRole")
        validity_variants = rust_enum_variants(rejection, "DirectRejectionValidity")
    except ValueError as error:
        errors.append(f"cannot inspect direct read/evidence constructors: {error}")
    else:
        errors.extend(
            enum_bijection_errors(
                "Accepted cell-read role", cell_roles, ["Input", "ProducerOnly"]
            )
        )
        errors.extend(
            enum_bijection_errors(
                "Direct rejection validity",
                validity_variants,
                ["Stable", "AcceptedReads"],
            )
        )
    dense_validity = dense(validity or "")
    for fragment in ("AcceptedReads{", "view:ChainViewId", "reads:AcceptedOverlay"):
        if fragment not in dense_validity:
            errors.append(f"DirectRejectionValidity lost exact receipt fragment {fragment!r}")

    dense_prepare = dense(prepare)
    for fragment in (
        ".inputs().len().checked_add(tx.cell_deps().len()).and_then(|count|count.checked_add(tx.header_deps().len()))",
        "ifdirect_edges>max_edges",
        "spent_inputs.try_reserve(tx.inputs().len())",
        "CellQuery::new(out_point,CellRole::Input)",
        "CellQuery::new(cell_dep.out_point(),CellRole::ProducerOnly)",
    ):
        if fragment not in dense_prepare:
            errors.append(f"direct read preparation lost bounded-domain fragment {fragment!r}")
    dense_resolved = dense(prepare_resolved)
    for fragment in (
        ".resolved_inputs.len().checked_add(resolved.resolved_cell_deps.len()).and_then(|count|count.checked_add(resolved.resolved_dep_groups.len()))",
        "producers.try_reserve(total_cells)",
        "queries.try_reserve(total_cells)",
        "CellRole::ProducerOnly",
    ):
        if fragment not in dense_resolved:
            errors.append(f"final-validation read preparation lost fragment {fragment!r}")

    dense_refresh = dense(prepare_refresh)
    refresh_fragments = (
        "self.queries.len().checked_add(missing.len())",
        "refreshed.queries.try_reserve(upper)",
        "refreshed.queries.extend(self.queries.iter().cloned())",
        "refreshed.queries.extend(missing.iter().cloned())",
        "ifrefreshed.queries.len()>max_edges",
        "refreshed.producers.try_reserve(refreshed.queries.len())",
        "refreshed.spent_inputs.try_reserve(input_count)",
    )
    positions = [dense_refresh.find(fragment) for fragment in refresh_fragments]
    if any(position < 0 for position in positions) or positions != sorted(positions):
        errors.append(
            "direct read refresh must preallocate the bounded old-plus-new query union in order"
        )

    def current_relation_errors(body: str) -> list[str]:
        compact = dense(body)
        missing = []
        for fragment in (
            "self.queries.iter().all(|query|",
            "self.producer_version(query)==Self::current_producer_version(authority,query)",
            "query.role!=CellRole::Input",
            "self.spent_inputs.get(&query.out_point)==Self::current_spender(authority,&query.out_point).as_ref()",
        ):
            if fragment not in compact:
                missing.append(fragment)
        if any(
            token in compact
            for token in ("collect::<", "Vec::", "vec![", "try_reserve(", "with_capacity(")
        ):
            missing.append("allocation-free exact currentness fold")
        return missing

    missing_current = current_relation_errors(is_current)
    if missing_current:
        errors.append(
            "AcceptedOverlay currentness lost its exact producer/spender relation: "
            f"{missing_current}"
        )
    current_canary = is_current.replace("Self::current_spender", "Self::captured_spender", 1)
    if not current_relation_errors(current_canary):
        errors.append(
            "direct-negative relation gate failed its missing-spender negative canary"
        )

    dense_same = dense(same_observations)
    for fragment in (
        "other.queries.iter().all(|query|",
        "self.producer_version(query)==other.producer_version(query)",
        "self.spent_inputs.get(&query.out_point)==other.spent_inputs.get(&query.out_point)",
    ):
        if fragment not in dense_same:
            errors.append(f"direct recheck lost exact observation equality {fragment!r}")
    for body, owner_name, fragments in (
        (
            current_producer,
            "current producer observation",
            (
                "letSome(OwnedTx::Accepted(entry))=authority.entry(&query.producer)",
                "index<entry.record.tx.outputs().len()",
                "entry.record.version",
            ),
        ),
        (
            current_spender,
            "current spender observation",
            (
                "authority.accepted_spender(out_point)?",
                "letSome(OwnedTx::Accepted(entry))=authority.entry(&key)",
                "version:entry.record.version",
            ),
        ),
        (
            capture_cell,
            "captured Accepted observation",
            (
                "query.role==CellRole::Input",
                "Self::current_spender(authority,&query.out_point)",
                "version:entry.record.version",
                "tx:Arc::clone(&entry.record.tx)",
            ),
        ),
    ):
        compact = dense(body)
        for fragment in fragments:
            if fragment not in compact:
                errors.append(f"{owner_name} lost exact owner/version fragment {fragment!r}")

    dense_probe_prepare = dense(prepare_probe)
    for fragment in (
        ".missing.first()",
        ".overlay.prepare_refresh(&self.missing,self.job.max_edges)",
    ):
        if fragment not in dense_probe_prepare:
            errors.append(f"direct probe preparation lost bounded fragment {fragment!r}")
    dense_observe = dense(observe_probe)
    observe_order = (
        "authority.chain_view()!=&self.job.view",
        "self.refreshed.populate(authority)",
        "!self.job.overlay.same_observations(&self.refreshed)",
        "std::mem::replace(&mutself.job.overlay,self.refreshed)",
        "ifchanged",
        "self.job.dependency_cut=authority.dependency_observation_cut()",
        "DirectResolutionProbeCut::Retry(self.job)",
    )
    positions = [dense_observe.find(fragment) for fragment in observe_order]
    if any(position < 0 for position in positions) or positions != sorted(positions):
        errors.append(
            "direct probe must compare one coherent exact cut and refresh retry evidence atomically"
        )
    dense_finish = dense(finish_probe)
    if "drop(discarded);" not in dense_finish:
        errors.append("discarded direct read receipts must retire in the lock-external carrier")
    dense_runtime = dense(direct_resolution)
    runtime_order = (
        "letrechecked={letstore=self.store.read();prepared.observe(&store.authority)};",
        "letobservation=rechecked.finish()?;",
    )
    positions = [dense_runtime.find(fragment) for fragment in runtime_order]
    if any(position < 0 for position in positions) or positions != sorted(positions):
        errors.append(
            "direct runtime must drop the read guard before retiring or interpreting recheck scratch"
        )

    dense_plan = dense(validate_validity)
    plan_order = (
        "DirectRejectionValidity::Stable=>Ok(())",
        "DirectRejectionValidity::AcceptedReads{view,reads}=>",
        "ifview!=&self.chain_view",
        "if!reads.is_current(self)",
        "StalePlan::AcceptedObservation",
    )
    positions = [dense_plan.find(fragment) for fragment in plan_order]
    if any(position < 0 for position in positions) or positions != sorted(positions):
        errors.append(
            "direct negative Plan must validate chain and exact Accepted observations in order"
        )
    plan_canary = validate_validity.replace("!reads.is_current(self)", "false", 1)
    if "if!reads.is_current(self)" in dense(plan_canary):
        errors.append("direct-negative Plan gate failed its missing-currentness negative canary")

    if "AcceptedOverlay::prepare_resolved(payload)" not in dense(prepare_validation):
        errors.append("final validation must prepare its exact Accepted read receipt")
    dense_membership = dense(membership_validation)
    if dense_membership.count("accepted_reads:overlay") != 2:
        errors.append(
            "both final-validation rejection exits must carry the exact Accepted read receipt"
        )

    production_sources = []
    authority_root = TX_POOL_SRC / "authority"
    for path in sorted(authority_root.rglob("*.rs")):
        if "tests" in path.relative_to(authority_root).parts:
            continue
        try:
            production_sources.append((path, mask_rust_non_code(path.read_text())))
        except (OSError, ValueError) as error:
            errors.append(f"cannot inspect direct-negative production source {path}: {error}")
    retired = re.compile(r"\baccepted_source(?:_cut)?\b")
    occurrences = [
        str(path.relative_to(REPO_ROOT))
        for path, masked in production_sources
        if retired.search(masked)
    ]
    if occurrences:
        errors.append(
            "direct negative evidence regained a global Accepted content clock in "
            f"{occurrences}"
        )
    dense_sources = dense(source_versions or "")
    if "relay_parents:ApplySequence" not in dense_sources or "template:PoolTemplateVersions" not in dense_sources:
        errors.append("AuthoritySourceVersions lost its two remaining derived source owners")
    if re.search(r"\baccepted\s*:\s*ApplySequence\b", source_versions or ""):
        errors.append("AuthoritySourceVersions must not own a direct-invalidating Accepted clock")
    return errors


def validate_owner_transition_construction() -> list[str]:
    """Keep nonempty owner changes and the retirement carrier structural."""

    try:
        plan = TX_POOL_AUTHORITY_PLAN.read_text()
        masked_plan = mask_rust_non_code(plan)
        authority_methods = {
            name: body
            for name, body, _line in rust_impl_methods(
                plan, "TxPoolAuthority", allow_multiple=True
            )
        }
        prepare = authority_methods["prepare_entry_delta_with_controls"]
        compile_membership = authority_methods["compile_membership_delta"]
        direct = authority_methods["plan_direct_admission"]
        internal = authority_methods["plan_internal_plug"]
        final = authority_methods["prepare_accept_delta"]
        apply_membership = impl_method_body(plan, "PreparedApply", "apply_membership")
    except (OSError, KeyError, ValueError) as error:
        return [f"cannot inspect closed owner-transition construction: {error}"]

    errors: list[str] = []

    def declaration_body(kind: str, name: str) -> str | None:
        declaration = re.search(rf"\b{kind}\s+{re.escape(name)}\s*\{{", masked_plan)
        if declaration is None:
            return None
        opening = masked_plan.find("{", declaration.start())
        closing = matching_brace(masked_plan, opening)
        return None if closing is None else masked_plan[opening + 1 : closing]

    transition_body = declaration_body("enum", "EntryTransition")
    controls_body = declaration_body("struct", "TransitionControls")
    retirement_body = declaration_body("enum", "MembershipRetirement")
    membership_delta_body = declaration_body("struct", "MembershipDelta")
    if any(
        body is None
        for body in (
            transition_body,
            controls_body,
            retirement_body,
            membership_delta_body,
        )
    ):
        return [
            "EntryTransition, TransitionControls, MembershipRetirement or "
            "MembershipDelta declaration disappeared"
        ]
    variants = re.findall(r"(?m)^\s*([A-Z][A-Za-z0-9_]*)\s*\{", transition_body)
    if variants != ["Insert", "Replace", "Remove"]:
        errors.append(
            f"EntryTransition must expose exactly Insert, Replace and Remove, found {variants}"
        )
    compact_transition = "".join(transition_body.split())
    if "Option<OwnedTx>" in compact_transition:
        errors.append("EntryTransition must not regain an optional before/after no-op state")
    for indirection in ("Box<OwnedTx>", "Arc<OwnedTx>"):
        if indirection in compact_transition:
            errors.append(
                f"EntryTransition must remain stack-owned without hot-path indirection {indirection!r}"
            )
    fields = re.findall(r"(?m)^\s*([a-z][A-Za-z0-9_]*)\s*:", controls_body)
    if fields != ["dependency", "effect"]:
        errors.append(
            f"TransitionControls must own exactly dependency and effect, found {fields}"
        )
    retirement_variants = re.findall(
        r"(?m)^\s*([A-Z][A-Za-z0-9_]*)\s*\(\s*Vec\s*<\s*OwnedTx\s*>\s*\)\s*,",
        retirement_body,
    )
    if retirement_variants != ["Inline", "Outside"]:
        errors.append(
            "MembershipRetirement must fuse policy with exactly Inline and Outside Vec "
            f"carriers, found {retirement_variants}"
        )
    membership_fields = re.findall(
        r"(?m)^\s*([a-z][A-Za-z0-9_]*)\s*:", membership_delta_body
    )
    expected_membership_fields = [
        "changed_key",
        "changed_after",
        "retirement",
        "removals",
        "owners",
        "resource",
        "projection",
        "scheduler",
        "dependency",
        "effect",
        "clocks",
        "async_process_start",
    ]
    if membership_fields != expected_membership_fields:
        errors.append(
            "MembershipDelta must own one fused retirement fact: "
            f"expected {expected_membership_fields}, found {membership_fields}"
        )

    expected_sites = {
        "plan_charged_admission": {"Insert": 1},
        "plan_existing_admission": {"Replace": 1},
        "plan_replacement_history_admission": {"Replace": 1},
        "plan_final_reresolution": {"Replace": 1},
        "plan_preaccepted_terminalization": {"Remove": 1},
        "plan_dependency_maintenance": {"Replace": 1},
        "prepare_settlement_inner": {"Replace": 1},
        "prepare_compute_rejection": {"Remove": 1},
        "prepare_entry_delta_with_controls": {
            "Insert": 1,
            "Replace": 1,
            "Remove": 1,
        },
    }
    site_pattern = re.compile(r"\bEntryTransition::(Insert|Replace|Remove)\s*\{")
    observed_sites: dict[str, dict[str, int]] = {}
    for method, body in authority_methods.items():
        found: dict[str, int] = {}
        for variant in site_pattern.findall(body):
            found[variant] = found.get(variant, 0) + 1
        if found:
            observed_sites[method] = found
    if observed_sites != expected_sites:
        errors.append(
            "EntryTransition constructor/match surface changed: "
            f"expected {expected_sites}, found {observed_sites}"
        )
    if len(site_pattern.findall(masked_plan)) != sum(
        sum(counts.values()) for counts in expected_sites.values()
    ):
        errors.append("EntryTransition escaped the closed TxPoolAuthority construction surface")

    compact_prepare = "".join(mask_rust_non_code(prepare).split())
    for fragment in (
        "EntryTransition::Insert{key,after}=>{(key,None,Some(after),1,EntryRetirement::InlineDrop)}",
        "EntryReplacementRetirement::SharedShellInline=>EntryRetirement::InlineDrop",
        "EntryReplacementRetirement::OutsideGuard=>{EntryRetirement::Outside(retired_buffer(1)?)}",
        "EntryTransition::Remove{key,before}=>(key,Some(before),None,0,EntryRetirement::Outside(retired_buffer(1)?),)",
        "self.reserve_primary_owner_insertions(primary_insertions)?",
    ):
        if fragment not in compact_prepare:
            errors.append(f"closed EntryTransition mapping lost fragment {fragment!r}")
    for forbidden in (
        "after.is_some()&&expected.is_none()",
        "expected.is_some()",
        "EntryTransition{",
    ):
        if forbidden in compact_prepare:
            errors.append(f"owner-transition mapping regained boolean option algebra {forbidden!r}")

    caller_pattern = re.compile(r"\bprepare_entry_delta_with_controls\s*\(")
    expected_callers = {
        "plan_preaccepted_terminalization": 1,
        "prepare_settlement_inner": 1,
        "prepare_compute_rejection": 1,
        "prepare_entry_delta": 1,
        "prepare_entry_delta_with_dependency": 1,
    }
    observed_callers = {
        method: len(caller_pattern.findall(body))
        for method, body in authority_methods.items()
        if caller_pattern.search(body)
    }
    if observed_callers != expected_callers:
        errors.append(
            "prepare_entry_delta_with_controls caller surface changed: "
            f"expected {expected_callers}, found {observed_callers}"
        )

    compact_membership = "".join(mask_rust_non_code(compile_membership).split())
    if "has_history" in compact_membership:
        errors.append("membership resource fallback regained a duplicate history predicate")
    fallback_fragments = (
        "Err(ResourceError::PreAcceptedLimit|ResourceError::ReplacementHistoryLimit)=>{",
        "removals.iter_mut().for_each(MembershipRemoval::terminalize)",
        "retained_history=false",
        "self.plan_membership_resources(&key,existing.as_ref(),&after,&removals)",
        "letretirement=matchchanged_retirement{",
        "ChangedOwnerRetirement::VacantOrSharedShellInline=>{MembershipRetirement::Inline(retired_buffer(removals.len())?)}",
        "ChangedOwnerRetirement::OutsideGuard=>{letcapacity=removals.len().checked_add(1).ok_or(PlanError::Fault(AuthorityFault::CounterExhausted))?;MembershipRetirement::Outside(retired_buffer(capacity)?)}",
    )
    cursor = 0
    positions: list[int] = []
    for fragment in fallback_fragments:
        position = compact_membership.find(fragment, cursor)
        positions.append(position)
        if position >= 0:
            cursor = position + len(fragment)
    if any(position < 0 for position in positions):
        errors.append(
            "membership must terminalize optional history, retry the same set transition and "
            "fuse changed-owner policy with its fallibly reserved carrier"
        )
    for forbidden in (
        "retired_owner_count",
        "changed_retirement:",
        "retired:",
    ):
        if forbidden in "".join(membership_delta_body.split()):
            errors.append(
                f"MembershipDelta regained split retirement fact {forbidden!r}"
            )
    for body, owner, fragments in (
        (
            "".join(mask_rust_non_code(direct).split()),
            "plan_direct_admission",
            (
                "letretirement=ifexisting.is_some(){ChangedOwnerRetirement::OutsideGuard}else{ChangedOwnerRetirement::VacantOrSharedShellInline}",
                "changed_retirement:retirement",
            ),
        ),
        (
            "".join(mask_rust_non_code(internal).split()),
            "plan_internal_plug",
            ("changed_retirement:ChangedOwnerRetirement::VacantOrSharedShellInline",),
        ),
        (
            "".join(mask_rust_non_code(final).split()),
            "prepare_accept_delta",
            (
                "ReadyPayloadRelation::Shared=>ChangedOwnerRetirement::VacantOrSharedShellInline",
                "ReadyPayloadRelation::LocationRefreshed=>ChangedOwnerRetirement::OutsideGuard",
            ),
        ),
    ):
        for fragment in fragments:
            if fragment not in body:
                errors.append(f"{owner} lost changed-owner retirement fragment {fragment!r}")

    compact_apply = "".join(mask_rust_non_code(apply_membership).split())
    for fragment in (
        "letmutretirement=delta.retirement",
        "match&mutretirement{MembershipRetirement::Inline(retired)|MembershipRetirement::Outside(retired)=>retired.push(owner)",
        "letretired=match(retirement,previous){",
        "(MembershipRetirement::Inline(retired),previous)=>{drop(previous);retired}",
        "(MembershipRetirement::Outside(mutretired),Some(owner))=>{retired.push(owner);retired}",
        "(MembershipRetirement::Outside(retired),None)=>retired",
    ):
        if fragment not in compact_apply:
            errors.append(
                f"PreparedApply::apply_membership lost fused carrier fragment {fragment!r}"
            )
    if "retired_buffer(" in compact_apply or ".try_reserve(" in compact_apply:
        errors.append("membership Apply must not allocate retirement capacity under the guard")
    carrier_sites = {
        variant: len(
            re.findall(rf"\bMembershipRetirement::{variant}\b", masked_plan)
        )
        for variant in ("Inline", "Outside")
    }
    if carrier_sites != {"Inline": 3, "Outside": 4}:
        errors.append(
            "MembershipRetirement construction/consumption surface changed: "
            f"expected Inline=3 and Outside=4, found {carrier_sites}"
        )
    return errors


def validate_evidence_and_settlement_construction() -> list[str]:
    """Bind legal evidence to sealed producers and one total settlement classifier."""

    try:
        chain = TX_POOL_AUTHORITY_CHAIN.read_text()
        validation = TX_POOL_AUTHORITY_VALIDATION.read_text()
        plan = TX_POOL_AUTHORITY_PLAN.read_text()
        work = TX_POOL_AUTHORITY_WORK.read_text()
        state = TX_POOL_AUTHORITY_STATE.read_text()
        ingress = TX_POOL_AUTHORITY_INGRESS.read_text()
        scheduler = TX_POOL_AUTHORITY_SCHEDULER.read_text()
        compute_exchange = TX_POOL_AUTHORITY_COMPUTE_EXCHANGE.read_text()
        chain_transition = TX_POOL_AUTHORITY_CHAIN_TRANSITION.read_text()
        final_work = required_function_body(plan, "final_admission_work")
        final_evidence = required_function_body(plan, "validate_acceptance_evidence")
        direct_evidence = required_function_body(
            plan, "validate_direct_acceptance_evidence"
        )
        membership_view = impl_method_body(chain, "MembershipReceipt", "view")
        final_receipt_key = impl_method_body(chain, "FinalAdmissionReceipt", "key")
        final_receipt_view = impl_method_body(chain, "FinalAdmissionReceipt", "view")
        direct_receipt_key = impl_method_body(chain, "DirectAdmissionReceipt", "key")
        direct_receipt_view = impl_method_body(chain, "DirectAdmissionReceipt", "view")
        subject = required_function_body(plan, "final_admission_subject_owner")
        classifier = required_function_body(plan, "classify_settlement")
        checkout = impl_method_body(work, "CheckedOutWork", "from_owner")
        final_location = impl_method_body(
            state, "VerifiedFacts", "with_final_validation"
        )
        refresh_locations = required_function_body(validation, "refresh_locations")
        queue_for_permit = impl_method_body(scheduler, "QueueLane", "for_permit")
        queue_population = impl_method_body(scheduler, "QueueLane", "population")
        owner_head_excluding = impl_method_body(
            scheduler, "OwnerQueue", "head_excluding"
        )
        frontier_slot = impl_method_body(scheduler, "FairFrontier", "slot")
        frontier_next = impl_method_body(
            scheduler, "FairFrontier", "next_queued_in_wave_with_overlay"
        )
        frontier_next_after = impl_method_body(
            scheduler, "FairFrontier", "next_queued_after_in_wave_with_overlay"
        )
        search_exchange_permit = impl_method_body(
            compute_exchange, "TxPoolAuthority", "search_exchange_permit"
        )
        exchange_checkout_resource = impl_method_body(
            compute_exchange, "TxPoolAuthority", "exchange_checkout_resource"
        )
        compile_compute_exchange_state = impl_method_body(
            compute_exchange, "TxPoolAuthority", "compile_compute_exchange_state"
        )
        policy_evolution = impl_method_body(state, "PayloadPolicy", "evolution_to")
        remote_ingress = impl_method_body(state, "RemoteBase", "ingress")
        plan_chain_transition = impl_method_body(
            chain_transition, "TxPoolAuthority", "plan_chain_transition"
        )
        defer_completion = required_function_body(compute_exchange, "defer_completion")
        recover_classified = impl_method_body(
            compute_exchange, "ClassifiedCompletion", "recover_into"
        )
        apply_exchange = impl_method_body(
            compute_exchange, "PreparedComputeExchange", "apply"
        )
    except (OSError, ValueError) as error:
        return [f"cannot inspect sealed evidence and settlement construction: {error}"]

    errors: list[str] = []
    compact_validation = "".join(mask_rust_non_code(validation).split())
    if validation.count("let seal = AdmissionValidationSeal(());") != 2:
        errors.append("final and direct validation must be the two seal construction cuts")
    for constructor in (
        "FinalAdmissionSubject::new(seal,key,expected,view,dependency_cut)",
        "FinalAdmissionReceipt::from_validation(seal,expected,membership,payload_relation,",
        "DirectAdmissionReceipt::from_validation(seal,tx,membership)",
    ):
        if compact_validation.count(constructor) != 1:
            errors.append(f"sealed validation lost its sole constructor {constructor!r}")

    seal = re.search(
        r"pub\s*\(super\)\s+struct\s+AdmissionValidationSeal\s*\(\s*\(\s*\)\s*\)\s*;",
        mask_rust_non_code(validation),
    )
    if seal is None:
        errors.append("AdmissionValidationSeal must retain a private tuple field")
    receipt_declarations: dict[str, re.Match[str]] = {}
    for receipt in (
        "MembershipReceipt",
        "FinalAdmissionReceipt",
        "FinalAdmissionSubject",
        "DirectAdmissionReceipt",
    ):
        declaration = re.search(
            rf"pub\s*\(super\)\s+struct\s+{receipt}\s*\{{(?P<body>.*?)\n\}}",
            mask_rust_non_code(chain),
            re.S,
        )
        if declaration is None or re.search(r"\bpub\b", declaration.group("body")):
            errors.append(f"{receipt} must retain private fields behind sealed constructors")
        elif declaration is not None:
            receipt_declarations[receipt] = declaration

    expected_receipt_fields = {
        "MembershipReceipt": {"proof", "proposal", "accepted_at", "async_process_start"},
        "FinalAdmissionReceipt": {"expected", "membership", "payload_relation"},
        "DirectAdmissionReceipt": {"tx", "membership"},
    }
    for receipt, expected_fields in expected_receipt_fields.items():
        declaration = receipt_declarations.get(receipt)
        if declaration is None:
            continue
        actual_fields = set(
            re.findall(r"^\s*([a-z_][A-Za-z0-9_]*)\s*:", declaration.group("body"), re.M)
        )
        if actual_fields != expected_fields:
            errors.append(
                f"{receipt} field topology changed: expected {sorted(expected_fields)}, "
                f"found {sorted(actual_fields)}"
            )

    derived_receipt_fragments = {
        "MembershipReceipt::view": (
            membership_view,
            "self.proof.admission_view()",
        ),
        "FinalAdmissionReceipt::key": (
            final_receipt_key,
            "&self.membership.proof().payload().identity().raw",
        ),
        "FinalAdmissionReceipt::view": (final_receipt_view, "self.membership.view()"),
        "DirectAdmissionReceipt::key": (
            direct_receipt_key,
            "&self.membership.proof().payload().identity().raw",
        ),
        "DirectAdmissionReceipt::view": (direct_receipt_view, "self.membership.view()"),
    }
    for owner, (body, fragment) in derived_receipt_fragments.items():
        if fragment.replace(" ", "") not in "".join(mask_rust_non_code(body or "").split()):
            errors.append(f"sealed receipt projection {owner} lost {fragment!r}")

    compact_final_work = "".join(mask_rust_non_code(final_work).split())
    for fragment in (
        ".get(key)",
        "existing.record().version!=expected",
        "letOwnedTx::PreAccepted(preaccepted)=existingelse",
        "letPreAcceptedPhase::Ready(verified)=&preaccepted.phaseelse",
    ):
        if compact_final_work.count(fragment) != 1:
            errors.append(f"FinalAdmissionWork lost reachable Ready-owner premise {fragment!r}")

    compact_final = "".join(mask_rust_non_code(final_evidence).split())
    final_order = (
        "receipt.view()!=&self.chain_view",
        "proof.payload().identity()!=&preaccepted.record.identity",
        ".proof_is_current(dependencies,proof.dependency_cut())",
    )
    positions = [compact_final.find(fragment) for fragment in final_order]
    if any(position < 0 for position in positions) or positions != sorted(positions):
        errors.append("final acceptance must check view, sealed identity and dependency cut in order")
    if "receipt.key()!=" in compact_final or "proof.is_for(" in compact_final or "||" in compact_final:
        errors.append("final acceptance reconstructed a duplicated key/view predicate")

    compact_direct = "".join(mask_rust_non_code(direct_evidence).split())
    direct_order = (
        "receipt.view()!=&self.chain_view",
        ".owner_free_proof_is_current(",
    )
    positions = [compact_direct.find(fragment) for fragment in direct_order]
    if any(position < 0 for position in positions) or positions != sorted(positions):
        errors.append("direct acceptance must check view before owner-free dependency currentness")
    if compact_direct.count(".owner_free_proof_is_current(") != 1:
        errors.append("direct acceptance must retain the global owner-free loss fence")
    if ".proof_is_current(" in compact_direct.replace(
        ".owner_free_proof_is_current(", ""
    ):
        errors.append("direct acceptance must not bypass the owner-free evidence relation")
    if "receipt.key()!=" in compact_direct or "proof.is_for(" in compact_direct or "||" in compact_direct:
        errors.append("direct acceptance reconstructed a duplicated key/view predicate")

    production_callers: dict[str, set[str]] = {
        "validate_acceptance_evidence": set(),
        "validate_direct_acceptance_evidence": set(),
    }
    production_occurrences = {validator: 0 for validator in production_callers}
    authority_root = TX_POOL_AUTHORITY_PLAN.parent
    for path in sorted(authority_root.rglob("*.rs")):
        if "tests" in path.relative_to(authority_root).parts:
            continue
        try:
            source = path.read_text()
            masked = mask_rust_non_code(source)
        except OSError as error:
            errors.append(f"cannot inspect acceptance caller source {path}: {error}")
            continue
        for validator in production_callers:
            token_pattern = re.compile(rf"\b{re.escape(validator)}\b")
            production_occurrences[validator] += len(token_pattern.findall(masked))
            call_pattern = re.compile(rf"(?:\.|::)\s*{re.escape(validator)}\s*\(")
            source_call_count = len(call_pattern.findall(masked))
            if source_call_count == 0:
                continue
            methods = rust_impl_methods(source, "TxPoolAuthority", allow_multiple=True)
            method_call_count = sum(len(call_pattern.findall(body)) for _, body, _ in methods)
            if method_call_count != source_call_count:
                errors.append(
                    f"{validator} call escaped a TxPoolAuthority method in "
                    f"{path.relative_to(REPO_ROOT)}"
                )
            for method, body, _line in methods:
                if call_pattern.search(body) is not None:
                    production_callers[validator].add(
                        f"{path.relative_to(REPO_ROOT)}::{method}"
                    )
    expected_callers = {
        "validate_acceptance_evidence": {
            "tx-pool/src/authority/plan.rs::prepare_accept_delta",
            "tx-pool/src/authority/plan/settlement.rs::plan_settlement",
        },
        "validate_direct_acceptance_evidence": {
            "tx-pool/src/authority/plan.rs::evaluate_direct_admission",
            "tx-pool/src/authority/plan.rs::plan_direct_admission",
            "tx-pool/src/authority/plan.rs::plan_internal_plug",
        },
    }
    for validator, expected in expected_callers.items():
        expected_occurrences = len(expected) + 1
        if production_occurrences[validator] != expected_occurrences:
            errors.append(
                f"{validator} complete production occurrence count changed: expected "
                f"{expected_occurrences}, found {production_occurrences[validator]}"
            )
        if production_callers[validator] != expected:
            errors.append(
                f"{validator} caller set changed: expected {sorted(expected)}, "
                f"found {sorted(production_callers[validator])}"
            )

    compact_subject = "".join(mask_rust_non_code(subject).split())
    subject_order = (
        "subject.view()!=&self.chain_view",
        ".get(subject.key())",
        "existing.record().version!=subject.expected()",
        "letOwnedTx::PreAccepted(preaccepted)=existingelse",
        "letPreAcceptedPhase::Ready(verified)=&preaccepted.phaseelse",
        ".proof_is_current(verified.payload().dependencies(),subject.dependency_cut())",
    )
    positions = [compact_subject.find(fragment) for fragment in subject_order]
    if any(position < 0 for position in positions) or positions != sorted(positions):
        errors.append("final subject lost its closed view/owner/version/Ready/dependency order")

    compact_classifier = "".join(mask_rust_non_code(classifier).split())
    baseline = compact_classifier.find(
        "self.dependencies.proof_is_current(preaccepted.dependencies(),dependency_cut)"
    )
    result_match = compact_classifier.find("matchnext{")
    if baseline < 0 or result_match < 0 or baseline > result_match:
        errors.append("every settlement result must pass the common baseline-currentness gate")
    if compact_classifier.count(
        "self.dependencies.proof_is_current(preaccepted.dependencies(),dependency_cut)"
    ) != 1:
        errors.append("the settlement baseline-currentness premise must have one owner")
    if compact_classifier.count(
        "verified.payload().identity()!=&preaccepted.record.identity"
    ) != 1:
        errors.append("Ready settlement must seal the proof identity to its exact owner")
    if "verified.witness()" in compact_classifier:
        errors.append("Ready settlement reconstructed witness beside its complete payload identity")

    compact_checkout = "".join(mask_rust_non_code(checkout).split())
    checkout_order = (
        "letPreAcceptedPhase::Queued(queued)=&preaccepted.phaseelse",
        "letdependency_cut=matchqueued{",
        "letpayload_policy=preaccepted.source.payload_policy();",
        "lettoken=LeaseToken{",
        "letactive=ActiveWork{",
        "letwork=match(permit,queued){",
        "Ok((work,active))",
    )
    positions = [compact_checkout.find(fragment) for fragment in checkout_order]
    if any(position < 0 for position in positions) or positions != sorted(positions):
        errors.append("checkout lost its one owner-to-work/ActiveWork construction order")
    checkout_facts = (
        "hash:preaccepted.record.identity.raw.clone()",
        "chain_view:chain_view.clone()",
        "attribution:preaccepted.source.compute_attribution()",
        "payload_policy",
        "dependencies:preaccepted.dependencies().clone()",
        "tx:Arc::clone(&preaccepted.record.tx)",
        "declared_dependencies:preaccepted.basis.dependencies().clone()",
        "resolved:resolved.clone()",
    )
    for fragment in checkout_facts:
        if fragment not in compact_checkout:
            errors.append(f"sealed checkout origin lost {fragment!r}")
    if compact_checkout.count("LeaseToken{") != 1 or compact_checkout.count("ActiveWork{") != 1:
        errors.append("checkout must construct exactly one worker token and one ActiveWork")

    authority_production = [
        path
        for path in production_rust_sources()
        if path.is_relative_to(TX_POOL_AUTHORITY_PLAN.parent)
    ]
    checkout_callers: list[str] = []
    for path in authority_production:
        masked = mask_rust_non_code(path.read_text())
        call_count = len(re.findall(r"\bCheckedOutWork\s*::\s*from_owner\s*\(", masked))
        checkout_callers.extend([str(path.relative_to(REPO_ROOT))] * call_count)
        if path != TX_POOL_AUTHORITY_WORK and "LeaseToken {" in masked:
            errors.append(
                f"LeaseToken escaped its sole checkout constructor in {path.relative_to(REPO_ROOT)}"
            )
        if path not in (TX_POOL_AUTHORITY_WORK, TX_POOL_AUTHORITY_STATE) and re.search(
            r"\bActiveWork\s*\{[^{}]*\bchain_view\s*:", masked, re.S
        ):
            errors.append(
                f"ActiveWork escaped its sole checkout constructor in {path.relative_to(REPO_ROOT)}"
            )
        if path == TX_POOL_AUTHORITY_STATE and masked.count("ActiveWork {") != 1:
            errors.append("ActiveWork state declaration gained a second constructor")
    expected_checkout_caller = str(TX_POOL_AUTHORITY_COMPUTE_EXCHANGE.relative_to(REPO_ROOT))
    if checkout_callers != [expected_checkout_caller]:
        errors.append(
            "CheckedOutWork::from_owner production caller set changed: "
            f"expected {[expected_checkout_caller]}, found {checkout_callers}"
        )

    compact_final_location = "".join(mask_rust_non_code(final_location).split())
    final_location_literal = re.search(
        r"Some\s*\(\s*Self\s*\{(?P<body>.*?)\}\s*\)",
        mask_rust_non_code(final_location),
        re.DOTALL,
    )
    compact_final_location_literal = (
        "".join(final_location_literal.group("body").split())
        if final_location_literal is not None
        else ""
    )
    guard = compact_final_location.find(
        "self.script.is_reusable_under(context.rules())"
    )
    literal = compact_final_location.find("Some(Self{")
    if (
        guard < 0
        or literal < 0
        or guard >= literal
        or compact_final_location_literal
        != "content:CellContentReceipt::from_resolution(payload),context,metrics,..self"
    ):
        errors.append(
            "final validation must rebind payload location and context in one sealed transition"
        )
    compact_refresh_locations = "".join(
        mask_rust_non_code(refresh_locations).split()
    )
    for fragment in (
        "ResolvedCellRole::Input",
        "ResolvedCellRole::Dependency",
        "ResolvedCellRole::DependencyGroup",
        "ifcurrent!=cell.transaction_info",
        "cell.transaction_info=change.current",
        "check_tx_fee_with_min_fee_rate(",
        "&refreshed,payload.serialized_bytes(),min_fee_rate",
        "payload.with_refreshed_locations(LocationRefreshSeal(()),Arc::new(refreshed),fee)",
        "ReadyPayloadRelation::LocationRefreshed",
    ):
        if fragment not in compact_refresh_locations:
            errors.append(f"pointwise final-location refresh lost {fragment!r}")
    compact_validation_location = "".join(mask_rust_non_code(validation).split())
    final_location_call = (
        "verified.with_final_validation(LocationRefreshSeal(()),payload,context)"
    )
    if compact_validation_location.count(final_location_call) != 1:
        errors.append("the final validator must consume one fused payload/context transition")
    if "matchpayload_relation" in compact_validation_location:
        errors.append("payload relation must not decide whether final evidence is rebound")
    if ".with_context(context)" in compact_validation_location:
        errors.append("production final validation regained a split context-only update")

    final_location_callers: list[str] = []
    for path in authority_production:
        masked = mask_rust_non_code(path.read_text())
        call_count = len(re.findall(r"\.\s*with_final_validation\s*\(", masked))
        final_location_callers.extend([str(path.relative_to(REPO_ROOT))] * call_count)
    expected_final_location_caller = str(TX_POOL_AUTHORITY_VALIDATION.relative_to(REPO_ROOT))
    if final_location_callers != [expected_final_location_caller]:
        errors.append(
            "VerifiedFacts::with_final_validation caller set changed: expected "
            f"{[expected_final_location_caller]}, found {final_location_callers}"
        )

    compact_scheduler = "".join(mask_rust_non_code(scheduler).split())
    ticket_declaration = re.search(
        r"pub\s*\(super\)\s+struct\s+CheckoutTicket\s*\{(?P<body>.*?)\n\}",
        mask_rust_non_code(scheduler),
        re.S,
    )
    if ticket_declaration is None or re.search(r"\bpub\b", ticket_declaration.group("body")):
        errors.append("CheckoutTicket must retain private fields behind the scheduler")
    if compact_scheduler.count("CheckoutTicket{") != 4:
        errors.append(
            "CheckoutTicket must have one declaration, one impl and exactly two scheduler-wave constructors"
        )

    scheduler_capability_fragments = {
        "QueueLane::for_permit": (
            queue_for_permit,
            ("super::state::WorkPermit::VerifyOnly(_)=>Self::Verify",),
        ),
        "QueueLane::population": (
            queue_population,
            (
                "(Self::Verify,VerifyCapability::SmallCycleOnly)=>QueuePopulation::SmallOnly",
            ),
        ),
        "OwnerQueue::head_excluding": (
            owner_head_excluding,
            (
                "VerifyCapability::SmallCycleOnly=>last_available(&self.small,excluded_versions)",
            ),
        ),
        "FairFrontier::slot": (
            frontier_slot,
            (
                "PreAcceptedPhase::Queued(super::state::QueuedWork::Verify(resolved))",
                "class:resolved.verify_class()",
            ),
        ),
        "FairFrontier::next": (
            frontier_next,
            (
                "letlane=QueueLane::for_permit(permit)",
                "letcapability=QueueLane::capability(permit)",
                "next_excluding_with_overlay(added,lane,capability",
                "CheckoutTicket{lane,owner,key:key.clone(),}",
            ),
        ),
        "FairFrontier::next_after": (
            frontier_next_after,
            (
                "letlane=QueueLane::for_permit(permit)",
                "letcapability=QueueLane::capability(permit)",
                "next_excluding_with_overlay(added,lane,capability",
                "CheckoutTicket{lane,owner,key:key.clone(),}",
            ),
        ),
        "TxPoolAuthority::search_exchange_permit": (
            search_exchange_permit,
            (
                "Some(owner)=>wave.next_after(permit,owner)",
                "None=>wave.next(permit)",
                "self.exchange_checkout_resource(owners,resources,&ticket,permit",
                "PlannedAssignment{permit,ticket,reservation,}",
            ),
        ),
        "TxPoolAuthority::exchange_checkout_resource": (
            exchange_checkout_resource,
            (
                "letbefore=owners.current(self,ticket.hash())?",
                "before.record().version!=ticket.version()",
                "letOwnedTx::PreAccepted(preaccepted)=beforeelse",
                "self.checkout_eligibility(preaccepted,permit)?",
            ),
        ),
    }
    for owner, (body, fragments) in scheduler_capability_fragments.items():
        compact_body = "".join(mask_rust_non_code(body).split())
        for fragment in fragments:
            if fragment not in compact_body:
                errors.append(
                    f"sealed checkout scheduler premise {owner} lost {fragment!r}"
                )
    compact_compile_exchange = "".join(
        mask_rust_non_code(compile_compute_exchange_state).split()
    )
    exchange_order = (
        "letPlannedAssignment{permit,ticket,reservation,}=assignment",
        "letkey=ticket.hash().clone()",
        "letOwnedTx::PreAccepted(preaccepted)=&reservation.beforeelse",
        "CheckedOutWork::from_owner(",
    )
    positions = [compact_compile_exchange.find(fragment) for fragment in exchange_order]
    if any(position < 0 for position in positions) or positions != sorted(positions):
        errors.append(
            "compute exchange must carry one scheduler permit/ticket pair into checkout"
        )
    if compact_checkout.count(
        "ifcapability.permits(resolved.verify_class())"
    ) != 1:
        errors.append("checkout lost its sole defensive Verify capability guard")

    policy_declaration = re.search(
        r"enum\s+PayloadPolicyEvolution\s*\{(?P<body>.*?)\n\}",
        mask_rust_non_code(state),
        re.S,
    )
    expected_policy_evolution = {"Unchanged", "RemoteToTrusted", "Invalid"}
    if policy_declaration is None:
        errors.append("the closed settlement payload-policy evolution disappeared")
    else:
        policy_variants = set(
            re.findall(
                r"(?m)^\s*([A-Z][A-Za-z0-9_]*)\s*,?\s*$",
                policy_declaration.group("body"),
            )
        )
        if policy_variants != expected_policy_evolution:
            errors.append(
                "payload-policy evolution changed: expected "
                f"{sorted(expected_policy_evolution)}, found {sorted(policy_variants)}"
            )
    compact_policy_evolution = "".join(mask_rust_non_code(policy_evolution).split())
    policy_fragments = (
        "Self::RemoteDeclaredCycles(active),Self::RemoteDeclaredCycles(current)",
        "ifactive.declared()==current.declared()",
        "Self::Trusted,Self::Trusted",
        "Self::RemoteDeclaredCycles(_),Self::Trusted",
        "Self::Trusted,Self::RemoteDeclaredCycles(_)",
    )
    for fragment in policy_fragments:
        if fragment not in compact_policy_evolution:
            errors.append(f"payload-policy evolution lost {fragment!r}")
    compact_remote_ingress = "".join(mask_rust_non_code(remote_ingress).split())
    if "payload_policy:PayloadPolicy::RemoteDeclaredCycles(declared_limit)" not in compact_remote_ingress:
        errors.append("Remote ingress no longer seals its exact declared-cycle policy")
    compact_ingress = "".join(mask_rust_non_code(ingress).split())
    for fragment in (
        "pub(super)structRemoteCycleLimit(Cycle)",
        "RemoteCycleLimit::checked(declared_cycles,consensus)",
        "declared<=consensus.max_block_cycles()",
    ):
        if fragment not in compact_ingress:
            errors.append(f"Remote ingress lost checked d <= M fragment {fragment!r}")
    compact_chain_transition = "".join(mask_rust_non_code(plan_chain_transition).split())
    trusted_demotion = (
        "after.source=PreAcceptedSource::Remote(RemoteBase{"
        "residency,payload_policy:PayloadPolicy::Trusted,});"
    )
    if compact_chain_transition.count(trusted_demotion) != 1:
        errors.append("Proposal demotion must preserve one Trusted payload policy")
    if compact_classifier.count("active.payload_policy.evolution_to(current_policy)") != 1:
        errors.append("settlement must consume one closed payload-policy evolution")
    for variant in expected_policy_evolution:
        if compact_classifier.count(f"PayloadPolicyEvolution::{variant}") != 1:
            errors.append(
                f"settlement payload-policy classifier must consume {variant} exactly once"
            )
    remote_base_constructors: list[str] = []
    for path in authority_production:
        if path == TX_POOL_AUTHORITY_STATE:
            continue
        masked = mask_rust_non_code(path.read_text())
        count = len(re.findall(r"\bRemoteBase\s*\{", masked))
        remote_base_constructors.extend([str(path.relative_to(REPO_ROOT))] * count)
        if re.search(r"\.payload_policy\s*=", masked):
            errors.append(
                f"payload policy gained an in-place mutation in {path.relative_to(REPO_ROOT)}"
            )
    expected_remote_base_constructor = str(
        TX_POOL_AUTHORITY_CHAIN_TRANSITION.relative_to(REPO_ROOT)
    )
    if remote_base_constructors != [expected_remote_base_constructor]:
        errors.append(
            "RemoteBase constructor set changed: expected the sole trusted demotion in "
            f"{expected_remote_base_constructor}, found {remote_base_constructors}"
        )

    deferred_declaration = re.search(
        r"(?ms)^\s{4}Deferred\s*\{(?P<body>.*?)^\s{4}\},",
        mask_rust_non_code(compute_exchange),
    )
    if deferred_declaration is None:
        errors.append("ClassifiedCompletion::Deferred disappeared")
    else:
        deferred_fields = set(
            re.findall(
                r"^\s*([a-z_][A-Za-z0-9_]*)\s*:",
                deferred_declaration.group("body"),
                re.M,
            )
        )
        expected_deferred_fields = {"slot", "settlement", "aftermath", "route"}
        if deferred_fields != expected_deferred_fields:
            errors.append(
                "deferred completion split its move-only settlement: expected "
                f"{sorted(expected_deferred_fields)}, found {sorted(deferred_fields)}"
            )
    compact_defer = "".join(mask_rust_non_code(defer_completion).split())
    if (
        "let(settlement,aftermath)=finished.into_parts();" not in compact_defer
        or "ClassifiedCompletion::Deferred{slot,settlement,aftermath,route,}"
        not in compact_defer
        or "letComputeSettlement{" in compact_defer
    ):
        errors.append("deferred completion must move the original settlement without splitting it")
    recovered_settlement = "AuthorityFinishedCompute::from_parts(settlement,aftermath)"
    if "".join(mask_rust_non_code(recover_classified).split()).count(recovered_settlement) != 1:
        errors.append("failed exchange recovery must return the intact deferred settlement")
    if "".join(mask_rust_non_code(apply_exchange).split()).count(recovered_settlement) != 1:
        errors.append("committed exchange deferral must return the intact deferred settlement")

    settlement = re.search(
        r"enum\s+SettlementNext\s*\{(?P<body>.*?)\n\}",
        mask_rust_non_code(work),
        re.S,
    )
    if settlement is None:
        errors.append("the closed SettlementNext result enum disappeared")
    else:
        variants = re.findall(
            r"(?m)^\s*([A-Z][A-Za-z0-9_]*)\b", settlement.group("body")
        )
        for variant in variants:
            if compact_classifier.count(f"SettlementNext::{variant}") != 1:
                errors.append(
                    f"the total settlement classifier must consume {variant} exactly once"
                )
        if "_=>" in compact_classifier[result_match:]:
            errors.append("the settlement classifier must remain exhaustive without a wildcard")

    production_outside_authority = "\n".join(
        path.read_text()
        for path in production_rust_sources()
        if not path.is_relative_to(TX_POOL_SRC / "authority")
    )
    if "ComputeSettlement {" in production_outside_authority:
        errors.append("move-only compute settlements must not be assembled outside the authority")
    production_without_work = "\n".join(
        path.read_text()
        for path in production_rust_sources()
        if path != TX_POOL_AUTHORITY_WORK
    )
    evidence_constructors = {
        "ResolvedFacts::from_resolution(": 2,
        "VerifiedFacts::from_verification(": 1,
    }
    for constructor, expected_count in evidence_constructors.items():
        if constructor in production_without_work:
            errors.append(f"sealed settlement evidence escaped authority work via {constructor!r}")
        if work.count(constructor) != expected_count:
            errors.append(
                f"sealed settlement evidence constructor {constructor!r} changed: "
                f"expected {expected_count}, found {work.count(constructor)}"
            )
    return errors


def validate_task_capability_lifecycle() -> list[str]:
    """Bind every default-generation task/channel to one model owner and join cut."""

    required_sources = {
        TX_POOL_BUILDER,
        TX_POOL_AUTHORITY_SERVICE,
        TX_POOL_AUTHORITY_TOPOLOGY,
        TX_POOL_AUTHORITY_WORKER,
        TX_POOL_AUTHORITY_COMPUTE_COORDINATOR,
        TX_POOL_AUTHORITY_TEMPLATE_DRIVER,
        TX_POOL_AUTHORITY_PUBLISHER,
        TX_POOL_BLOCK_ASSEMBLER_NOTIFY,
        TX_POOL_BLOCKING_TEST_SERVICE,
        SYNC_RELAYER_TEST_HELPER,
        RPC_TEST_SETUP,
        RPC_TEST_MOD,
        TX_POOL_MODEL_PROTOCOL,
    }
    try:
        sources = {path: path.read_text() for path in required_sources}
        model_roles = rust_enum_variants(
            sources[TX_POOL_MODEL_PROTOCOL], "GenerationTaskRole"
        )
        worker_roles = rust_enum_variants(
            sources[TX_POOL_AUTHORITY_WORKER], "AuthorityWorkerRole"
        )
        template_roles = rust_enum_variants(
            sources[TX_POOL_AUTHORITY_TEMPLATE_DRIVER], "AuthorityTemplateRole"
        )
        topology_roles = rust_enum_variants(
            sources[TX_POOL_AUTHORITY_TOPOLOGY], "AuthorityTaskRole"
        )
        topology_shutdown = impl_method_body(
            sources[TX_POOL_AUTHORITY_TOPOLOGY], "AuthorityTaskTopology", "shutdown"
        )
        topology_invalidate = impl_method_body(
            sources[TX_POOL_AUTHORITY_TOPOLOGY],
            "AuthorityTaskTopology",
            "invalidate_generation",
        )
        topology_retire = impl_method_body(
            sources[TX_POOL_AUTHORITY_TOPOLOGY],
            "AuthorityTaskTopology",
            "retire_invalid_generation",
        )
        topology_abort_join = impl_method_body(
            sources[TX_POOL_AUTHORITY_TOPOLOGY],
            "AuthorityTaskTopology",
            "abort_and_join_all",
        )
        topology_abort_request = impl_method_body(
            sources[TX_POOL_AUTHORITY_TOPOLOGY],
            "AuthorityTaskTopology",
            "request_abort_all",
        )
        generation_shutdown = impl_method_body(
            sources[TX_POOL_AUTHORITY_SERVICE], "AuthorityGeneration", "shutdown"
        )
        notification = impl_method_body(
            sources[TX_POOL_BLOCK_ASSEMBLER_NOTIFY], "BlockAssembler", "notify"
        )
        notify_if_changed = impl_method_body(
            sources[TX_POOL_AUTHORITY_TEMPLATE_DRIVER],
            "AuthorityBlockAssembler",
            "notify_if_changed",
        )
    except (OSError, ValueError) as error:
        return [f"cannot inspect task/capability lifecycle: {error}"]

    errors: list[str] = []

    def cross_crate_fixture_lifecycle_errors(
        blocking_source: str,
        sync_source: str,
        rpc_setup_source: str,
        rpc_mod_source: str,
    ) -> list[str]:
        fixture_errors: list[str] = []
        masked_blocking = mask_rust_non_code(blocking_source)
        scope = re.search(
            r"pub\s+struct\s+BlockingTxPoolTestScope\s*\{(?P<body>.*?)\n\}",
            masked_blocking,
            re.S,
        )
        if scope is None:
            fixture_errors.append("blocking tx-pool test scope disappeared")
        else:
            fixture_errors.extend(
                require_ordered_fragments(
                    "".join(scope.group("body").split()),
                    "BlockingTxPoolTestScope field lifetime",
                    (
                        "signal:CancellationToken",
                        "runtime:Handle",
                        "dispatcher:Option<JoinHandle<()>>",
                        "relay_results:Option<TxVerificationResultReceiver>",
                    ),
                )
            )
        try:
            transfer = impl_method_body(
                blocking_source,
                "BlockingTxPoolTestScope",
                "take_relay_results",
            )
        except ValueError as error:
            fixture_errors.append(str(error))
        else:
            if "self.relay_results.take()" not in "".join(transfer.split()):
                fixture_errors.append(
                    "blocking tx-pool fixture lost its linear relay receiver transfer"
                )

        masked_sync = mask_rust_non_code(sync_source)
        relayer_scope = re.search(
            r"pub\(crate\)\s+struct\s+RelayerTestScope\s*\{(?P<body>.*?)\n\}",
            masked_sync,
            re.S,
        )
        if relayer_scope is None:
            fixture_errors.append("sync relayer fixture has no aggregate lifetime owner")
        else:
            fixture_errors.extend(
                require_ordered_fragments(
                    "".join(relayer_scope.group("body").split()),
                    "RelayerTestScope drop order",
                    (
                        "_tx_pool:ckb_tx_pool::internal_test_support::BlockingTxPoolTestScope",
                        "_sync_shared:Arc<SyncShared>",
                        "chain:ChainServiceScope",
                    ),
                )
            )
        try:
            build_chain = function_body(sync_source, "build_chain")
        except ValueError as error:
            fixture_errors.append(str(error))
        else:
            compact_build = "".join(mask_rust_non_code(build_chain).split())
            fixture_errors.extend(
                require_ordered_fragments(
                    compact_build,
                    "sync relayer fixture capability flow",
                    (
                        "start_blocking_test_service(",
                        "pack.take_relay_tx_receiver()",
                        "tx_pool.take_relay_results()",
                        "Arc::new(SyncShared::new(shared,Default::default(),relay_results))",
                        "Relayer::new(chain.chain_controller().clone(),Arc::clone(&sync_shared))",
                        "RelayerTestScope{_tx_pool:tx_pool,_sync_shared:sync_shared,chain,}",
                    ),
                )
            )
            if ".take_tx_pool_builder().start(" in compact_build:
                fixture_errors.append(
                    "sync relayer fixture detached the tx-pool generation from database lifetime"
                )

        masked_rpc_mod = mask_rust_non_code(rpc_mod_source)
        rpc_scope = re.search(
            r"pub\(crate\)\s+struct\s+RpcTestSuite\s*\{(?P<body>.*?)\n\}",
            masked_rpc_mod,
            re.S,
        )
        if rpc_scope is None:
            fixture_errors.append("RPC fixture has no aggregate lifetime owner")
        else:
            fixture_errors.extend(
                require_ordered_fragments(
                    "".join(rpc_scope.group("body").split()),
                    "RpcTestSuite drop order",
                    (
                        "chain_controller:ChainController",
                        "_tx_pool_scope:BlockingTxPoolTestScope",
                        "_sync_shared:std::sync::Arc<ckb_sync::SyncShared>",
                        "_chain_scope:ChainServiceScope",
                        "shared:Shared",
                        "_runtime_stop_rx:Receiver<()>",
                        "_runtime:Runtime",
                    ),
                )
            )
        try:
            setup_rpc = function_body(rpc_setup_source, "setup_rpc_test_suite")
        except ValueError as error:
            fixture_errors.append(str(error))
        else:
            compact_rpc = "".join(mask_rust_non_code(setup_rpc).split())
            fixture_errors.extend(
                require_ordered_fragments(
                    compact_rpc,
                    "RPC fixture capability flow",
                    (
                        "start_blocking_test_service(",
                        "pack.take_relay_tx_receiver()",
                        "tx_pool_scope.take_relay_results()",
                        "Arc::new(SyncShared::new(shared.clone(),Default::default(),relay_results,))",
                        ".enable_net(network_controller.clone(),Arc::clone(&sync_shared),Arc::new(chain_controller.clone()),)",
                        "reqwest::blocking::Client::builder().no_proxy().build()",
                        "RpcTestSuite{",
                        "_tx_pool_scope:tx_pool_scope",
                        "_sync_shared:sync_shared",
                        "_chain_scope:chain_scope",
                    ),
                )
            )
            if ".take_tx_pool_builder().start(" in compact_rpc:
                fixture_errors.append(
                    "RPC fixture detached the tx-pool generation from database lifetime"
                )
        return fixture_errors

    blocking_test_service = sources[TX_POOL_BLOCKING_TEST_SERVICE]
    sync_relayer_test_helper = sources[SYNC_RELAYER_TEST_HELPER]
    rpc_test_setup = sources[RPC_TEST_SETUP]
    rpc_test_mod = sources[RPC_TEST_MOD]
    errors.extend(
        cross_crate_fixture_lifecycle_errors(
            blocking_test_service,
            sync_relayer_test_helper,
            rpc_test_setup,
            rpc_test_mod,
        )
    )
    detached_fixture_canary = sync_relayer_test_helper.replace(
        "start_blocking_test_service", "start_detached_test_service", 1
    )
    if not cross_crate_fixture_lifecycle_errors(
        blocking_test_service,
        detached_fixture_canary,
        rpc_test_setup,
        rpc_test_mod,
    ):
        errors.append("cross-crate fixture lifecycle gate failed its detached-task canary")
    reordered_scope_canary = sync_relayer_test_helper.replace(
        "    _tx_pool: ckb_tx_pool::internal_test_support::BlockingTxPoolTestScope,\n"
        "    _sync_shared: Arc<SyncShared>,",
        "    _sync_shared: Arc<SyncShared>,\n"
        "    _tx_pool: ckb_tx_pool::internal_test_support::BlockingTxPoolTestScope,",
        1,
    )
    if not cross_crate_fixture_lifecycle_errors(
        blocking_test_service,
        reordered_scope_canary,
        rpc_test_setup,
        rpc_test_mod,
    ):
        errors.append("cross-crate fixture lifecycle gate failed its drop-order canary")
    detached_rpc_canary = rpc_test_setup.replace(
        "start_blocking_test_service", "start_detached_test_service", 1
    )
    if not cross_crate_fixture_lifecycle_errors(
        blocking_test_service,
        sync_relayer_test_helper,
        detached_rpc_canary,
        rpc_test_mod,
    ):
        errors.append("RPC fixture lifecycle gate failed its detached-task canary")
    ambient_proxy_canary = rpc_test_setup.replace(".no_proxy()", ".use_ambient_proxy()", 1)
    if not cross_crate_fixture_lifecycle_errors(
        blocking_test_service,
        sync_relayer_test_helper,
        ambient_proxy_canary,
        rpc_test_mod,
    ):
        errors.append("RPC fixture boundary gate failed its ambient-proxy canary")
    reordered_rpc_canary = rpc_test_mod.replace(
        "    _tx_pool_scope: BlockingTxPoolTestScope,\n"
        "    _sync_shared: std::sync::Arc<ckb_sync::SyncShared>,",
        "    _sync_shared: std::sync::Arc<ckb_sync::SyncShared>,\n"
        "    _tx_pool_scope: BlockingTxPoolTestScope,",
        1,
    )
    if not cross_crate_fixture_lifecycle_errors(
        blocking_test_service,
        sync_relayer_test_helper,
        rpc_test_setup,
        reordered_rpc_canary,
    ):
        errors.append("RPC fixture lifecycle gate failed its drop-order canary")

    expected_model_roles = [
        "DispatcherRoot",
        "MessageHandler",
        "ChainControl",
        "ComputeCoordinator",
        "ComputeWorker",
        "Ready",
        "Maintenance",
        "EffectPublisher",
        "VerificationCache",
        "TemplateLane",
    ]
    if model_roles != expected_model_roles:
        errors.append(
            "generation task model lost its ordered complete role partition: "
            f"expected {expected_model_roles}, found {model_roles}"
        )
    if worker_roles != [
        "ComputeCoordinator",
        "Resolver",
        "Verifier",
        "Ready",
        "Maintenance",
    ]:
        errors.append(f"authority worker role partition changed: {worker_roles}")
    if template_roles != [
        "Replacement",
        "Proposals",
        "Transactions",
        "Uncles",
        "Notification",
    ]:
        errors.append(f"template task role partition changed: {template_roles}")
    if topology_roles != ["Worker", "EffectPublisher", "VerificationCache", "Template"]:
        errors.append(f"topology task role partition changed: {topology_roles}")

    model = sources[TX_POOL_MODEL_PROTOCOL]
    for owner in (
        "GenerationTaskContract",
        "generation_task_contract",
        "generation_task_disposition",
        "GENERATION_TASK_ROLES",
    ):
        if owner not in model:
            errors.append(f"task lifecycle model owner {owner!r} disappeared")
    for fragment in (
        "ShutdownPhase::Invalidating",
        "ShutdownPhase::AbortRequested",
        "ShutdownPhase::InvalidTasksJoined",
        "ShutdownAction::RequestAbort",
        "ShutdownAction::JoinAbortedTasks",
        "ShutdownAction::ReportPersistenceForbidden",
    ):
        if fragment not in model:
            errors.append(f"invalid shutdown relation lost {fragment!r}")

    default_sources: dict[Path, str] = {}
    try:
        for path in TX_POOL_SRC.rglob("*.rs"):
            relative_parts = path.relative_to(TX_POOL_SRC).parts
            if "tests" in relative_parts or path.name == "benchmark.rs":
                continue
            default_sources[path] = path.read_text()
    except OSError as error:
        errors.append(f"cannot build default tx-pool task census: {error}")
        return errors

    handle_spawn = re.compile(r"\bhandle\s*\.\s*spawn\s*\(")
    handler_spawn = re.compile(r"\bhandlers\s*\.\s*spawn\s*\(")
    raw_tokio_spawn = re.compile(r"\btokio\s*::\s*spawn\s*\(")
    blocking_spawn = re.compile(
        r"\btokio\s*::\s*task\s*::\s*spawn_blocking\s*\("
    )
    mpsc_channel = re.compile(r"\bmpsc\s*::\s*channel\s*\(")
    watch_channel = re.compile(r"\bwatch\s*::\s*channel\s*\(")

    expected_handle_spawns = {
        "tx-pool/src/service/builder.rs": 1,
        "tx-pool/src/authority/service.rs": 1,
        "tx-pool/src/authority/topology.rs": 2,
        "tx-pool/src/authority/worker.rs": 2,
        "tx-pool/src/authority/compute_coordinator.rs": 2,
        "tx-pool/src/authority/template_driver.rs": 3,
    }
    expected_handler_spawns = {"tx-pool/src/service/builder.rs": 2}
    expected_blocking_spawns = {
        "tx-pool/src/service/builder.rs": 1,
        "tx-pool/src/authority/service.rs": 1,
        "tx-pool/src/authority/publisher.rs": 1,
    }
    expected_mpsc_channels = {
        "tx-pool/src/service/builder.rs": 2,
        "tx-pool/src/authority/topology.rs": 1,
        "tx-pool/src/authority/compute_coordinator.rs": 3,
    }
    expected_watch_channels = {"tx-pool/src/authority/service.rs": 1}

    def census(
        source_map: dict[Path, str], pattern: re.Pattern[str]
    ) -> dict[str, int]:
        observed: dict[str, int] = {}
        for path, source in source_map.items():
            count = len(pattern.findall(mask_rust_non_code(source)))
            if count:
                observed[path.relative_to(REPO_ROOT).as_posix()] = count
        return observed

    def spawn_census_errors(source_map: dict[Path, str]) -> list[str]:
        found_errors: list[str] = []
        for label, pattern, expected in (
            ("Handle-owned task", handle_spawn, expected_handle_spawns),
            ("dispatcher JoinSet task", handler_spawn, expected_handler_spawns),
            ("awaited/circuit blocking task", blocking_spawn, expected_blocking_spawns),
            ("bounded mpsc task channel", mpsc_channel, expected_mpsc_channels),
            ("verification watch channel", watch_channel, expected_watch_channels),
        ):
            observed = census(source_map, pattern)
            if observed != expected:
                found_errors.append(
                    f"{label} census differs: expected {expected}, found {observed}"
                )
        raw = census(source_map, raw_tokio_spawn)
        if raw:
            found_errors.append(
                "default tx-pool production contains generation-unowned tokio::spawn "
                f"sites: {raw}"
            )
        return found_errors

    errors.extend(spawn_census_errors(default_sources))
    canary_sources = dict(default_sources)
    canary_sources[TX_POOL_BLOCK_ASSEMBLER_NOTIFY] += (
        "\nfn detached_task_canary() { tokio::spawn(async {}); }\n"
    )
    if not spawn_census_errors(canary_sources):
        errors.append("task-owner census failed its detached-task negative canary")

    def invalid_retirement_errors(body: str, owner: str) -> list[str]:
        dense_body = "".join(body.split())
        relation_errors = require_ordered_fragments(
            dense_body,
            owner,
            (
                "self.begin_shutdown();",
                "self.abort_and_join_all().await;",
            ),
        )
        if "self.request_abort_all();" in dense_body:
            relation_errors.append(
                f"{owner} must not substitute an abort request for joined retirement"
            )
        return relation_errors

    errors.extend(
        invalid_retirement_errors(
            topology_invalidate, "AuthorityTaskTopology::invalidate_generation"
        )
    )
    errors.extend(
        invalid_retirement_errors(
            topology_retire, "AuthorityTaskTopology::retire_invalid_generation"
        )
    )
    invalid_canary = topology_invalidate.replace(
        "self.abort_and_join_all().await;", "self.request_abort_all();", 1
    )
    if not invalid_retirement_errors(invalid_canary, "invalid-retirement canary"):
        errors.append("invalid-retirement gate failed its abort-without-join negative canary")

    dense_shutdown = "".join(topology_shutdown.split())
    if dense_shutdown.count("self.abort_and_join_all().await;") != 2:
        errors.append(
            "clean shutdown must join aborted owners after both authority fault and timeout"
        )
    errors.extend(
        require_ordered_fragments(
            "".join(topology_abort_join.split()),
            "AuthorityTaskTopology::abort_and_join_all",
            (
                "self.request_abort_all();",
                "whileletSome(task)=self.workers.pop()",
                "task.handle.await",
                "ifletSome(templates)=self.templates.as_mut()",
                "ifletSome(task)=slot.take()",
                "join_slot(&mutself.publisher).await",
                "join_slot(&mutself.verification_cache).await",
            ),
        )
    )
    if "".join(topology_abort_join.split()).count("task.handle.await") != 2:
        errors.append("aborted authority and template task owners must each be joined")
    dense_abort_request = "".join(topology_abort_request.split())
    if ".drain(" in dense_abort_request or ".take()" in dense_abort_request:
        errors.append("abort request must retain every JoinHandle for the later join cut")

    dense_generation_shutdown = "".join(generation_shutdown.split())
    for fragment in (
        "topology.invalidate_generation(fault).await",
        "topology.retire_invalid_generation().await",
    ):
        if fragment not in dense_generation_shutdown:
            errors.append(f"generation invalid shutdown lost joined topology route {fragment!r}")

    dense_notification = "".join(notification.split())
    for fragment in (
        "lethttp_notifications=FuturesUnordered::new();",
        "letscript_notifications=FuturesUnordered::new();",
        "futures_util::stream::select(http_notifications,script_notifications)",
        "whilenotifications.next().await.is_some(){}",
        "kill_on_drop(true)",
    ):
        if fragment not in dense_notification:
            errors.append(f"structured notification batch lost {fragment!r}")
    for retired in ("NotifyScriptRunner", "Semaphore", "tokio::spawn"):
        if retired in sources[TX_POOL_BLOCK_ASSEMBLER_NOTIFY]:
            errors.append(f"notification lane regained detached ownership via {retired!r}")
    if "self.assembler.notify().await" not in "".join(notify_if_changed.split()):
        errors.append("template Notification lane must await its complete endpoint batch")

    topology = sources[TX_POOL_AUTHORITY_TOPOLOGY]
    service = sources[TX_POOL_AUTHORITY_SERVICE]
    compute = sources[TX_POOL_AUTHORITY_COMPUTE_COORDINATOR]
    for fragment in (
        "workers: Vec<AuthorityWorkerTask>",
        "templates: Option<[Option<AuthorityTemplateTask>; 5]>",
        "publisher: Option<tokio::task::JoinHandle",
        "verification_cache: Option<tokio::task::JoinHandle",
    ):
        if fragment not in topology:
            errors.append(f"topology task owner lost stored handle {fragment!r}")
    for fragment in (
        "topology: Option<AuthorityTaskTopology>",
        "chain_control: Option<tokio::task::JoinHandle",
    ):
        if fragment not in service:
            errors.append(f"generation owner lost stored lifecycle edge {fragment!r}")
    for fragment in (
        "AuthorityWorkerFault::completion(error.0)",
        "assignment.into_requeue_completion()",
        "lane.sender = None",
        "self.completions.send(completion).await",
    ):
        if fragment not in compute:
            errors.append(f"compute transport lost exact capability return {fragment!r}")

    return errors


def validate_execution_topology_contract() -> list[str]:
    """Bind current serial cuts, batching costs and shutdown order to source."""

    try:
        builder = TX_POOL_BUILDER.read_text()
        controller = TX_POOL_CONTROLLER.read_text()
        effect = TX_POOL_AUTHORITY_EFFECT.read_text()
        runtime = TX_POOL_AUTHORITY_RUNTIME.read_text()
        scheduler = TX_POOL_AUTHORITY_SCHEDULER.read_text()
        service = TX_POOL_AUTHORITY_SERVICE.read_text()
        compute_coordinator = TX_POOL_AUTHORITY_COMPUTE_COORDINATOR.read_text()
        topology = TX_POOL_AUTHORITY_TOPOLOGY.read_text()
        template_driver = TX_POOL_AUTHORITY_TEMPLATE_DRIVER.read_text()
        worker = TX_POOL_AUTHORITY_WORKER.read_text()
        dispatch = TX_POOL_DISPATCH.read_text()
        message = TX_POOL_MESSAGE.read_text()

        builder_run = impl_method_body(builder, "TxPoolServiceBuilder", "run")
        dispatcher = impl_method_body(
            builder, "TxPoolServiceBuilder", "run_dispatcher"
        )
        signals = impl_method_body(runtime, "AuthoritySignals", "new")
    except (OSError, ValueError) as error:
        return [f"cannot inspect execution topology contract: {error}"]

    errors: list[str] = []

    for fragment in (
        ".max(1)",
        ".checked_mul(MESSAGE_CONCURRENCY_MULTIPLIER)",
    ):
        if fragment not in builder_run:
            errors.append(f"dispatcher concurrency formula lost {fragment!r}")
    for fragment in (
        "mpsc::channel(DEFAULT_CHANNEL_SIZE)",
        "mpsc::channel(CHAIN_CONTROL_CHANNEL_SIZE)",
    ):
        if fragment not in builder:
            errors.append(f"service assembly lost bounded channel owner {fragment!r}")
    if "handlers.len() < handler_limit" not in dispatcher:
        errors.append("dispatcher must bound live handlers before receiving more work")

    ordered_methods = (
        (runtime, "AuthorityRuntime", "commit_retained_ingress_batch", (
            "plan_retained_admission_batch(&batch)",
            "prepared.apply()",
            "self.publish_committed(retirement)",
        )),
        (runtime, "AuthorityRuntime", "exchange_compute", (
            "apply_compute_exchange(completions, grants)",
            "if let Some(retirement) = retirement",
            "self.publish_committed(retirement)",
        )),
        (runtime, "AuthorityRuntime", "settle", (
            "apply_settlement(settlement)",
            "self.publish_committed(committed)",
        )),
        (runtime, "AuthorityRuntime", "try_drive_ready", (
            "capture_ready_work_batch()",
            ".prepare(self.resolution_policy.min_fee_rate)",
            "complete_ready_batch(prepared)",
            ".validate()",
            "self.publish_committed(committed)",
        )),
        (runtime, "AuthorityRuntime", "settle_effect", (
            "apply_effect_settlement(settlement)",
            "self.publish_committed(retirement)",
        )),
        (runtime, "AuthorityRuntimeConfig", "from_runtime", (
            "config.max_tx_verify_workers.max(1)",
            ".checked_add(1)",
            "transient_compute_permits",
        )),
        (compute_coordinator, None, "spawn_compute_exchange", (
            ".checked_add(3)",
            "AuthorityWorkerRole::Resolver",
            "for worker_id in 0..verifier_count",
            "AuthorityWorkerRole::Verifier(slot.worker_id())",
            "AuthorityWorkerRole::ComputeCoordinator",
        )),
        (worker, "AuthorityRuntime", "spawn_workers", (
            "spawn_compute_exchange(",
            "AuthorityWorkerRole::Ready",
            "AuthorityWorkerRole::Maintenance",
        )),
        (runtime, "AuthorityRuntime", "apply_chain_update", (
            "self.store.upgradable_read()",
            "chain_validation_work_from_view(facts)",
            "work.validate(&command.snapshot)",
            "AuthorityStoreLock::upgrade(store)",
            "plan_chain_transition(receipt)",
            "plan.apply()",
            "publish_post_commit(post_commit)",
        )),
        (builder, "TxPoolServiceBuilder", "run_dispatcher", (
            "receiver.close()",
            "while receiver.try_recv().is_ok()",
            "generation.shutdown(handler_timeout).await",
            "if let Err(error) = service.save_pool().await",
        )),
        (service, "AuthorityGeneration", "begin_shutdown", (
            "topology.begin_shutdown()",
            "self.cancel.cancel()",
        )),
        (service, "AuthorityGeneration", "shutdown", (
            "self.begin_shutdown()",
            "self.chain_control.take()",
            "topology.shutdown(timeout).await",
        )),
        (topology, "AuthorityTaskTopology", "shutdown_authority", (
            "self.join_authority_workers().await",
            "self.runtime.close_effects()",
            "self.join_publisher().await",
            "self.runtime.effects_closed_and_drained()",
        )),
        (topology, "AuthorityTaskTopology", "shutdown", (
            "self.shutdown_authority()",
            "self.join_templates(timeout, &mut derived_failures).await",
            "self.join_verification_cache(timeout, &mut derived_failures)",
        )),
        (topology, "AuthorityTaskTopology", "start", (
            ".spawn_workers(",
            "run_verification_cache_updates",
            "run_claimed_authority_effect_publisher",
            ".spawn_drivers(",
        )),
        (service, "AuthorityService", "save_pool", (
            "self.persistence_writer.acquire().await",
            ".persistence_receipt()",
            ".into_parent_first()",
            "spawn_blocking(move || writer.write(&base, snapshot))",
        )),
    )
    for source, impl_name, method, fragments in ordered_methods:
        owner = method if impl_name is None else f"{impl_name}::{method}"
        if impl_name is None:
            body = function_body(source, method)
            if body is None:
                errors.append(f"expected one {method} function, found 0")
                continue
        else:
            try:
                body = impl_method_body(source, impl_name, method)
            except ValueError as error:
                errors.append(str(error))
                continue
        errors.extend(require_ordered_fragments(body, owner, fragments))
    if re.search(r"\bfn\s+commit_retained_ingress\s*\(", runtime):
        errors.append(
            "retained ingress must not regain a second single-item production kernel"
        )
    if re.search(r"\bfn\s+submit_proposal\s*\(", service):
        errors.append(
            "proposal ingress must not regain a second single-item service adapter"
        )
    if re.search(r"impl\s+IntoIterator\s+for\s+NotifyTxBatch", message):
        errors.append(
            "the validated proposal batch must not expose implicit per-item consumption"
        )
    proposal_dispatch = function_body(dispatch, "process")
    if proposal_dispatch is None:
        errors.append("the exhaustive service message dispatcher disappeared")
    else:
        if "submit_proposal_batch(arguments.into_transactions())" not in proposal_dispatch:
            errors.append("proposal notification fallback must retain one batch submission")
        if re.search(r"for\s+transaction\s+in\s+arguments", proposal_dispatch):
            errors.append("proposal notification fallback must not restore per-item Apply")
    if ".take(MAX_READY_BATCH)" not in scheduler:
        errors.append("Ready capture must consume the named bounded batch limit")
    if signals.count("Notify::new()") != 6:
        errors.append("AuthoritySignals must own exactly six coalescing wake hints")
    if worker.count("role: AuthorityWorkerRole::Ready") != 1:
        errors.append("Ready must retain exactly one independently spawned driver")
    if topology.count("run_claimed_authority_effect_publisher(") != 1:
        errors.append("Ready and effects must retain one separate claimed publisher task")
    if "templates: Option<[Option<AuthorityTemplateTask>; 5]>" not in topology:
        errors.append("template topology must retain five independently joined lanes")
    try:
        template_spawn = impl_method_body(
            template_driver, "AuthorityBlockAssembler", "spawn_drivers"
        )
    except ValueError as error:
        errors.append(str(error))
    else:
        errors.extend(
            require_ordered_fragments(
                template_spawn,
                "AuthorityBlockAssembler::spawn_drivers",
                (
                    "run_replacement_lane(cancel)",
                    "TemplateComponent::Proposals",
                    "TemplateComponent::Transactions",
                    "TemplateComponent::Uncles",
                    "run_notification_lane(cancel, enabled)",
                    "tasks: [replacement, proposals, transactions, uncles, notification]",
                ),
            )
        )
    if "mpsc::channel(VERIFY_CACHE_CHANNEL_SIZE)" not in topology:
        errors.append("verification-cache updates must retain their named bounded channel")
    if topology.count("run_verification_cache_updates(") != 2:
        errors.append("verification cache must retain one task entry and one implementation")
    if compute_coordinator.count("self.cache_updates.try_send(update)") != 1:
        errors.append("retained verification cache publication must remain best-effort and unique")
    if re.search(r"cache_updates\s*\.\s*send\s*\(", compute_coordinator):
        errors.append("retained compute must not await the derived verification cache writer")
    if controller.count("chain_control_sender.send(command)") != 2:
        errors.append(
            "service-residency model requires exactly one trusted reorg send and one "
            "typed public-administration send"
        )
    for owner in ("EffectReceipt", "EffectSettlement"):
        declaration = re.search(
            rf"struct\s+{owner}\s*\{{(?P<body>.*?)\n\}}", effect, re.S
        )
        if declaration is None or "batch: Arc<EffectBatch>" not in declaration.group("body"):
            errors.append(f"{owner} must retain one complete immutable EffectBatch")

    return errors


def historical_bidirectional_coverage_errors(
    contract: dict,
    registry: dict,
    manifest: dict,
    *,
    verify_sources: bool,
) -> list[str]:
    """Derive both history-to-owner and semantic-role-to-rule differences."""

    errors: list[str] = []
    history = contract.get("historical_convergence")
    if not isinstance(history, dict) or set(history) != {
        "schema_version",
        "law",
        "normalization_sources",
        "findings",
    }:
        return ["historical convergence contract has an invalid shape"]
    if history.get("schema_version") != 1:
        errors.append("historical convergence schema_version must be 1")
    if not re.fullmatch(r"[a-z][a-z0-9_]+", str(history.get("law"))):
        errors.append("historical convergence law must be one stable identifier")

    sources = history.get("normalization_sources")
    source_ids: list[str] = []
    if not isinstance(sources, list) or not sources:
        errors.append("historical convergence has no content-addressed source universe")
        sources = []
    for source in sources:
        if not isinstance(source, dict) or set(source) != {"id", "git_blob", "sha256"}:
            errors.append(f"historical normalization source has invalid fields: {source!r}")
            continue
        source_id = source.get("id")
        blob = source.get("git_blob")
        expected_sha = source.get("sha256")
        if not isinstance(source_id, str) or not source_id:
            errors.append("historical normalization source has no id")
        else:
            source_ids.append(source_id)
        if not isinstance(blob, str) or not re.fullmatch(r"[0-9a-f]{40}", blob):
            errors.append(f"historical normalization source {source_id!r} has invalid Git blob")
            continue
        if not isinstance(expected_sha, str) or not re.fullmatch(
            r"[0-9a-f]{64}", expected_sha
        ):
            errors.append(f"historical normalization source {source_id!r} has invalid SHA-256")
            continue
        if verify_sources:
            result = subprocess.run(
                ["git", "cat-file", "-p", blob],
                cwd=REPO_ROOT,
                capture_output=True,
                check=False,
            )
            if result.returncode != 0:
                errors.append(
                    f"historical normalization source {source_id!r} is unavailable from Git"
                )
            elif hashlib.sha256(result.stdout).hexdigest() != expected_sha:
                errors.append(
                    f"historical normalization source {source_id!r} content hash differs"
                )
    if len(source_ids) != len(set(source_ids)):
        errors.append("historical normalization source ids are not unique")

    behaviors = {
        row.get("id"): row
        for row in registry.get("behaviors", [])
        if isinstance(row, dict) and isinstance(row.get("id"), str)
    }
    unit_evidence = {
        row.get("test"): row
        for row in registry.get("unit_evidence", [])
        if isinstance(row, dict) and isinstance(row.get("test"), str)
    }
    known_roots = set(contract.get("root_families", {}))
    known_invariants = set(contract.get("target_invariants", {}))
    checker_cache: dict[str, str] = {}

    def checker_ref_error(reference: str) -> str | None:
        target = reference.removeprefix("checker:")
        path_value, separator, symbol = target.partition("::")
        if (
            not separator
            or not path_value.startswith("tx-pool/scripts/check_")
            or not path_value.endswith(".py")
            or re.fullmatch(r"[a-z_][a-z0-9_]*", symbol) is None
        ):
            return f"invalid historical checker reference {reference!r}"
        source = checker_cache.get(path_value)
        if source is None:
            path = (REPO_ROOT / path_value).resolve()
            try:
                path.relative_to(REPO_ROOT)
                source = path.read_text()
            except (OSError, ValueError) as error:
                return f"cannot read historical checker reference {reference!r}: {error}"
            checker_cache[path_value] = source
        if re.search(rf"^def\s+{re.escape(symbol)}\s*\(", source, re.M) is None:
            return f"historical checker owner {reference!r} has no definition"
        if len(re.findall(rf"\b{re.escape(symbol)}\s*\(", source)) < 2:
            return f"historical checker owner {reference!r} is not called"
        return None

    findings = history.get("findings")
    finding_ids: list[str] = []
    covered_roots: set[str] = set()
    historical_invariants: set[str] = set()
    allowed_dispositions = {
        "confirmed_closed",
        "superseded_by_proven_model",
        "suppressed_with_current_counterevidence",
        "superseded_as_release_evidence",
    }
    required_finding_fields = {
        "id",
        "law",
        "falsifier",
        "disposition",
        "root_family_refs",
        "owner_refs",
        "evidence_refs",
    }
    if not isinstance(findings, list) or not findings:
        errors.append("historical convergence has no retained finding universe")
        findings = []
    for finding in findings:
        if not isinstance(finding, dict) or set(finding) != required_finding_fields:
            errors.append(f"historical finding has invalid fields: {finding!r}")
            continue
        finding_id = finding.get("id")
        if not isinstance(finding_id, str) or re.fullmatch(
            r"HF-[A-Z0-9-]+", finding_id
        ) is None:
            errors.append(f"historical finding has invalid id {finding_id!r}")
            continue
        finding_ids.append(finding_id)
        for field in ("law", "falsifier"):
            if re.fullmatch(r"[a-z][a-z0-9_]+", str(finding.get(field))) is None:
                errors.append(f"historical finding {finding_id} has invalid {field}")
        if finding.get("disposition") not in allowed_dispositions:
            errors.append(f"historical finding {finding_id} has invalid disposition")

        roots = finding.get("root_family_refs")
        if (
            not isinstance(roots, list)
            or not roots
            or not all(isinstance(root, str) and root for root in roots)
            or len(roots) != len(set(roots))
        ):
            errors.append(f"historical finding {finding_id} has invalid root refs")
            roots = []
        if roots == ["*"]:
            covered_roots.update(known_roots)
        elif "*" in roots or set(roots).difference(known_roots):
            errors.append(f"historical finding {finding_id} has unknown root refs")
        else:
            covered_roots.update(roots)

        owner_refs = finding.get("owner_refs")
        evidence_refs = finding.get("evidence_refs")
        for field, refs in (("owner", owner_refs), ("evidence", evidence_refs)):
            if (
                not isinstance(refs, list)
                or not refs
                or not all(isinstance(ref, str) and ref for ref in refs)
                or len(refs) != len(set(refs))
            ):
                errors.append(f"historical finding {finding_id} has invalid {field} refs")
        if not isinstance(owner_refs, list) or not isinstance(evidence_refs, list):
            continue
        owner_behaviors: set[str] = set()
        owner_checkers: set[str] = set()
        for reference in owner_refs:
            if reference.startswith("behavior:"):
                behavior = reference.removeprefix("behavior:")
                if behavior not in behaviors:
                    errors.append(
                        f"historical finding {finding_id} has unknown behavior owner {behavior!r}"
                    )
                else:
                    owner_behaviors.add(behavior)
            elif reference.startswith("checker:"):
                checker_error = checker_ref_error(reference)
                if checker_error is not None:
                    errors.append(checker_error)
                else:
                    owner_checkers.add(reference)
            else:
                errors.append(
                    f"historical finding {finding_id} has untyped owner ref {reference!r}"
                )
        for reference in evidence_refs:
            if reference.startswith("test:"):
                test = reference.removeprefix("test:")
                evidence = unit_evidence.get(test)
                if evidence is None:
                    errors.append(
                        f"historical finding {finding_id} has unknown test evidence {test!r}"
                    )
                    continue
                if evidence.get("behavior_id") not in owner_behaviors:
                    errors.append(
                        f"historical finding {finding_id} test {test!r} is outside its owners"
                    )
                historical_invariants.update(evidence.get("invariants", []))
            elif reference.startswith("checker:"):
                checker_error = checker_ref_error(reference)
                if checker_error is not None:
                    errors.append(checker_error)
                elif reference not in owner_checkers:
                    errors.append(
                        f"historical finding {finding_id} checker evidence is not its owner"
                    )
            else:
                errors.append(
                    f"historical finding {finding_id} has untyped evidence ref {reference!r}"
                )
    if len(finding_ids) != len(set(finding_ids)):
        errors.append("historical finding ids are not unique")
    if covered_roots != known_roots:
        errors.append(
            "historical top-down root difference is nonempty: "
            f"missing={sorted(known_roots - covered_roots)}, "
            f"unknown={sorted(covered_roots - known_roots)}"
        )
    required_composition = {"T14", "T15", "T16"}
    if not required_composition.issubset(historical_invariants):
        errors.append(
            "historical T14-T16 evidence difference is nonempty: "
            f"{sorted(required_composition - historical_invariants)}"
        )

    refinement = contract.get("refinement_inventory", {})
    model_roles = set(refinement.get("model_roots", {}).values())
    production_roles = set(refinement.get("production_roots", {}).values())
    bound_model: set[str] = set()
    bound_production: set[str] = set()
    bound_behaviors: set[str] = set()
    for binding in refinement.get("semantic_bindings", {}).values():
        if not isinstance(binding, dict):
            continue
        bound_model.update(binding.get("model_roles", []))
        bound_production.update(binding.get("production_roles", []))
        bound_behaviors.update(binding.get("behavior_ids", []))
    for label, known, bound in (
        ("model role", model_roles, bound_model),
        ("production role", production_roles, bound_production),
        ("current behavior", set(behaviors), bound_behaviors),
    ):
        if known != bound:
            errors.append(
                f"historical bottom-up {label} difference is nonempty: "
                f"unowned={sorted(known - bound)}, unknown={sorted(bound - known)}"
            )

    historical_cases = contract.get("historical_external_regression_samples", {}).get(
        "cases"
    )
    if not isinstance(historical_cases, list) or not historical_cases:
        errors.append("historical external counterexample catalog is empty")
    else:
        for case in historical_cases:
            if not isinstance(case, dict):
                errors.append("historical external counterexample has invalid shape")
                continue
            if not set(case.get("current_behaviors", [])).issubset(behaviors):
                errors.append(
                    f"historical external case {case.get('id')!r} has no current owner"
                )

    construction_roots = manifest.get("construction_root_families")
    rank = manifest.get("convergence_status", {}).get("construction_rank", {})
    dispositions = manifest.get("convergence_status", {}).get(
        "release_law_dispositions", {}
    )
    construction_rows = construction_roots if isinstance(construction_roots, list) else []
    root_members = {
        member
        for row in construction_rows
        if isinstance(row, dict)
        for member in row.get("members", [])
    }
    classified = set(rank.get("open_release_laws", [])).union(dispositions)
    if root_members != classified:
        errors.append(
            "historical post-restart release-law difference is nonempty: "
            f"unclassified={sorted(root_members - classified)}, "
            f"unknown={sorted(classified - root_members)}"
        )
    if not known_invariants.issuperset(historical_invariants):
        errors.append("historical evidence references an unknown invariant")
    return errors


def validate_historical_bidirectional_coverage() -> list[str]:
    """Bind audited history and every current semantic role without prose copies."""

    try:
        contract = json.loads(TX_POOL_ARCHITECTURE_CONTRACT.read_text())
        registry = json.loads(TX_POOL_BEHAVIOR_REGISTRY.read_text())
        manifest = json.loads(TX_POOL_SECURITY_MANIFEST.read_text())
    except (OSError, json.JSONDecodeError) as error:
        return [f"cannot load historical convergence inputs: {error}"]
    errors = historical_bidirectional_coverage_errors(
        contract, registry, manifest, verify_sources=True
    )

    missing_owner = copy.deepcopy(contract)
    missing_owner["historical_convergence"]["findings"][0]["owner_refs"] = []
    observed = historical_bidirectional_coverage_errors(
        missing_owner, registry, manifest, verify_sources=False
    )
    if not any("invalid owner refs" in error for error in observed):
        errors.append("historical convergence canary admitted an unowned finding")

    unbound_behavior = copy.deepcopy(contract)
    for binding in unbound_behavior["refinement_inventory"]["semantic_bindings"].values():
        if "TP-CACHE-001" in binding["behavior_ids"]:
            binding["behavior_ids"].remove("TP-CACHE-001")
    observed = historical_bidirectional_coverage_errors(
        unbound_behavior, registry, manifest, verify_sources=False
    )
    if not any("current behavior difference is nonempty" in error for error in observed):
        errors.append("historical convergence canary admitted an unbound current rule")

    missing_composition = copy.deepcopy(contract)
    performance = next(
        row
        for row in missing_composition["historical_convergence"]["findings"]
        if row["id"] == "HF-PRE-WITNESS-PERFORMANCE"
    )
    performance["evidence_refs"] = [
        "test:mathematical_model::topology_properties::model_exchange_cost_names_its_task_channel_and_failure_price"
    ]
    observed = historical_bidirectional_coverage_errors(
        missing_composition, registry, manifest, verify_sources=False
    )
    if not any("T14-T16 evidence difference is nonempty" in error for error in observed):
        errors.append("historical convergence canary admitted missing composition evidence")
    return errors


def production_rust_sources() -> list[Path]:
    sources: list[Path] = []
    excluded = {".git", "target", "test", "tests", "benches"}
    for root, directories, files in os.walk(REPO_ROOT):
        directories[:] = [name for name in directories if name not in excluded]
        base = Path(root)
        sources.extend(base / name for name in files if name.endswith(".rs"))
    return sources


def validate_tx_pool_module_reachability() -> list[str]:
    """Reject Rust source files that are absent from the crate module graph."""

    root = TX_POOL_SRC / "lib.rs"
    pending = [root]
    reachable: set[Path] = set()
    errors: list[str] = []

    while pending:
        source = pending.pop()
        source = source.resolve()
        if source in reachable:
            continue
        reachable.add(source)
        try:
            raw = source.read_text()
            masked = mask_rust_non_code(raw)
        except (OSError, ValueError) as error:
            errors.append(
                f"cannot inspect Rust module source {source.relative_to(REPO_ROOT)}: {error}"
            )
            continue

        module_root = (
            source.parent
            if source.name in {"lib.rs", "main.rs", "mod.rs"}
            else source.with_suffix("")
        )
        for declaration in RUST_FILE_MODULE.finditer(masked):
            prefix = raw[: declaration.start()]
            attributes = re.search(
                r"(?P<attributes>(?:\s*#\s*\[[^]]*\]\s*)*)$", prefix
            )
            paths = (
                RUST_PATH_ATTRIBUTE.findall(attributes.group("attributes"))
                if attributes is not None
                else []
            )
            if len(paths) > 1:
                errors.append(
                    f"module {declaration.group('name')} in "
                    f"{source.relative_to(REPO_ROOT)} has multiple path attributes"
                )
                continue
            if paths:
                candidates = [(source.parent / paths[0]).resolve()]
            else:
                name = declaration.group("name")
                candidates = [
                    (module_root / f"{name}.rs").resolve(),
                    (module_root / name / "mod.rs").resolve(),
                ]
            existing = [candidate for candidate in candidates if candidate.is_file()]
            if len(existing) != 1:
                rendered = ", ".join(
                    candidate.relative_to(REPO_ROOT).as_posix()
                    if candidate.is_relative_to(REPO_ROOT)
                    else candidate.as_posix()
                    for candidate in candidates
                )
                errors.append(
                    f"module {declaration.group('name')} in "
                    f"{source.relative_to(REPO_ROOT)} resolves to {len(existing)} files: "
                    f"{rendered}"
                )
                continue
            pending.append(existing[0])

    discovered = {source.resolve() for source in TX_POOL_SRC.rglob("*.rs")}
    for source in sorted(discovered - reachable):
        errors.append(
            "Rust source is absent from the tx-pool module graph: "
            f"{source.relative_to(REPO_ROOT)}"
        )
    return errors


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
            "the ordered tx-pool chain-control lane must have one reorg publisher, "
            f"found {reorg_publishers}"
        )
    if chain.count("self.install_chain_tip_transition(&fork, new_snapshot);") != 2:
        errors.append(
            "normal best-block and truncate paths must both install their transition "
            "through install_chain_tip_transition"
        )
    return errors


def proposal_history_provenance_errors(
    chain_model: str,
    proposal_table: str,
    block_verifier: str,
    contextual_verifier: str,
    contextual_uncles_verifier: str,
    runtime: str,
    validation: str,
    boundary: str,
    planner: str,
    controller: str,
    chain_service: str,
    chain_verify: str,
    shared_builder: str,
    ckb_setup: str,
    ckb_replay: str,
    verification_traits: str,
    indexes: str,
) -> list[str]:
    """Prove one snapshot-derived proposal-status and transition-delta path."""

    errors: list[str] = []

    def compact(source: str) -> str:
        return "".join(mask_rust_non_code(source).split())

    try:
        position_variants = rust_enum_variants(chain_model, "ProposalWindowPosition")
        candidate_variants = rust_enum_variants(boundary, "CandidateUncleCollection")
        table_fields = rust_type_body(proposal_table, "struct", "ProposalTable") or ""
        view_fields = rust_type_body(proposal_table, "struct", "ProposalView") or ""
        canonicalize_ids = impl_method_body(
            proposal_table, "ProposalKey", "sorted_unique"
        )
        table_insert = impl_method_body(proposal_table, "ProposalTable", "insert")
        successor_view = impl_method_body(
            proposal_table, "ProposalTable", "successor_view"
        )
        same_identity = impl_method_body(
            proposal_table, "ProposalView", "same_identity"
        )
        height_position = impl_method_body(
            proposal_table, "ProposalTable", "height_position"
        )
        table_finalize = impl_method_body(proposal_table, "ProposalTable", "finalize")
        changed_keys = impl_method_body(
            proposal_table, "ProposalView", "try_for_each_changed_key_from"
        )
        two_phase_verify = impl_method_body(
            contextual_verifier, "TwoPhaseCommitVerifier", "verify"
        )
        position = function_body(chain_model, "proposal_window_position") or ""
        receipt = function_body(chain_model, "proposal_context_receipt") or ""
        receipt_from_position = impl_method_body(
            chain_model, "ProposalContextReceipt", "from_position"
        )
        membership = impl_method_body(
            chain_model, "MembershipReceipt", "from_validation"
        )
        transition = impl_method_body(
            chain_model, "ProposalTransitionFacts", "between"
        )
        apply_chain = impl_method_body(runtime, "AuthorityRuntime", "apply_chain_update")
        plan_chain = impl_method_body(
            planner, "TxPoolAuthority", "chain_validation_work_from_view"
        )
    except ValueError as error:
        return [str(error)]

    dense_table_fields = compact(table_fields)
    dense_view_fields = compact(view_fields)
    dense_proposal_table = compact(proposal_table)
    if "table:BTreeMap<BlockNumber,Box<[ProposalKey]>>" not in dense_table_fields:
        errors.append(
            "ProposalTable primitive history must use deterministic sorted inline-id slices"
        )
    if "state:Arc<ProposalViewState>" not in dense_view_fields:
        errors.append(
            "ProposalView must share one immutable exact counted state"
        )
    if "counts:OrdMap<ProposalKey,BandCounts>" not in dense_proposal_table:
        errors.append(
            "ProposalViewState must retain the structurally shared exact counted projection"
        )
    if (
        "receipt:Option<ProposalTransitionReceipt>" not in dense_proposal_table
        or "predecessor:Weak<ProposalViewState>" not in dense_proposal_table
    ):
        errors.append(
            "ProposalViewState must own one sparse receipt bound to the exact predecessor allocation"
        )
    if compact(same_identity) != "state.as_ptr()==Arc::as_ptr(&self.state)":
        errors.append(
            "proposal sparse receipts must compare the exact predecessor allocation"
        )
    if "structProposalKey([u8;10]);" not in dense_proposal_table:
        errors.append("proposal history keys must retain exactly the 10-byte protocol identity")
    if "HashSet" in dense_proposal_table:
        errors.append(
            "proposal history regained an expected-bound randomized hash representation"
        )

    dense_canonicalize_ids = compact(canonicalize_ids)
    canonicalize_fragments = (
        "ids.into_iter()",
        ".map(|id|Self::from_packed(&id))",
        ".collect::<Vec<_>>()",
        "ids.sort_unstable();",
        "ids.dedup();",
    )
    canonicalize_positions = [
        dense_canonicalize_ids.find(fragment) for fragment in canonicalize_fragments
    ]
    if any(position < 0 for position in canonicalize_positions) or canonicalize_positions != sorted(
        canonicalize_positions
    ):
        errors.append(
            "proposal ids must have one shared sorted/deduplicated inline canonicalizer"
        )
    dense_insert = compact(table_insert)
    if (
        "letids=ProposalKey::sorted_unique(ids);" not in dense_insert
        or "self.table.insert(number,ids.into_boxed_slice())" not in dense_insert
        or dense_proposal_table.count("ProposalKey::sorted_unique(") != 3
    ):
        errors.append(
            "ProposalView construction and ProposalTable insertion must share one deterministic canonicalizer"
        )

    dense_successor_view = compact(successor_view)
    successor_fragments = (
        "letmutheights=[",
        "heights.sort_unstable();",
        "letmutprevious_height=None;",
        "ifprevious_height==Some(height)",
        "touched.sort_unstable();",
        "touched.dedup();",
        "touched.retain(|id|{origin.key_position(*id)!=ProposalView::position_in_counts(&counts,*id)});",
        "changed:touched",
    )
    successor_positions = [
        dense_successor_view.find(fragment) for fragment in successor_fragments
    ]
    if any(position < 0 for position in successor_positions) or successor_positions != sorted(
        successor_positions
    ):
        errors.append(
            "ordinary proposal succession must classify one fixed boundary array and publish its single canonical touched vector"
        )
    if "changed:Vec<ProposalKey>" not in dense_proposal_table:
        errors.append(
            "the shared transition receipt must own exactly one changed-id vector"
        )
    if "predecessor:Arc::downgrade(&origin.state)" not in dense_successor_view:
        errors.append(
            "ordinary proposal succession must seal its exact predecessor allocation"
        )

    dense_block_verifier = compact(block_verifier)
    dense_uncles_verifier = compact(contextual_uncles_verifier)
    dense_chain_service = compact(chain_service)
    dense_shared_builder = compact(shared_builder)
    dense_chain_verify = compact(chain_verify)
    dense_ckb_setup = compact(ckb_setup)
    dense_ckb_replay = compact(ckb_replay)
    dense_verification_traits = compact(verification_traits)
    if (
        "BlockProposalsLimitVerifier::new(max_block_proposals_limit).verify(target)?;"
        not in dense_block_verifier
        or "BlockVerifier::new(consensus).verify(block)" not in dense_chain_service
    ):
        errors.append(
            "canonical proposal history must be downstream of the validated main-block proposal bound"
        )
    uncle_bound_fragments = (
        "letmax_uncles_num=self.provider.consensus().max_uncles_num()asu32;",
        "ifuncle.data().proposals().len()>self.provider.consensus().max_block_proposals_limit()asusize",
    )
    if any(fragment not in dense_uncles_verifier for fragment in uncle_bound_fragments):
        errors.append(
            "canonical proposal history must be downstream of validated uncle-count and per-uncle proposal bounds"
        )
    if (
        "UnclesVerifier::new(uncle_verifier_context,block).verify()?;"
        not in compact(contextual_verifier)
        or "self.proposal_table.insert(blk.header().number(),blk.union_proposal_ids_iter());"
        not in dense_chain_verify
        or "proposal_ids.insert(bn,block_ids.chain(uncle_ids));" not in dense_shared_builder
    ):
        errors.append(
            "proposal projection inputs must come from canonical main/uncle history or its startup replay"
        )
    normal_and_operator_switch_fragments = (
        "Switch::DISABLE_SCRIPT",
        "ifmatches.get_flag(cli::ARG_SKIP_ALL_VERIFY){Switch::DISABLE_ALL}",
        "iffull_verification{Switch::NONE}else{Switch::DISABLE_ALL-Switch::DISABLE_NON_CONTEXTUAL}",
        "Self::DISABLE_UNCLES.bits()|Self::DISABLE_TWO_PHASE_COMMIT.bits()",
        "Self::DISABLE_NON_CONTEXTUAL.bits()|Self::DISABLE_SCRIPT.bits()",
    )
    switch_sources = (
        dense_chain_verify,
        dense_ckb_setup,
        dense_ckb_replay,
        dense_verification_traits,
    )
    if any(
        not any(fragment in source for source in switch_sources)
        for fragment in normal_and_operator_switch_fragments
    ):
        errors.append(
            "proposal history must distinguish normal/assume-valid consensus-bounded input from explicit trusted operator verification bypasses"
        )

    dense_height_position = compact(height_position)
    if (
        "letSome(candidate)=tip.checked_add(1)else" not in dense_height_position
        or "tip+1" in dense_height_position
    ):
        errors.append("proposal candidate height must be a total checked successor")

    dense_changed_keys = compact(changed_keys)
    changed_fragments = (
        "ifletSome(receipt)=&self.state.receipt",
        "predecessor.same_identity(",
        "returnOk(ProposalTransitionSource::AuthenticatedSparse);",
        "letmutold=predecessor.state.counts.iter().peekable();",
        "letmutnew=self.state.counts.iter().peekable();",
        "if!predecessor.same_position(self,*candidate)",
        "Ok(ProposalTransitionSource::ExactFallback)",
    )
    changed_positions = [
        dense_changed_keys.find(fragment) for fragment in changed_fragments
    ]
    if any(position < 0 for position in changed_positions) or changed_positions != sorted(
        changed_positions
    ):
        errors.append(
            "ProposalView must authenticate an exact sparse predecessor or merge both full ordered universes"
        )

    dense_finalize = compact(table_finalize)
    if (
        "try_for_each_changed_key_from" in dense_finalize
        or "removed" in dense_finalize
        or "proposed_ids().count()" in dense_finalize
        or "gap_ids().count()" in dense_finalize
        or "->ProposalView" not in dense_proposal_table
    ):
        errors.append(
            "ProposalTable finalize must publish only the exact view, without a duplicate policy projection or population scan"
        )

    dense_two_phase = compact(two_phase_verify)
    if any(
        forbidden in contextual_verifier
        for forbidden in ("ProposalTable", "ProposalView", "ckb_proposal_table")
    ):
        errors.append(
            "TwoPhaseCommitVerifier must remain independent of the rebuildable proposal projection"
        )
    two_phase_fragments = (
        "letblock_number=self.block.header().number();",
        "letproposal_window=self.context.consensus.tx_proposal_window();",
        "letproposal_start=block_number.saturating_sub(proposal_window.farthest());",
        "letmutproposal_end=block_number.saturating_sub(proposal_window.closest());",
        "get_block_proposal_txs_ids(&block_hash)",
        "get_block_uncles(&block_hash)",
        ".transactions().iter().skip(1).map(TransactionView::proposal_short_id).collect();",
        "committed_ids.difference(&proposal_txs_ids).next().is_some()",
    )
    two_phase_positions = [
        dense_two_phase.find(fragment) for fragment in two_phase_fragments
    ]
    if any(position < 0 for position in two_phase_positions) or two_phase_positions != sorted(
        two_phase_positions
    ):
        errors.append(
            "TwoPhaseCommitVerifier lost its primitive branch-history committed-subset proof path"
        )

    if position_variants != ["Proposed", "Gap", "Outside"]:
        errors.append(
            "proposal-window position must remain the closed Proposed/Gap/Outside relation"
        )
    dense_position = compact(position)
    position_fragments = (
        "ifsnapshot.proposals().contains_proposed(proposal)",
        "ProposalWindowPosition::Proposed",
        "elseifsnapshot.proposals().contains_gap(proposal)",
        "ProposalWindowPosition::Gap",
        "ProposalWindowPosition::Outside",
    )
    positions = [dense_position.find(fragment) for fragment in position_fragments]
    if any(position < 0 for position in positions) or positions != sorted(positions):
        errors.append(
            "proposal_window_position must be the sole ordered Snapshot set/gap derivation"
        )
    if chain_model.count("contains_proposed(") != 1 or chain_model.count("contains_gap(") != 1:
        errors.append(
            "proposal status must have exactly one production Snapshot membership derivation"
        )

    dense_receipt = compact(receipt)
    if dense_receipt != (
        "ProposalContextReceipt::from_position("
        "proposal_window_position(snapshot,proposal))"
    ):
        errors.append("proposal context receipt must seal the canonical Snapshot position")
    dense_from_position = compact(receipt_from_position)
    for fragment in (
        "ProposalWindowPosition::Proposed=>AcceptedStatus::Proposed",
        "ProposalWindowPosition::Gap=>AcceptedStatus::Gap",
        "ProposalWindowPosition::Outside=>AcceptedStatus::Pending",
        "Self{status}",
    ):
        if fragment not in dense_from_position:
            errors.append(
                "ProposalContextReceipt::from_position lost the total position/status map"
            )
            break
    if "from_validation" in "".join(
        name for name, _body, _line in rust_impl_methods(chain_model, "ProposalContextReceipt")
    ):
        errors.append("ProposalContextReceipt regained a free validation-status constructor")
    dense_membership = compact(membership)
    if (
        "proposal:ProposalContextReceipt" not in compact(chain_model)
        or "proposal," not in dense_membership
        or "ProposalContextReceipt::" in dense_membership
    ):
        errors.append(
            "MembershipReceipt must consume one already sealed proposal context receipt"
        )

    dense_validation = compact(function_body(validation, "validate_membership") or "")
    validation_fragments = (
        "letproposal=proposal_context_receipt(&snapshot,&verified.payload().identity().proposal.0);",
        "letstatus=proposal.status();",
        "letenvironment=verification_environment(status,&snapshot);",
        "MembershipReceipt::from_validation(seal,verified,sensitivity,proposal,",
    )
    validation_positions = [
        dense_validation.find(fragment) for fragment in validation_fragments
    ]
    if any(position < 0 for position in validation_positions) or validation_positions != sorted(
        validation_positions
    ):
        errors.append(
            "final admission must derive status and verification rules from one sealed Snapshot receipt"
        )

    dense_transition = compact(transition)
    transition_fragments = (
        "letold=old_snapshot.proposals();",
        "letnew=new_snapshot.proposals();",
        "letmutchanged=Vec::new();",
        "new.try_for_each_changed_from(old,|proposal|",
        "changed.try_reserve(1)",
        "changed.push(ProposalId(proposal));",
        "Ok(Self{changed})",
    )
    transition_positions = [
        dense_transition.find(fragment) for fragment in transition_fragments
    ]
    if any(position < 0 for position in transition_positions) or transition_positions != sorted(
        transition_positions
    ):
        errors.append(
            "ProposalTransitionFacts::between must derive the canonical exact set/gap position delta"
        )
    if any(
        forbidden in dense_transition
        for forbidden in (
            "symmetric_difference",
            "sort_unstable",
            "dedup",
            "retain",
            "letmutuniverse",
        )
    ):
        errors.append(
            "proposal transition construction regressed to sorting/allocating the complete window universe"
        )

    dense_apply = compact(apply_chain)
    apply_fragments = (
        "letold_snapshot={letstore=self.store.read();Arc::clone(&store.snapshot)};",
        "ProposalTransitionFacts::between(&old_snapshot,&command.snapshot)",
        "letstore=self.store.upgradable_read();",
        "if!Arc::ptr_eq(&store.snapshot,&old_snapshot)",
        ".bind(new_view.clone(),accepted_validity,&proposal_transition);",
    )
    apply_positions = [dense_apply.find(fragment) for fragment in apply_fragments]
    if any(position < 0 for position in apply_positions) or apply_positions != sorted(
        apply_positions
    ):
        errors.append(
            "chain Apply must derive proposal delta outside the guard and revalidate the exact old Snapshot identity before Plan/Apply"
        )

    dense_plan = compact(plan_chain)
    plan_fragments = (
        "proposal_candidates.try_reserve(facts.changed_proposals.len())",
        "proposal_candidates.extend(facts.changed_proposals.iter().cloned());",
        "status_subjects.try_reserve(proposal_candidates.len())",
        "forproposalin&proposal_candidates",
        "self.indexes.proposal_owner(proposal)",
    )
    plan_positions: list[int] = []
    plan_cursor = 0
    for fragment in plan_fragments:
        position = dense_plan.find(fragment, plan_cursor)
        plan_positions.append(position)
        if position >= 0:
            plan_cursor = position + len(fragment)
    if any(position < 0 for position in plan_positions):
        errors.append(
            "chain Plan must reconcile only the exact changed proposal owners"
        )
    if (
        "left_proposed" in dense_transition
        or "left_proposed" in dense_plan
        or "ForcePending" in transition
        or "ForcePending" in plan_chain
    ):
        errors.append(
            "proposal status changes regained a semantically inert causal descendant rescan"
        )

    if candidate_variants != ["CollectCandidateUncles", "SkipCandidateUncles"]:
        errors.append(
            "chain boundary candidate-uncle capability must remain the closed collect/skip relation"
        )
    dense_boundary = compact(boundary)
    if (
        "self.candidate_uncles.collects_candidate_uncles()" not in dense_boundary
        or "candidate_uncles.try_insert(block.as_uncle())" not in dense_boundary
        or "detached_proposal" in dense_boundary
        or "ProposalWindowPosition" in dense_boundary
    ):
        errors.append(
            "candidate-uncle collection must affect only the rebuildable candidate projection"
        )

    dense_controller = compact(function_body(controller, "update_tx_pool_for_reorg") or "")
    controller_fragments = (
        "drop(detached_proposal_id);",
        "ChainReorgArgs::bounded(detached_blocks,attached_blocks,snapshot,self.chain_reorg_payload_limit,)",
    )
    controller_positions = [
        dense_controller.find(fragment) for fragment in controller_fragments
    ]
    if any(position < 0 for position in controller_positions) or controller_positions != sorted(
        controller_positions
    ):
        errors.append(
            "the legacy detached-proposal API parameter must remain a non-authoritative facade"
        )
    dense_verify = compact(chain_verify)
    if (
        dense_verify.count("letnew_proposals=self.proposal_table.finalize(") != 2
        or "fork.detached_proposal" in dense_verify
        or dense_verify.count("HashSet::new(),new_snapshot") != 1
    ):
        errors.append(
            "chain publication must discard ProposalTable's historical subset and pass no proposal policy hint"
        )

    forbidden = (
        "ChainPackagingMode",
        "ProposalStatusBaseline",
        "AcceptedProposalIndex",
    )
    joined = "\n".join((chain_model, runtime, validation, boundary, planner, indexes))
    for name in forbidden:
        if name in joined:
            errors.append(f"retired proposal authority vocabulary returned: {name}")
    if "accepted_proposal" in indexes.lower():
        errors.append("Accepted membership regained a full-scan proposal-status index")
    return errors


def validate_proposal_history_provenance() -> list[str]:
    try:
        sources = {
            "chain_model": TX_POOL_AUTHORITY_CHAIN.read_text(),
            "proposal_table": PROPOSAL_TABLE.read_text(),
            "block_verifier": BLOCK_VERIFIER.read_text(),
            "contextual_verifier": CONTEXTUAL_BLOCK_VERIFIER.read_text(),
            "contextual_uncles_verifier": CONTEXTUAL_UNCLES_VERIFIER.read_text(),
            "runtime": TX_POOL_AUTHORITY_RUNTIME.read_text(),
            "validation": TX_POOL_AUTHORITY_VALIDATION.read_text(),
            "boundary": TX_POOL_AUTHORITY_CHAIN_BOUNDARY.read_text(),
            "planner": TX_POOL_AUTHORITY_CHAIN_TRANSITION.read_text(),
            "controller": TX_POOL_CONTROLLER.read_text(),
            "chain_service": CHAIN_SERVICE.read_text(),
            "chain_verify": CHAIN_VERIFY.read_text(),
            "shared_builder": SHARED_BUILDER.read_text(),
            "ckb_setup": CKB_SETUP.read_text(),
            "ckb_replay": CKB_REPLAY.read_text(),
            "verification_traits": VERIFICATION_TRAITS.read_text(),
            "indexes": TX_POOL_AUTHORITY_INDEXES.read_text(),
        }
    except OSError as error:
        return [f"cannot inspect proposal-history provenance: {error}"]

    errors = proposal_history_provenance_errors(**sources)
    canaries = (
        (
            "exact_delta_bypass",
            "chain_model",
            "new.try_for_each_changed_from(old, |proposal| {",
            "new.proposed_ids().try_for_each(|proposal| {",
        ),
        (
            "randomized_height_index",
            "proposal_table",
            "BTreeMap<BlockNumber, Box<[ProposalKey]>>",
            "BTreeMap<BlockNumber, HashSet<ProposalKey>>",
        ),
        (
            "unsorted_height_projection",
            "proposal_table",
            "ids.sort_unstable();",
            "ids.reverse();",
        ),
        (
            "sparse_predecessor_forgery",
            "proposal_table",
            "predecessor: Arc::downgrade(&origin.state),",
            "predecessor: Weak::new(),",
        ),
        (
            "sparse_identity_equivalence",
            "proposal_table",
            "state.as_ptr() == Arc::as_ptr(&self.state)",
            "state.upgrade().is_some()",
        ),
        (
            "unvalidated_main_proposals",
            "block_verifier",
            "BlockProposalsLimitVerifier::new(max_block_proposals_limit).verify(target)?;",
            "let _ = max_block_proposals_limit;",
        ),
        (
            "unbounded_uncle_proposals",
            "contextual_uncles_verifier",
            "> self.provider.consensus().max_block_proposals_limit() as usize",
            "> usize::MAX",
        ),
        (
            "population_scan_in_finalize_log",
            "proposal_table",
            "next.state.counts.len()",
            "next.proposed_ids().count()",
        ),
        (
            "operator_bypass_erased",
            "ckb_setup",
            "Switch::DISABLE_ALL",
            "Switch::NONE",
        ),
        (
            "replay_bypass_erased",
            "ckb_replay",
            "Switch::DISABLE_ALL - Switch::DISABLE_NON_CONTEXTUAL",
            "Switch::NONE",
        ),
        (
            "raw_candidate_overflow",
            "proposal_table",
            "tip.checked_add(1)",
            "Some(tip + 1)",
        ),
        (
            "shared_consensus_projection",
            "contextual_verifier",
            "pub struct TwoPhaseCommitVerifier<'a, CS> {",
            "pub struct ProposalView;\npub struct TwoPhaseCommitVerifier<'a, CS> {",
        ),
        (
            "old_snapshot_fence_omission",
            "runtime",
            "if !Arc::ptr_eq(&store.snapshot, &old_snapshot) {",
            "if false {",
        ),
        (
            "free_admission_status",
            "validation",
            "let proposal = proposal_context_receipt(",
            "let proposal = ProposalContextReceipt::from_internal_status(",
        ),
        (
            "packaging_status_authority",
            "boundary",
            "pub(super) enum CandidateUncleCollection {",
            "pub(super) enum ChainPackagingMode {\n    ObserveOnly,\n}\n\npub(super) enum CandidateUncleCollection {",
        ),
        (
            "causal_proposal_rescan",
            "planner",
            "let mut proposal_candidates = Vec::new();",
            "for proposal in facts.left_proposed {\n"
            "    causal.seed_accepted(proposal.clone(), CausalDisposition::ForcePending)?;\n"
            "}\nlet mut proposal_candidates = Vec::new();",
        ),
    )
    for name, source_name, needle, replacement in canaries:
        original = sources[source_name]
        if needle not in original:
            errors.append(f"cannot construct proposal-history negative canary {name}")
            continue
        mutated = dict(sources)
        mutated[source_name] = original.replace(needle, replacement, 1)
        if not proposal_history_provenance_errors(**mutated):
            errors.append(f"proposal-history negative canary survived: {name}")
    return errors


def proposal_commit_liveness_path_errors(
    template: str,
    packing: str,
    template_driver: str,
    block_assembler: str,
) -> list[str]:
    """Bind typed template capacity to the conditional proposal/commit offer path."""

    errors: list[str] = []
    try:
        proposal_ids = impl_method_body(
            template, "TemplateSelectionReceipt", "proposal_short_ids"
        )
        causal = impl_method_body(
            template, "TemplateSelectionReceipt", "causally_eligible_proposed"
        )
        pack = impl_method_body(
            packing,
            "TemplateSelectionReceipt",
            "pack_transactions_with_failure_bound",
        )
        pack_entry = impl_method_body(
            packing, "TemplateSelectionReceipt", "pack_transactions"
        )
        full = impl_method_body(
            template_driver, "AuthorityBlockAssembler", "prepare_full"
        )
        proposals = impl_method_body(
            template_driver, "AuthorityBlockAssembler", "prepare_proposals"
        )
        transactions = impl_method_body(
            template_driver, "AuthorityBlockAssembler", "prepare_transactions"
        )
        uncles = impl_method_body(
            template_driver, "AuthorityBlockAssembler", "prepare_uncles"
        )
        optional = impl_method_body(
            block_assembler, "BlockAssembler", "fit_optional_content"
        )
    except ValueError as error:
        return [str(error)]

    dense_proposal_ids = "".join(mask_rust_non_code(proposal_ids).split())
    errors.extend(
        require_ordered_fragments(
            dense_proposal_ids,
            "current proposal prefix",
            (
                "self.ordered_indices([AcceptedStatus::Pending])?",
                "usize::try_from(limit)",
                "limit.min(ordered.len())",
                "ordered.into_iter().take(selected)",
                ".proposal_short_id()",
            ),
        )
    )
    if dense_proposal_ids.count("AcceptedStatus::Pending") != 1:
        errors.append("current proposal prefix must derive only the Pending phase once")

    dense_causal = "".join(mask_rust_non_code(causal).split())
    errors.extend(
        require_ordered_fragments(
            dense_causal,
            "causal commit eligibility",
            (
                "letcausal=causal_indices(&self.candidates,by_hash)?;",
                "forparentin&candidate.parents",
                "candidate.status==AcceptedStatus::Proposed&&parents_eligible",
                "forindexincausal",
                "selected.push(index)",
            ),
        )
    )

    dense_pack_entry = "".join(mask_rust_non_code(pack_entry).split())
    if (
        "self.pack_transactions_with_failure_bound(limits,MAX_CONSECUTIVE_PACKING_FAILURES)"
        not in dense_pack_entry
    ):
        errors.append("production packing bypasses its finite failure-work bound")
    dense_pack = "".join(mask_rust_non_code(pack).split())
    errors.extend(
        require_ordered_fragments(
            dense_pack,
            "conditional commit offer",
            (
                "leteligible=self.causally_eligible_proposed(&by_hash)?;",
                "ifaggregate.fits(limits)",
                "whileletSome(key)=queue.pop_last()",
                "selected.push(member)",
                "consecutive_failures=0;",
                "self.order_packed_indices(selected,&by_hash)?",
            ),
        )
    )

    proposal_call = ".proposal_short_ids(consensus.max_block_proposals_limit())?;"
    optional_call = "BlockAssembler::fit_optional_content("
    for owner, body in (
        ("prepare_full", full),
        ("prepare_proposals", proposals),
        ("prepare_uncles", uncles),
    ):
        dense = "".join(mask_rust_non_code(body).split())
        errors.extend(
            require_ordered_fragments(
                dense,
                owner,
                (proposal_call, optional_call),
            )
        )

    for owner, body in (("prepare_full", full), ("prepare_transactions", transactions)):
        dense = "".join(mask_rust_non_code(body).split())
        errors.extend(
            require_ordered_fragments(
                dense,
                f"{owner} commit capacity",
                (
                    "max_block_bytes.checked_sub(",
                    ".pack_transactions(TemplatePackingLimits::new(",
                    "consensus.max_block_cycles()",
                ),
            )
        )

    dense_optional = "".join(mask_rust_non_code(optional).split())
    errors.extend(
        require_ordered_fragments(
            dense_optional,
            "mandatory proposal before optional uncle fitting",
            (
                "Self::fit_proposal_prefix(&mutproposals,base_total_size,max_block_bytes)",
                "letproposal_set=proposals.iter().cloned().collect::<HashSet<_>>();",
                "Self::filter_uncles_conflicting_with_proposals(",
                "Self::fit_uncle_prefix_after_base(&mutuncles,proposals_total,max_block_bytes)",
            ),
        )
    )

    relevant = "\n".join(
        (proposal_ids, causal, pack_entry, pack, full, proposals, transactions, uncles, optional)
    )
    for forbidden in ("tokio::time", "sleep(", "qualitative_fair", "retry_delay"):
        if forbidden in relevant:
            errors.append(
                f"proposal/commit liveness path uses {forbidden!r} as a progress premise"
            )
    return errors


def validate_proposal_commit_liveness_path() -> list[str]:
    try:
        sources = {
            "template": TX_POOL_AUTHORITY_TEMPLATE.read_text(),
            "packing": TX_POOL_AUTHORITY_PACKING.read_text(),
            "template_driver": TX_POOL_AUTHORITY_TEMPLATE_DRIVER.read_text(),
            "block_assembler": TX_POOL_BLOCK_ASSEMBLER.read_text(),
        }
    except OSError as error:
        return [f"cannot inspect proposal/commit liveness path: {error}"]

    errors = proposal_commit_liveness_path_errors(**sources)
    canaries = (
        (
            "pending_phase",
            "template",
            "self.ordered_indices([AcceptedStatus::Pending])?",
            "self.ordered_indices([AcceptedStatus::Proposed])?",
        ),
        (
            "proposal_count_limit",
            "template",
            "ordered.into_iter().take(selected)",
            "ordered.into_iter()",
        ),
        (
            "consensus_proposal_limit",
            "template_driver",
            "consensus.max_block_proposals_limit()",
            "u64::MAX",
        ),
        (
            "causal_commit_eligibility",
            "packing",
            "self.causally_eligible_proposed(&by_hash)?",
            "self.ordered_indices([AcceptedStatus::Proposed])?",
        ),
        (
            "proposal_byte_priority",
            "block_assembler",
            "Self::fit_proposal_prefix(&mut proposals, base_total_size, max_block_bytes)",
            "Some((0, base_total_size))",
        ),
        (
            "proposal_uncle_conflict",
            "block_assembler",
            "Self::filter_uncles_conflicting_with_proposals(",
            "Self::retain_all_uncles_for_canary(",
        ),
        (
            "uncle_consumes_proposal_base",
            "block_assembler",
            "Self::fit_uncle_prefix_after_base(&mut uncles, proposals_total, max_block_bytes)",
            "Self::fit_uncle_prefix_after_base(&mut uncles, base_total_size, max_block_bytes)",
        ),
        (
            "consensus_commit_cycles",
            "template_driver",
            "consensus.max_block_cycles()",
            "u64::MAX",
        ),
    )
    for name, source_name, needle, replacement in canaries:
        original = sources[source_name]
        if needle not in original:
            errors.append(f"cannot construct proposal/commit negative canary {name}")
            continue
        mutated = dict(sources)
        mutated[source_name] = original.replace(needle, replacement, 1)
        if not proposal_commit_liveness_path_errors(**mutated):
            errors.append(f"proposal/commit negative canary survived: {name}")
    return errors


def chain_apply_template_visibility_errors(
    message: str,
    controller: str,
    service: str,
    template_driver: str,
    template_model: str,
) -> list[str]:
    """Check the relational completion/freshness protocol, not isolated tokens."""

    errors: list[str] = []
    control_enum = re.search(
        r"enum\s+ChainControl\s*\{(?P<body>.*?)\n\}", message, re.S
    )
    dense_control = (
        "".join(mask_rust_non_code(control_enum.group("body")).split())
        if control_enum is not None
        else ""
    )
    if dense_control.count("Reconcile(SyncRequest<ChainReorgArgs,()>)") != 1:
        errors.append(
            "ordered Reconcile must carry exactly one Apply-completion responder"
        )

    update = function_body(controller, "update_tx_pool_for_reorg") or ""
    dense_update = "".join(mask_rust_non_code(update).split())
    update_fragments = (
        "let(responder,response)=oneshot::channel();",
        "ChainControl::Reconcile(Request::call(",
        "self.handle.block_on(self.chain_control_sender.send(command))",
        "block_in_place(||response.recv())",
    )
    positions = [dense_update.find(fragment) for fragment in update_fragments]
    if any(position < 0 for position in positions) or positions != sorted(positions):
        errors.append(
            "the trusted chain publisher must construct one response capability, send the "
            "bounded command and await that exact Apply completion in order"
        )
    if "try_send" in dense_update or "tokio::time" in dense_update or "sleep(" in dense_update:
        errors.append(
            "chain Apply completion may neither drop, poll nor use a timer as progress"
        )

    driver = function_body(service, "run_ordered_chain_control_driver") or ""
    dense_driver = "".join(mask_rust_non_code(driver).split())
    driver_fragments = (
        "ChainControl::Reconcile(Request{responder,arguments,})",
        "service.commit_chain_update(arguments)",
        "respond(responder,(),)",
        "service.publish_chain_observers(committed)",
    )
    positions = [dense_driver.find(fragment) for fragment in driver_fragments]
    if any(position < 0 for position in positions) or positions != sorted(positions):
        errors.append(
            "Reconcile must acknowledge after the exact authority commit and before every "
            "rebuildable chain observer"
        )

    try:
        commit = impl_method_body(service, "AuthorityService", "commit_chain_update")
        observers = impl_method_body(service, "AuthorityService", "publish_chain_observers")
        block_template = impl_method_body(service, "AuthorityService", "block_template")
        read = impl_method_body(
            template_driver, "AuthorityBlockAssembler", "current_template"
        )
        replacement = impl_method_body(
            template_driver, "AuthorityBlockAssembler", "run_replacement_lane"
        )
        read_state = impl_method_body(
            template_model, "TemplateConvergence", "chain_read_state"
        )
        coherent_source = impl_method_body(
            template_model, "TemplateCoverage", "coherent_chain_source"
        )
        record_failure = impl_method_body(
            template_model, "TemplateConvergence", "record_replacement_failure"
        )
        record_progress = impl_method_body(
            template_model, "TemplateConvergence", "record_replacement_progress"
        )
        publish_reset = impl_method_body(
            template_model, "TemplateConvergence", "publish_reset"
        )
        publish_partial = impl_method_body(
            template_model, "TemplateConvergence", "publish_partial"
        )
    except ValueError as error:
        return [*errors, str(error)]

    if "publish_chain_observers" in commit or "current_template" in commit:
        errors.append(
            "the minimum authority commit cut must not absorb fee/template derived work"
        )
    dense_observers = "".join(mask_rust_non_code(observers).split())
    if (
        "self.fee_estimator.commit_block(block)" not in dense_observers
        or "observe_candidate_uncle(assembler,uncle)" not in dense_observers
    ):
        errors.append("post-commit chain observers escaped their sole derived owner")

    dense_read = "".join(mask_rust_non_code(read).split())
    read_fragments = (
        "letauthority_notified=authority_signal.notified();",
        "letlocal_notified=self.wake.notified();",
        "let_=authority_notified.as_mut().enable();",
        "let_=local_notified.as_mut().enable();",
        "letrequired=self.runtime.template_chain_source();",
        "letcurrent=self.assembler.current.read().await;",
        "letstate=self.convergence.lock().chain_read_state(required);",
        "TemplateChainReadState::Published",
        "TemplateChainReadState::Failed",
        "TemplateChainReadState::Pending",
        "drop(current);tokio::select!{_=cancel.cancelled()",
        "Cancelled),_=authority_notified.as_mut()",
        "=>{}_=local_notified.as_mut()",
    )
    positions = [dense_read.find(fragment) for fragment in read_fragments]
    if any(position < 0 for position in positions) or positions != sorted(positions):
        errors.append(
            "template read must subscribe before its coherent current/source cut, return only "
            "Published, terminate Failed and wait Pending on named monotonic releasers"
        )
    if "tokio::time" in dense_read or "sleep(" in dense_read:
        errors.append("template freshness may not use timeout or polling as progress")

    dense_state = "".join(mask_rust_non_code(read_state).split())
    state_fragments = (
        "ifself.covered.coherent_chain_source()==Some(required)",
        "TemplateChainReadState::Published",
        "elseifself.failed_replacement_chain==Some(required)",
        "TemplateChainReadState::Failed",
        "TemplateChainReadState::Pending",
    )
    positions = [dense_state.find(fragment) for fragment in state_fragments]
    if any(position < 0 for position in positions) or positions != sorted(positions):
        errors.append(
            "template chain-read state must derive Published from the coherent component "
            "vector before exact-source Failed/Pending"
        )
    dense_coherent = "".join(mask_rust_non_code(coherent_source).split())
    coherent_fragments = (
        "letproposals=self.proposals?.chain_source();",
        "lettransactions=self.transactions?.chain_source();",
        "letuncles=self.uncles?.chain_source();",
        "proposals==transactions&&transactions==uncles",
        ".then_some(proposals)",
    )
    positions = [dense_coherent.find(fragment) for fragment in coherent_fragments]
    if any(position < 0 for position in positions) or positions != sorted(positions):
        errors.append(
            "template coverage must prove proposal/transaction/uncle chain-source "
            "coherence without a scalar publication surrogate"
        )
    if "published_chain" in template_model or "record_chain_publication" in template_model:
        errors.append("template readiness regained a duplicate scalar publication authority")
    if "self.failed_replacement_chain=Some(failed)" not in "".join(
        mask_rust_non_code(record_failure).split()
    ):
        errors.append("replacement failure must publish its exact terminal source cut")
    dense_progress = "".join(mask_rust_non_code(record_progress).split())
    if (
        "self.failed_replacement_chain" not in dense_progress
        or "self.failed_replacement_chain=None" not in dense_progress
        or "published_chain" in dense_progress
    ):
        errors.append(
            "replacement progress may retire only the failure marker, never own readiness"
        )
    dense_reset = "".join(mask_rust_non_code(publish_reset).split())
    if "self.covered=TemplateCoverage::default()" not in dense_reset:
        errors.append("template reset must invalidate every component receipt")
    dense_partial = "".join(mask_rust_non_code(publish_partial).split())
    partial_fragments = (
        "self.covered.proposals=Some(PublishedComponentCoverage::Exact(coverage));self.covered.transactions=None;self.covered.uncles=None;",
        "self.covered.transactions=Some(PublishedComponentCoverage::Exact(coverage))",
        "self.covered.proposals=Some(PublishedComponentCoverage::Exact(coverage.proposal_cut()));self.covered.transactions=None;self.covered.uncles=Some(PublishedComponentCoverage::Exact(coverage));",
    )
    if any(fragment not in dense_partial for fragment in partial_fragments):
        errors.append(
            "partial publication must invalidate every component coupled to its changed cut"
        )

    try:
        coverage_variants = rust_enum_variants(
            template_model, "PublishedComponentCoverage"
        )
        initial_base = impl_method_body(
            template_model, "TemplateCoverage", "initial_base"
        )
        convergence_new = impl_method_body(
            template_model, "TemplateConvergence", "new"
        )
        coverage_methods = rust_impl_methods(
            template_model, "PublishedComponentCoverage", allow_multiple=True
        )
    except ValueError as error:
        errors.append(str(error))
        coverage_variants = []
        initial_base = ""
        convergence_new = ""
        coverage_methods = []
    if coverage_variants != ["ChainOnly", "Exact"]:
        errors.append(
            "published component coverage must remain the closed ChainOnly/Exact relation"
        )
    dense_initial = "".join(mask_rust_non_code(initial_base).split())
    if dense_initial.count("Some(PublishedComponentCoverage::ChainOnly(chain))") != 3:
        errors.append(
            "the startup base must publish three chain-only receipts without claiming exact source capture"
        )
    dense_new = "".join(mask_rust_non_code(convergence_new).split())
    if (
        "covered:TemplateCoverage::initial_base(initial.chain_source())" not in dense_new
        or "full_required:Some(initial)" not in dense_new
        or "covered:TemplateCoverage::full(initial)" in dense_new
    ):
        errors.append(
            "TemplateConvergence startup must separate chain-safe read coverage from exact full construction"
        )
    exact_methods = [
        body for name, body, _line in coverage_methods if name == "is_exact"
    ]
    if len(exact_methods) != 1 or "self==Self::Exact(source)" not in "".join(
        mask_rust_non_code(exact_methods[0]).split()
    ):
        errors.append("component convergence must require an exact source receipt")

    dense_replacement = "".join(mask_rust_non_code(replacement).split())
    replacement_fragments = (
        "Err(FailedTemplateAttempt{source,error})",
        "source.replacement_chain_source()",
        ".record_replacement_failure(chain_source)",
        "self.wake.notify_waiters()",
        "self.next_template_source_after_failure(&cancel,source).await",
    )
    positions = [dense_replacement.find(fragment) for fragment in replacement_fragments]
    if any(position < 0 for position in positions) or positions != sorted(positions):
        errors.append(
            "replacement failure must settle and wake the exact chain-source reader before "
            "parking on the same source-change law"
        )

    dense_block_template = "".join(mask_rust_non_code(block_template).split())
    if (
        ".current_template(&self.cancel).await.map_err(map_template_availability)"
        not in dense_block_template
    ):
        errors.append("the public block-template query bypasses the source-gated read protocol")
    if "AuthorityServiceError::TemplateUnavailable" not in service:
        errors.append("same-source template failure lost its typed operational outcome")
    return errors


def validate_chain_apply_template_visibility() -> list[str]:
    try:
        sources = {
            "message": TX_POOL_MESSAGE.read_text(),
            "controller": TX_POOL_CONTROLLER.read_text(),
            "service": TX_POOL_AUTHORITY_SERVICE.read_text(),
            "template_driver": TX_POOL_AUTHORITY_TEMPLATE_DRIVER.read_text(),
            "template_model": TX_POOL_AUTHORITY_TEMPLATE.read_text(),
        }
    except OSError as error:
        return [f"cannot inspect chain/template visibility protocol: {error}"]

    errors = chain_apply_template_visibility_errors(**sources)
    canaries = (
        (
            "publisher_response_wait",
            "controller",
            "block_in_place(|| response.recv())",
            "block_in_place(|| Ok(()))",
        ),
        (
            "authority_apply_response",
            "service",
            'respond(responder, (), "chain_reconcile_apply");',
            "drop(responder);",
        ),
        (
            "template_failure_terminal",
            "template_driver",
            ".record_replacement_failure(chain_source);",
            ".observe_sources_for_canary(chain_source);",
        ),
        (
            "template_source_gate",
            "template_model",
            "TemplateChainReadState::Pending",
            "TemplateChainReadState::Published",
        ),
        (
            "template_component_vector_gate",
            "template_model",
            "self.covered.coherent_chain_source() == Some(required)",
            "self.desired.chain_source() == required",
        ),
        (
            "template_reset_component_invalidation",
            "template_model",
            "self.covered = TemplateCoverage::default();",
            "self.covered = TemplateCoverage::full(self.desired);",
        ),
        (
            "template_scalar_shadow_authority",
            "template_model",
            "failed_replacement_chain: Option<ApplySequence>,",
            "published_chain: ApplySequence,\n    failed_replacement_chain: Option<ApplySequence>,",
        ),
        (
            "template_initial_exact_impersonation",
            "template_model",
            "TemplateCoverage::initial_base(initial.chain_source())",
            "TemplateCoverage::full(initial)",
        ),
    )
    for name, source_name, needle, replacement in canaries:
        original = sources[source_name]
        if needle not in original:
            errors.append(f"cannot construct chain/template negative canary {name}")
            continue
        mutated = dict(sources)
        if name == "publisher_response_wait":
            position = original.rfind(needle)
            mutated[source_name] = (
                original[:position]
                + replacement
                + original[position + len(needle) :]
            )
        else:
            mutated[source_name] = original.replace(needle, replacement, 1)
        if not chain_apply_template_visibility_errors(**mutated):
            errors.append(f"chain/template negative canary survived: {name}")
    return errors


def validate_startup_backpressure() -> list[str]:
    errors: list[str] = []
    try:
        service = TX_POOL_SERVICE.read_text()
        controller = TX_POOL_CONTROLLER.read_text()
        builder = TX_POOL_BUILDER.read_text()
        message = TX_POOL_MESSAGE.read_text()
        authority_service = TX_POOL_AUTHORITY_SERVICE.read_text()
    except OSError as error:
        return [f"cannot inspect startup backpressure protocol: {error}"]

    if "const CHAIN_CONTROL_CHANNEL_SIZE: usize = 1;" not in service:
        errors.append("the ordered chain-control boundary must retain capacity one")

    compact_service = " ".join(mask_rust_non_code(service).split())
    if "mod administration {" not in compact_service:
        errors.append("public administration capabilities must have a sealed module")
    if service.count("pub(crate) struct AdministrationGate") != 1:
        errors.append("the ordered boundary must own one shared AdministrationGate")
    if service.count("pub(crate) struct AdminAdmission") != 1:
        errors.append("the ordered boundary must own one AdminAdmission capability")
    if service.count("pub(crate) struct AdmittedAdministration") != 1:
        errors.append("the ordered boundary must own one admitted command wrapper")
    if re.search(
        r"#\s*\[\s*derive\s*\([^]]*\b(?:Clone|Copy)\b[^]]*\)\s*\]\s*"
        r"pub\s*\(crate\)\s+struct\s+AdminAdmission",
        service,
        re.S,
    ):
        errors.append("AdminAdmission must remain a unique move-only capability")
    for fragment in (
        "compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)",
        "self.gate.occupied.store(false, Ordering::Release)",
        "pub(crate) use administration::{AdministrationGate, AdmittedAdministration};",
    ):
        if fragment not in compact_service:
            errors.append(f"sealed administration capability lost {fragment!r}")

    dense_controller = "".join(mask_rust_non_code(controller).split())
    if dense_controller.count("administration_gate:AdministrationGate") != 1:
        errors.append("every cloneable controller must share one administration gate")
    if dense_controller.count("administration_gate.try_acquire()") != 1:
        errors.append("public administration must have one production acquisition point")
    if controller.count("AdmittedAdministration::new(") != 1:
        errors.append("public administration must have one admitted-command constructor")
    admitted_macro = re.search(
        r"macro_rules!\s+send_admitted_chain_control\s*\{(?P<body>.*?)\n\}\n\n"
        r"macro_rules!\s+reject_callback_mutation",
        controller,
        re.S,
    )
    if admitted_macro is None:
        errors.append("the admitted ordered-control producer disappeared")
    else:
        macro_body = "".join(mask_rust_non_code(admitted_macro.group("body")).split())
        ordered_fragments = (
            "$self.administration_gate.try_acquire()",
            "let(responder,response)=oneshot::channel();",
            "letrequest=Request::call($args,responder);",
            "AdmittedAdministration::new(admission,request)",
            ".send(command)",
        )
        positions = [macro_body.find(fragment) for fragment in ordered_fragments]
        if any(position < 0 for position in positions) or positions != sorted(positions):
            errors.append(
                "public administration must acquire before request construction and reliable send"
            )
        if "try_send" in macro_body:
            errors.append("an admitted administration must retain reliable ordered send")

    if builder.count("let administration_gate = AdministrationGate::new();") != 1:
        errors.append("service assembly must construct one shared administration gate")
    if builder.count("administration_gate,") != 1:
        errors.append("service assembly must move the shared administration gate into the controller")

    update = function_body(controller, "update_tx_pool_for_reorg")
    if update is None:
        errors.append("TxPoolController::update_tx_pool_for_reorg disappeared")
    else:
        if "chain_control_sender.send(command)" not in update:
            errors.append("authoritative reorg delivery must use bounded async send")
        if "try_send" in update or "service_started" in update:
            errors.append(
                "authoritative reorg delivery may neither drop on capacity nor gate on readiness"
            )

    message_enum = re.search(r"enum\s+Message\s*\{(?P<body>.*?)\n\}", message, re.S)
    control_enum = re.search(
        r"enum\s+ChainControl\s*\{(?P<body>.*?)\n\}", message, re.S
    )
    if message_enum is None or control_enum is None:
        errors.append("Message and ChainControl must remain explicit closed enums")
    else:
        for command in ("ClearPool", "ClearPipeline"):
            if command in message_enum.group("body"):
                errors.append(
                    f"{command} must not race chain reconciliation on the concurrent dispatcher"
                )
            if command not in control_enum.group("body"):
                errors.append(f"{command} disappeared from the ordered chain-control lane")
        compact_control = " ".join(control_enum.group("body").split())
        for command, request in (
            ("ClearPool", "SyncRequest<Arc<Snapshot>, ()>"),
            ("ClearPipeline", "SyncRequest<(), ()>"),
        ):
            if f"{command}(AdmittedAdministration<{request}>)" not in compact_control:
                errors.append(
                    f"{command} must carry the unique administration capability with its request"
                )
    for method, fragment in (
        ("clear_pool", "send_admitted_chain_control!(self, ClearPool"),
        ("clear_verify_queue", "send_admitted_chain_control!(self, ClearPipeline"),
    ):
        body = function_body(controller, method)
        if body is None or fragment not in body:
            errors.append(f"TxPoolController::{method} must use the ordered control lane")

    assemble = builder.find("AuthorityService::assemble(")
    replay = builder.find("service.replay_persisted(")
    ready = builder.find("started.store(true, Ordering::Release)")
    if min(assemble, replay, ready) < 0 or not assemble < replay < ready:
        errors.append(
            "startup must assemble the chain-control consumer before persistence replay and publish "
            "RPC readiness only after replay"
        )
    assembly = function_body(authority_service, "assemble")
    if assembly is None or "run_ordered_chain_control_driver" not in assembly:
        errors.append("AuthorityService::assemble must own the ordered control consumer")

    driver = function_body(authority_service, "run_ordered_chain_control_driver")
    if driver is None:
        errors.append("the ordered control driver disappeared")
    else:
        compact_driver = " ".join(mask_rust_non_code(driver).split())
        for command, operation in (
            ("ClearPool", "clear_pool(arguments)"),
            ("ClearPipeline", "clear_pipeline()"),
        ):
            fragments = (
                f"ChainControl::{command}(command)",
                "command.into_parts()",
                f"service.{operation}.await",
                "drop(admission)",
                "settle_ordered_administration(responder, result",
            )
            cursor = 0
            for fragment in fragments:
                position = compact_driver.find(fragment, cursor)
                if position < 0:
                    errors.append(
                        f"{command} must consume, execute, release and only then respond; "
                        f"missing ordered fragment {fragment!r}"
                    )
                    break
                cursor = position + len(fragment)
        if compact_driver.count("command.into_parts()") != 2:
            errors.append("the ordered driver must consume exactly two admitted clear commands")
    return errors


def main() -> int:
    errors = [
        *validate_tx_pool_module_reachability(),
        *validate_chain_transition_publication(),
        *validate_proposal_history_provenance(),
        *validate_proposal_commit_liveness_path(),
        *validate_chain_apply_template_visibility(),
        *validate_startup_backpressure(),
        *validate_authority_mutation_publication(),
        *validate_authority_profiling_seams(),
        *validate_authority_failure_algebra(),
        *validate_transaction_query_failure_domains(),
        *validate_prepared_full_query(),
        *validate_atomic_apply_construction(),
        *validate_dependency_maintenance_successor(),
        *validate_dependency_maintenance_producers(),
        *validate_sparse_resource_set_transition(),
        *validate_finite_scheduler_owner_ring(),
        *validate_ready_priority_progress(),
        *validate_expiry_index_producers(),
        *validate_compute_capability_identity(),
        *validate_ordered_chain_error_domain(),
        *validate_production_vocabulary(),
        *validate_fallible_scratch_construction(),
        *validate_shared_variable_residency(),
        *validate_bounded_external_residency(),
        *validate_relay_full_hash_query_identity(),
        *validate_allocation_progress_protocol(),
        *validate_canonical_accepted_removal_set(),
        *validate_effect_publication_authority(),
        *validate_effect_publication_observation(),
        *validate_post_commit_wake_wiring(),
        *validate_released_input_projection(),
        *validate_direct_negative_evidence(),
        *validate_owner_transition_construction(),
        *validate_evidence_and_settlement_construction(),
        *validate_task_capability_lifecycle(),
        *validate_historical_bidirectional_coverage(),
        *validate_execution_topology_contract(),
    ]
    if errors:
        for error in errors:
            print(f"error: {error}", file=sys.stderr)
        return 1
    print(
        "validated cross-crate chain-tip publication, typed-capacity conditional "
        "proposal/commit offer refinement, startup ordering and "
        "bounded chain-control backpressure, exact relay full-hash query identity, "
        "plus authority post-commit wake coverage, "
        "claim-bound effect publication, centralized profiling seams, the typed "
        "authority failure algebra, split transaction-query failure domains, "
        "the architecture-owned prepared full-query protocol, "
        "sealed one-stamp atomic Apply, nonempty dependency-maintenance construction and "
        "closed dependency cut/ticket/projection producers, sparse "
        "resource set transitions, finite scheduler owner rings, exact Ready strict-priority "
        "OCC progress and cooperative rounds, explicit wall/monotonic "
        "clock domains and sealed bounded expiry index producers, total log-owned effect "
        "observation, exhaustive post-commit wake wiring, one projected released-input law, "
        "exact bounded direct-negative Accepted read receipts with negative canaries, "
        "fallible bounded production scratch with a syntax negative canary and shared "
        "variable-residency normal form, "
        "allocation terminals with generation/source progress and no timer retry, "
        "sealed canonical Accepted-removal set semantics, "
        "sealed evidence and total settlement classification plus "
        "a generated task/channel owner census with joined invalid retirement and "
        "cross-crate fixture quiescence and loopback endpoint identity with "
        "detached-task/drop-order/ambient-proxy negative canaries plus zero "
        "historical/current semantic "
        "ownership differences plus a closed Rust module graph plus "
        "current production vocabulary and "
        "execution-topology cost and shutdown order"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
