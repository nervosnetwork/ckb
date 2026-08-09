#!/usr/bin/env python3
"""Validate cross-crate production contracts that Rust types cannot seal."""

from __future__ import annotations

import json
import os
from pathlib import Path
import re
import sys


REPO_ROOT = Path(__file__).resolve().parents[2]
TX_POOL_SRC = REPO_ROOT / "tx-pool" / "src"
TX_POOL_ARCHITECTURE_CONTRACT = REPO_ROOT / "tx-pool" / "architecture-contract.json"
CHAIN_VERIFY = REPO_ROOT / "chain" / "src" / "verify.rs"
TX_POOL_SERVICE = REPO_ROOT / "tx-pool" / "src" / "service.rs"
TX_POOL_CONTROLLER = REPO_ROOT / "tx-pool" / "src" / "service" / "controller.rs"
TX_POOL_BUILDER = REPO_ROOT / "tx-pool" / "src" / "service" / "builder.rs"
TX_POOL_DISPATCH = REPO_ROOT / "tx-pool" / "src" / "service" / "dispatch.rs"
TX_POOL_MESSAGE = REPO_ROOT / "tx-pool" / "src" / "service" / "message.rs"
TX_POOL_AUTHORITY_SERVICE = (
    REPO_ROOT / "tx-pool" / "src" / "authority" / "service.rs"
)
TX_POOL_AUTHORITY_RUNTIME = (
    REPO_ROOT / "tx-pool" / "src" / "authority" / "runtime.rs"
)
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
TX_POOL_AUTHORITY_WORK = REPO_ROOT / "tx-pool" / "src" / "authority" / "work.rs"
TX_POOL_AUTHORITY_CHAIN = REPO_ROOT / "tx-pool" / "src" / "authority" / "chain.rs"
TX_POOL_AUTHORITY_VALIDATION = (
    REPO_ROOT / "tx-pool" / "src" / "authority" / "validation.rs"
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
    """Keep generation invalidation behind one typed, exhaustive boundary."""

    try:
        source = TX_POOL_AUTHORITY_SERVICE.read_text()
        masked = mask_rust_non_code(source)
    except (OSError, ValueError) as error:
        return [f"cannot inspect authority failure algebra: {error}"]

    errors: list[str] = []
    service_error = re.search(
        r"\benum\s+AuthorityServiceError\s*\{(?P<body>.*?)\n\}", masked, re.S
    )
    if service_error is None or "Integrity(AuthorityIntegrityFault)" not in service_error.group(
        "body"
    ):
        errors.append(
            "AuthorityServiceError must contain the typed Integrity(AuthorityIntegrityFault) "
            "boundary"
        )

    invalidity = re.search(
        r"\bstruct\s+AuthorityGenerationInvalidity\s*\(AuthorityIntegrityFault\)\s*;",
        masked,
    )
    if invalidity is None:
        errors.append(
            "AuthorityGenerationInvalidity must own AuthorityIntegrityFault directly"
        )
        constructor_source = masked
    else:
        constructor_source = (
            masked[: invalidity.start()]
            + " " * (invalidity.end() - invalidity.start())
            + masked[invalidity.end() :]
        )
        capability_declaration = re.search(
            r"(?P<attributes>(?:#\s*\[[^\]]*\]\s*)*)"
            r"pub\s*\(crate\)\s+struct\s+AuthorityGenerationInvalidity\s*"
            r"\(AuthorityIntegrityFault\)\s*;",
            masked,
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
    constructors = re.findall(
        r"\bAuthorityGenerationInvalidity\s*\(", constructor_source
    )
    if len(constructors) != 1:
        errors.append(
            "AuthorityGenerationInvalidity must have one production constructor at the "
            f"service settlement boundary, found {len(constructors)}"
        )

    settlement = function_body(source, "settle_operation_error")
    if settlement is None:
        errors.append("AuthorityService::settle_operation_error disappeared")
    else:
        settlement = mask_rust_non_code(settlement)
        if settlement.count("AuthorityServiceError::Integrity(fault)") != 1:
            errors.append(
                "settle_operation_error must classify the typed Integrity variant exactly once"
            )
        if settlement.count("AuthorityGenerationInvalidity(fault)") != 1:
            errors.append(
                "settle_operation_error must be the sole integrity-to-invalidity conversion"
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
            "QueuedWork::Resolve=>crate::authority::state::DependencyCut(sequence)",
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
            "letaccepted_removals=hashes.iter().filter(|hash|matches!(self.entries.get(*hash),Some(OwnedTx::Accepted(_))))",
            "letmembership=self.prepare_chain_projection(&accepted_removals,&HashMap::new())?",
            "self.collect_dependency_loss_keys(owner_refs.iter().copied())?.keys",
            "self.dependencies.plan_replacements(owner_refs.iter().copied().map(|owner|(Some(owner),None)))?.with_control(dependency_control)",
        ),
    )
    require_fragments(
        authority_methods[TX_POOL_AUTHORITY_CHAIN_TRANSITION].get(
            "plan_chain_transition", ""
        ),
        "TxPoolAuthority::plan_chain_transition",
        (
            "letmembership=self.prepare_chain_projection(&accepted_removals,&status_after)?",
            "changes.windows(2).any(",
            "self.dependencies.plan_primary_replacements(changes.iter().map(|change|(change.before.as_ref(),change.after.as_ref())),)?.with_control(control)",
        ),
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


def validate_expiry_index_producers() -> list[str]:
    """Bind both expiry planners to the sole bounded due-index producers."""

    try:
        plan = TX_POOL_AUTHORITY_PLAN.read_text()
        indexes = TX_POOL_AUTHORITY_INDEXES.read_text()
        remote_index = required_function_body(indexes, "due_remote")
        accepted_index = required_function_body(indexes, "due_accepted")
        remote_plan = required_function_body(plan, "plan_remote_expiry")
        accepted_plan = required_function_body(plan, "plan_accepted_expiry")
        compiler = required_function_body(plan, "compile_administrative_removal")
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
        r"pub\s*\(crate\)\s+async\s+fn\s+apply_chain_update\b.*?"
        r"->\s*Result\s*<\s*\(\)\s*,\s*AuthorityChainUpdateError\s*>",
        masked,
        re.S,
    )
    if signature is None:
        errors.append(
            "AuthorityService::apply_chain_update must return the closed chain error domain"
        )

    driver = function_body(source, "run_ordered_chain_control_driver")
    if driver is None:
        errors.append("run_ordered_chain_control_driver disappeared")
    else:
        for required in (
            "Err(AuthorityChainUpdateError::Cancelled)",
            "Err(AuthorityChainUpdateError::Integrity(fault))",
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
            "ChainControl::Reconcile(arguments)",
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
        )
        compact = " ".join(mapping.split())
        for required in required_mappings:
            if " ".join(required.split()) not in compact:
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
        replacement = required_function_body(
            plan, "collect_released_replacement_inputs"
        )
        administrative = required_function_body(
            plan, "collect_released_administrative_inputs"
        )
        shared = required_function_body(
            plan, "released_input_survives_final_owner_set"
        )
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
    return errors


def validate_evidence_and_settlement_construction() -> list[str]:
    """Bind legal evidence to sealed producers and one total settlement classifier."""

    try:
        chain = TX_POOL_AUTHORITY_CHAIN.read_text()
        validation = TX_POOL_AUTHORITY_VALIDATION.read_text()
        plan = TX_POOL_AUTHORITY_PLAN.read_text()
        work = TX_POOL_AUTHORITY_WORK.read_text()
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
    except (OSError, ValueError) as error:
        return [f"cannot inspect sealed evidence and settlement construction: {error}"]

    errors: list[str] = []
    compact_validation = "".join(mask_rust_non_code(validation).split())
    if validation.count("let seal = AdmissionValidationSeal(());") != 2:
        errors.append("final and direct validation must be the two seal construction cuts")
    for constructor in (
        "FinalAdmissionSubject::new(seal,key.clone(),expected,view.clone(),dependency_cut)",
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
    for constructor in ("ResolvedFacts::from_resolution(", "VerifiedFacts::from_verification("):
        if constructor in production_without_work:
            errors.append(f"sealed settlement evidence escaped authority work via {constructor!r}")
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
            ".prepare()",
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
        *validate_expiry_index_producers(),
        *validate_compute_capability_identity(),
        *validate_ordered_chain_error_domain(),
        *validate_production_vocabulary(),
        *validate_effect_publication_authority(),
        *validate_effect_publication_observation(),
        *validate_post_commit_wake_wiring(),
        *validate_released_input_projection(),
        *validate_evidence_and_settlement_construction(),
        *validate_execution_topology_contract(),
    ]
    if errors:
        for error in errors:
            print(f"error: {error}", file=sys.stderr)
        return 1
    print(
        "validated cross-crate chain-tip publication, startup ordering and "
        "bounded chain-control backpressure plus authority post-commit wake coverage, "
        "claim-bound effect publication, centralized profiling seams, the typed "
        "authority failure algebra, split transaction-query failure domains, "
        "the architecture-owned prepared full-query protocol, "
        "sealed one-stamp atomic Apply, nonempty dependency-maintenance construction and "
        "closed dependency cut/ticket/projection producers, sparse "
        "resource set transitions, finite scheduler owner rings and sealed bounded expiry "
        "index producers, total log-owned effect "
        "observation, exhaustive post-commit wake wiring, one projected released-input law, "
        "sealed evidence and total settlement classification plus "
        "a closed Rust module graph plus current production vocabulary and "
        "execution-topology cost and shutdown order"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
