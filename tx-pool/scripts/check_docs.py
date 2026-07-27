#!/usr/bin/env python3
"""Validate the tx-pool documentation and tool index without building Rust."""

from __future__ import annotations

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
    "review-behaviors.json",
    "security-regression-manifest.json",
    "test-inventory.txt",
    "test-layout-manifest.json",
)


def markdown_files() -> list[Path]:
    return [TX_POOL / "README.md", *sorted(DOCS.glob("*.md"))]


def local_target(source: Path, raw_target: str) -> Path | None:
    target = raw_target.strip().split(maxsplit=1)[0].strip("<>")
    if not target or target.startswith("#") or "://" in target:
        return None
    path = unquote(target.split("#", 1)[0])
    return (source.parent / path).resolve()


def validate() -> list[str]:
    errors: list[str] = []
    files = markdown_files()
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
    for script in (
        "tx-pool/scripts/check_docs.py",
        "tx-pool/scripts/check_review_guide.py",
        "tx-pool/scripts/check_test_layout.py",
    ):
        if script not in ci:
            errors.append(f"tx-pool review CI omits {script}")

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
