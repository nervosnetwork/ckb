#!/usr/bin/env python3
"""Validate the tx-pool documentation and tool index without building Rust."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path
import re
import sys
from urllib.parse import unquote


REPO_ROOT = Path(__file__).resolve().parents[2]
TX_POOL = REPO_ROOT / "tx-pool"
DOCS = TX_POOL / "docs"
VALIDATION = DOCS / "VALIDATION.md"
DOC_INDEX = TX_POOL / "README.md"
CI_WORKFLOW = REPO_ROOT / ".github" / "workflows" / "ci_tx_pool_review.yaml"
BEHAVIOR_REGISTRY = TX_POOL / "review-behaviors.json"
MARKDOWN_LINK = re.compile(r"\[[^]]*\]\(([^)]+)\)")
RETIRED_PATHS = (
    "devtools/check_tx_pool_review_guide.py",
    "devtools/check_tx_pool_test_layout.py",
    "devtools/check_tx_pool_security_manifest.py",
    "devtools/tx_pool_bench.py",
    "tx-pool/ARCHITECTURE.md",
    "tx-pool/ARCHITECTURE_AUDIT.md",
    "tx-pool/IMPLEMENTATION_PLAN.md",
    "tx-pool/REVIEW_GUIDE.md",
    "tx-pool/security-regression-ledger.md",
    "tx-pool/docs/ARCHITECTURE_AUDIT.md",
    "tx-pool/docs/IMPLEMENTATION_PLAN.md",
    "tx-pool/docs/PIPELINE.md",
    "tx-pool/docs/README.md",
    "tx-pool/docs/SECURITY_REGRESSION_LEDGER.md",
    "tx-pool/docs/TOOLS.md",
    "tx-pool/scripts/check_develop_refinement.py",
    "tx-pool/scripts/generate_mutation_matrix.py",
)
RETIRED_DOCUMENT_NAMES = {
    "ARCHITECTURE_AUDIT.md",
    "IMPLEMENTATION_PLAN.md",
    "PIPELINE.md",
    "SECURITY_REGRESSION_LEDGER.md",
    "TOOLS.md",
}
MACHINE_CONTRACTS = (
    "architecture-contract.json",
    "integration-impact.json",
    "mutation-acceptance-lock.json",
    "mutation-result-lock.json",
    "review-behaviors.json",
    "security-regression-manifest.json",
    "test-inventory.txt",
    "test-layout-manifest.json",
)
OPTIMIZATION_GOAL_REFERENCE = "architecture-contract.json#/optimization_goal"
STATUS_FIELDS = (
    "state",
    "completed",
    "active",
    "rho_C",
    "rho_A",
    "claim",
)


def markdown_files() -> list[Path]:
    return [TX_POOL / "README.md", *sorted(DOCS.glob("*.md"))]


def optimization_goal_documents() -> list[Path]:
    hidden_current_documents = (
        TX_POOL / ".independent-execution-plan",
        TX_POOL / ".release-progress",
    )
    documents = sorted({*TX_POOL.glob("*.md"), *DOCS.glob("*.md")})
    documents.extend(path for path in hidden_current_documents if path.exists())
    tx_pool_agents = TX_POOL / "AGENTS.md"
    if tx_pool_agents.exists():
        documents.append(tx_pool_agents)
    return documents


def local_target(source: Path, raw_target: str) -> Path | None:
    target = raw_target.strip().split(maxsplit=1)[0].strip("<>")
    if not target or target.startswith("#") or "://" in target:
        return None
    path = unquote(target.split("#", 1)[0])
    return (source.parent / path).resolve()


def expected_progress_machine_state(contract: dict, manifest: dict) -> dict[str, str]:
    status = manifest["convergence_status"]
    construction = status["construction_rank"]
    acceptance = status["acceptance_rank"]
    claim = contract["optimality_protocol"]["current_claim"]
    return {
        "state": str(status["state"]).capitalize(),
        "completed": ", ".join(status["completed_phases"]) or "none",
        "active": str(status["active_phase"] or "none"),
        "rho_C": str(tuple(len(construction[field]) for field in construction)),
        "rho_A": (
            "undefined"
            if acceptance is None
            else str(tuple(len(acceptance[field]) for field in acceptance))
        ),
        "claim": str(claim),
    }


def progress_machine_state(text: str) -> dict[str, str]:
    blocks = re.findall(r"```text\n(?P<body>.*?)\n```", text, re.S)
    if not blocks:
        return {}
    state: dict[str, str] = {}
    for line in blocks[0].splitlines():
        if "=" not in line:
            continue
        key, value = (part.strip() for part in line.split("=", 1))
        if key in STATUS_FIELDS:
            state[key] = value
    return state


def validate() -> list[str]:
    errors: list[str] = []
    files = markdown_files()
    for source in optimization_goal_documents():
        if OPTIMIZATION_GOAL_REFERENCE not in source.read_text():
            errors.append(
                "tx-pool document omits the canonical final optimization goal: "
                f"{source.relative_to(REPO_ROOT)}"
            )
    root_agents = REPO_ROOT / "AGENTS.md"
    if root_agents.exists():
        agents_digest = hashlib.sha256(root_agents.read_bytes()).hexdigest()
        progress = (TX_POOL / ".release-progress").read_text()
        if agents_digest not in progress:
            errors.append(
                "current release progress does not bind the ignored AGENTS.md digest"
            )
    try:
        contract = json.loads((TX_POOL / "architecture-contract.json").read_text())
        manifest = json.loads((TX_POOL / "security-regression-manifest.json").read_text())
        progress_text = (TX_POOL / ".release-progress").read_text()
        expected_state = expected_progress_machine_state(contract, manifest)
        observed_state = progress_machine_state(progress_text)
        if observed_state != expected_state:
            errors.append(
                "current release progress machine-state projection differs: "
                f"expected={expected_state}, observed={observed_state}"
            )
    except (OSError, KeyError, TypeError, json.JSONDecodeError) as error:
        errors.append(f"cannot derive current release progress projection: {error}")
    for source in files:
        text = source.read_text()
        for raw_target in MARKDOWN_LINK.findall(text):
            target = local_target(source, raw_target)
            if target is not None and not target.exists():
                errors.append(
                    f"broken local link in {source.relative_to(REPO_ROOT)}: {raw_target}"
                )
    index = DOC_INDEX.read_text()
    for document in sorted(DOCS.glob("*.md")):
        if document == DOC_INDEX:
            continue
        if document.name not in index:
            errors.append(f"documentation index omits {document.name}")

    validation = VALIDATION.read_text()
    for script in sorted((TX_POOL / "scripts").glob("*.py")):
        relative = script.relative_to(REPO_ROOT).as_posix()
        if relative not in validation:
            errors.append(f"validation guide omits {relative}")

    for name in MACHINE_CONTRACTS:
        if name not in index:
            errors.append(f"crate README omits machine contract {name}")
        if name not in validation:
            errors.append(f"validation guide does not explain machine contract {name}")

    ci = CI_WORKFLOW.read_text()
    canonical_ci_command = "python3 tx-pool/scripts/check_all.py --light"
    if canonical_ci_command not in ci:
        errors.append("tx-pool review CI omits the canonical light contract gate")
    component_commands = re.findall(r"python3 tx-pool/scripts/check_[A-Za-z0-9_]+\.py", ci)
    if component_commands != ["python3 tx-pool/scripts/check_all.py"]:
        errors.append(
            "tx-pool review CI must call only check_all.py instead of copying component gates"
        )

    try:
        registry = json.loads(BEHAVIOR_REGISTRY.read_text())
    except (OSError, json.JSONDecodeError) as error:
        errors.append(f"cannot derive CI roots from review-behaviors.json: {error}")
    else:
        evidence_paths: list[str] = []
        for behavior in registry.get("behaviors", []):
            for owner in behavior.get("implementation_owners", []):
                path = owner.get("path")
                if isinstance(path, str):
                    evidence_paths.append(path)
        for field in ("workspace_evidence", "integration_evidence"):
            for evidence in registry.get(field, []):
                path = evidence.get("path")
                if isinstance(path, str):
                    evidence_paths.append(path)
        roots = {
            Path(path).parts[0]
            for path in evidence_paths
            if Path(path).parts
        }
        workflow_paths = re.findall(
            r"(?m)^\s*-\s*['\"](?P<path>[^'\"]+)['\"]\s*$", ci
        )
        for root in sorted(roots):
            pattern = f"{root}/**"
            if workflow_paths.count(pattern) < 2:
                errors.append(
                    "tx-pool review CI must cover every behavior-evidence root in "
                    f"pull_request and push paths: missing {pattern}"
                )

    drift_surfaces = [
        *files,
        *sorted((REPO_ROOT / ".github" / "workflows").glob("*.yaml")),
        *sorted(TX_POOL.glob("*.json")),
    ]
    for source in drift_surfaces:
        text = source.read_text()
        if source.suffix == ".md":
            for retired in RETIRED_DOCUMENT_NAMES:
                if retired in text:
                    errors.append(
                        f"retired tx-pool document in "
                        f"{source.relative_to(REPO_ROOT)}: {retired}"
                    )
        for retired in RETIRED_PATHS:
            if retired in text:
                errors.append(
                    f"retired tx-pool path in {source.relative_to(REPO_ROOT)}: {retired}"
                )
        if source.suffix == ".md":
            for retired_term in (
                "G5.3c",
                "P9.7g",
                "ComputeLeaseId",
                "VerifyLease",
                "CommitSession",
            ):
                if retired_term in text:
                    errors.append(
                        f"retired implementation term in "
                        f"{source.relative_to(REPO_ROOT)}: {retired_term}"
                    )
    return errors


def main() -> int:
    errors = validate()
    if errors:
        for error in errors:
            print(f"error: {error}", file=sys.stderr)
        return 1
    print(
        f"validated {len(markdown_files())} tx-pool documents and "
        f"{len(list((TX_POOL / 'scripts').glob('*.py')))} documented tools"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
