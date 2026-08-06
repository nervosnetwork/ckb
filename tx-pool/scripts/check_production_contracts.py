#!/usr/bin/env python3
"""Validate cross-crate production contracts that Rust types cannot seal."""

from __future__ import annotations

import os
from pathlib import Path
import re
import sys


REPO_ROOT = Path(__file__).resolve().parents[2]
CHAIN_VERIFY = REPO_ROOT / "chain" / "src" / "verify.rs"
TX_POOL_SERVICE = REPO_ROOT / "tx-pool" / "src" / "service.rs"
TX_POOL_CONTROLLER = REPO_ROOT / "tx-pool" / "src" / "service" / "controller.rs"
TX_POOL_BUILDER = REPO_ROOT / "tx-pool" / "src" / "service" / "builder.rs"
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
TX_POOL_BENCHMARK = REPO_ROOT / "tx-pool" / "src" / "benchmark.rs"
TX_POOL_AUTHORITY_PLAN = REPO_ROOT / "tx-pool" / "src" / "authority" / "plan.rs"
TX_POOL_AUTHORITY_STATE = REPO_ROOT / "tx-pool" / "src" / "authority" / "state.rs"
TX_POOL_AUTHORITY_WORK = REPO_ROOT / "tx-pool" / "src" / "authority" / "work.rs"
RUST_CHAR_LITERAL = re.compile(
    r"'(?:[^'\\\r\n]|\\(?:[nrt0\\'\"]|x[0-9A-Fa-f]{2}|u\{[0-9A-Fa-f_]{1,6}\}))'"
)
RUST_RAW_STRING = re.compile(r'(?:br|cr|r)(?P<hashes>#{0,255})"')
AUTHORITY_MUTATION = re.compile(r"\.\s*apply(?:_[a-z][A-Za-z0-9_]*)?\s*\(")
POST_COMMIT_PUBLICATION = re.compile(
    r"\.\s*(?:publish_committed|publish_post_commit(?:_pair)?)\s*\("
)
EARLY_EXIT = re.compile(r"\b(?:return|break|continue)\b|\?")


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


def rust_impl_methods(source: str, impl_name: str) -> list[tuple[str, str, int]]:
    """Return concrete inherent-impl method bodies as masked source."""

    masked = mask_rust_non_code(source)
    declarations = list(
        re.finditer(rf"\bimpl\s+{re.escape(impl_name)}\s*\{{", masked)
    )
    if len(declarations) != 1:
        raise ValueError(
            f"expected one inherent impl {impl_name}, found {len(declarations)}"
        )
    opening = masked.find("{", declarations[0].start())
    closing = matching_brace(masked, opening)
    if closing is None:
        raise ValueError(f"inherent impl {impl_name} has no closing brace")

    methods: list[tuple[str, str, int]] = []
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
        "fn try_effect_publication(&self) -> EffectPublicationState",
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
    service = TX_POOL_SERVICE.read_text()
    controller = TX_POOL_CONTROLLER.read_text()
    builder = TX_POOL_BUILDER.read_text()
    message = TX_POOL_MESSAGE.read_text()
    authority_service = TX_POOL_AUTHORITY_SERVICE.read_text()

    if "const CHAIN_CONTROL_CHANNEL_SIZE: usize = 1;" not in service:
        errors.append("the ordered chain-control boundary must retain capacity one")
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
    for method, fragment in (
        ("clear_pool", "send_chain_control!(self, ClearPool"),
        ("clear_verify_queue", "send_chain_control!(self, ClearPipeline"),
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
    return errors


def main() -> int:
    errors = [
        *validate_chain_transition_publication(),
        *validate_startup_backpressure(),
        *validate_authority_mutation_publication(),
        *validate_authority_profiling_seams(),
        *validate_authority_failure_algebra(),
        *validate_compute_capability_identity(),
        *validate_ordered_chain_error_domain(),
        *validate_production_vocabulary(),
        *validate_effect_publication_authority(),
    ]
    if errors:
        for error in errors:
            print(f"error: {error}", file=sys.stderr)
        return 1
    print(
        "validated cross-crate chain-tip publication, startup ordering and "
        "bounded chain-control backpressure plus authority post-commit wake coverage, "
        "claim-bound effect publication, centralized profiling seams, the typed "
        "authority failure algebra and current production vocabulary"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
