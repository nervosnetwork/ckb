#!/usr/bin/env python3
"""Verify the repository-owned txpool-v8 handoff and its live source identity."""

from __future__ import annotations

import hashlib
import json
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent
REPOSITORY = ROOT.parents[3]
SUBJECT_COMMIT = "51d282345d1d83119c46cdde8f1115f14561b4ac"
SUBJECT_TREE = "1e19719c764c7349a178d7ac0b7bf4999542966f"


def fail(message: str) -> None:
    raise SystemExit(f"INVALID TXPOOL-V8 HANDOFF: {message}")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def load(path: Path) -> dict:
    try:
        value = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot load {path}: {error}")
    if not isinstance(value, dict):
        fail(f"top level is not object: {path}")
    return value


def command(*args: str) -> str:
    result = subprocess.run(
        args,
        cwd=REPOSITORY,
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        fail(f"command failed ({' '.join(args)}): {result.stderr.strip()}")
    return result.stdout.strip()


def main() -> int:
    if ROOT.name != "txpool-v8" or ROOT.parent.name != "handoff":
        fail("noncanonical handoff directory")
    manifest = load(ROOT / "MANIFEST.json")
    if manifest.get("schema") != "txpool-v8-repository-handoff-manifest-v1":
        fail("manifest schema mismatch")
    required_manifest_files = {
        "README.md",
        "HANDOFF.json",
        "CONTROL_KERNEL.json",
        "METHOD_LEDGER.json",
        "OPERATING_SYSTEM.md",
        "RESUME_PROMPT.md",
        "DOCUMENT_AUTHORITY.json",
        "DOCUMENT_AUDIT.json",
        "CKB_AUTHORITY_INPUT_LEDGER.md",
        "AUDIT_PLAN.json",
        "TODO.md",
        "FINDINGS_LEDGER.json",
        "USER_REPORT_VALIDATION.json",
        "EVIDENCE.json",
        "SUBAGENT_RESULTS.json",
        "CONTEXT_LOAD_POLICY.json",
        "VERIFY_HANDOFF.py",
    }
    if set(manifest.get("files", {})) != required_manifest_files:
        fail("manifest file set mismatch")
    for name, expected in manifest["files"].items():
        path = ROOT / name
        if not path.is_file():
            fail(f"missing handoff file: {name}")
        if sha256(path) != expected:
            fail(f"handoff file hash mismatch: {name}")

    handoff = load(ROOT / "HANDOFF.json")
    control = load(ROOT / "CONTROL_KERNEL.json")
    document_authority = load(ROOT / "DOCUMENT_AUTHORITY.json")
    document_audit = load(ROOT / "DOCUMENT_AUDIT.json")
    methods = load(ROOT / "METHOD_LEDGER.json")
    audit = load(ROOT / "AUDIT_PLAN.json")
    findings = load(ROOT / "FINDINGS_LEDGER.json")
    report_validation = load(ROOT / "USER_REPORT_VALIDATION.json")
    evidence = load(ROOT / "EVIDENCE.json")
    agents = load(ROOT / "SUBAGENT_RESULTS.json")
    context = load(ROOT / "CONTEXT_LOAD_POLICY.json")

    if handoff["single_current_root"] != control["single_current_root"]:
        fail("multiple current roots")
    if handoff["current_source"]["commit_before_handoff_artifacts"] != SUBJECT_COMMIT:
        fail("subject commit mismatch")
    if handoff["current_source"]["tree_before_handoff_artifacts"] != SUBJECT_TREE:
        fail("subject tree mismatch")
    if findings["subject_commit"] != SUBJECT_COMMIT or findings["subject_tree"] != SUBJECT_TREE:
        fail("findings subject mismatch")
    if audit["subject"]["commit_before_handoff_artifacts"] != SUBJECT_COMMIT:
        fail("audit subject mismatch")
    if evidence["source"]["commit_before_handoff_artifacts"] != SUBJECT_COMMIT:
        fail("evidence subject mismatch")
    if methods["authority"] != "REPOSITORY_OWNED_PRIMARY_METHOD_MEMORY":
        fail("method authority mismatch")
    if document_authority["purpose"] != "PREVENT_STALE_DOCUMENTS_FROM_BECOMING_A_SECOND_CURRENT_CONTROL_PLANE":
        fail("document authority matrix missing")
    if document_authority.get("schema") != "txpool-v8-document-authority-matrix-v2":
        fail("document authority schema mismatch")
    document_status = {
        item["path"]: item["status"] for item in document_authority["documents"]
    }
    if document_status.get("tx-pool/architecture-contract.json") != "CURRENT_STABLE_CONTRACT_NOT_LIVE_EXECUTION_STATE":
        fail("architecture contract role mismatch")
    if document_status.get("tx-pool/docs/ARCHITECTURE.md") != "RECONCILED_CURRENT_TRUE_SHARD_ARCHITECTURE_AND_OPEN_TERMINAL_BOUNDARY":
        fail("current architecture document not reconciled")
    if document_audit.get("schema") != "txpool-v8-document-audit-v1":
        fail("document audit schema mismatch")
    if document_audit.get("status") != "PASS_RECONCILED_NO_SECOND_CURRENT_ROOT":
        fail("document audit is not complete")
    if not handoff.get("document_state", "").startswith("RECONCILED_"):
        fail("handoff document state is not reconciled")
    if handoff.get("delivery_scope") != {
        "canonical": "G0_CURRENT_FROZEN_CANDIDATE_SET_NEXT_GENERATION",
        "open_architecture_class_research": "G0_OPEN_ARCHITECTURE_CLASS_RESEARCH_OPEN_FROZEN",
        "rule": "FINITE_CANDIDATE_ACCEPTANCE_MUST_NOT_BE_REPORTED_AS_OPEN_CLASS_GLOBAL_LOWER_BOUND_OR_ATTAINMENT",
    }:
        fail("delivery scope mismatch")
    required_method_ids = {
        "SOURCE_AND_STATE_AUTHORITY",
        "ROOT_CAUSAL_ENGINEERING",
        "PROOF_MODALITY_AND_EVIDENCE_BOUNDARY",
        "FINDINGS_FIRST_GLOBAL_AUDIT",
        "TEST_FAILURE_AND_GATE_DISCIPLINE",
        "HIGH_VALUE_PARALLELISM",
        "SUBTRACTIVE_MAXIMUM_ENGINEERING_EFFORT",
        "COLD_CONTINUITY_AND_HANDOFF",
    }
    if {item.get("id") for item in methods.get("methods", [])} != required_method_ids:
        fail("method ledger is incomplete")
    operating = (ROOT / "OPERATING_SYSTEM.md").read_text()
    for required in (
        "先收集全部候选，不立即补丁",
        "Primary 裁决",
        "局部放大 + 全局贯穿",
        "compact/重启后只加载首读集",
    ):
        if required not in operating:
            fail(f"operating discipline missing: {required}")
    order = {item["step"]: item["status"] for item in audit["execution_order"]}
    if not order.get(5, "").startswith("COMPLETE_EIGHT_ROOT_CLUSTERS") or not order.get(6, "").startswith("PRIMARY_SOURCE_REPRODUCTION_COMPLETE"):
        fail("audit resume step mismatch")
    todo = (ROOT / "TODO.md").read_text()
    for required in (
        "## 当前活动项",
        "C1：为冻结的八个根簇建立 deterministic",
        "不把 agent、报告、绿测试、文档自述或本 TodoList 当作 claim closure",
    ):
        if required not in todo:
            fail(f"progress dashboard discipline missing: {required}")
    if len(findings.get("blocking_candidates", [])) != 11:
        fail("terminal blocking candidate census mismatch")
    if len(findings.get("cluster_census", [])) != 8:
        fail("terminal blocker cluster census mismatch")
    if report_validation.get("summary") != {
        "upheld_new_blockers": 2,
        "upheld_existing_blocker_instances": 2,
        "upheld_control_plane_meta": 1,
        "conditional_assurance": 6,
        "defer_later_phase": 7,
        "refuted": 5,
        "new_blocker_clusters": [
            "EXTERNAL_EFFECT_OPERATION_LIFECYCLE_AND_TERMINAL_KNOWLEDGE",
            "POST_OWNER_PROJECTION_ALLOCATION_AND_APPLY_INFALLIBILITY",
        ],
        "deduplicated_existing_clusters": [
            "HELD_SHARD_CUT_AND_POLICY_READ_COHERENCE",
            "COHERENT_QUERY_CAPTURE_WITHOUT_POPULATION_WORK_UNDER_AUTHORITY_GUARDS",
        ],
    }:
        fail("neutral user report disposition summary mismatch")
    audited_paths = {item["path"] for item in document_audit.get("documents", [])}
    required_audited_paths = {
        "AGENTS.md",
        "README.md",
        "SECURITY.md",
        "tx-pool/AGENTS.md",
        "tx-pool/README.md",
        "tx-pool/CHANGELOG.md",
        "tx-pool/PROFILING.md",
        "tx-pool/.independent-execution-plan",
        "tx-pool/architecture-contract.json",
        "tx-pool/docs/ARCHITECTURE.md",
        "tx-pool/docs/BENCHMARK.md",
        "tx-pool/docs/PERFORMANCE.md",
        "tx-pool/docs/REVIEW_GUIDE.md",
        "tx-pool/docs/VALIDATION.md",
        "tx-pool/optimization-evidence/design/policy-read-feasibility.md",
        "tx-pool/docs/handoff/txpool-v8/",
    }
    if required_audited_paths - audited_paths:
        fail("document audit path coverage incomplete")
    authority_ledger = (ROOT / "CKB_AUTHORITY_INPUT_LEDGER.md").read_text()
    for required in (
        "## 九、当前主仓吸收状态",
        "CANONICAL_DELIVERY_GOAL_ACTIVE_IDENTITY_REFRESH_PENDING",
        "TRUE_SHARD_ROUTE_MIGRATION_SYNCED_TERMINAL_ACCEPTANCE_OPEN",
        "IMPLEMENTED_SCOPED_TERMINAL_COMPOSITION_OPEN",
    ):
        if required not in authority_ledger:
            fail(f"authority ledger absorption missing: {required}")
    if agents["live_agents_at_handoff"]:
        fail("handoff claims live subagents")
    if not {"CONVERSATION_HISTORY", "COMPACTION_SUMMARY"}.issubset(
        set(context["forbidden_execution_state"])
    ):
        fail("history noise policy missing")

    command("git", "cat-file", "-e", f"{SUBJECT_COMMIT}^{{commit}}")
    if subprocess.run(
        ("git", "merge-base", "--is-ancestor", SUBJECT_COMMIT, "HEAD"),
        cwd=REPOSITORY,
        check=False,
    ).returncode:
        fail("current HEAD does not contain the synchronized true-shard subject")
    if command("git", "rev-parse", f"{SUBJECT_COMMIT}^{{tree}}") != SUBJECT_TREE:
        fail("subject tree no longer resolves exactly")
    if command("git", "status", "--porcelain", "--untracked-files=normal"):
        fail("repository is not clean")

    runtime = (REPOSITORY / "tx-pool/src/authority/runtime.rs").read_text()
    if runtime.count("self.store.write()") != 5:
        fail("outer writer census changed before audit repair")
    if control["single_current_root"] != "B8_TRUE_SHARD_GLOBAL_TERMINAL_AUDIT_AND_ROOT_REPAIR_R1":
        fail("unexpected current root")
    if findings["global_claims"]["global_terminal_correctness"] != "OPEN_BLOCKING_CANDIDATES":
        fail("audit blocker boundary weakened")

    architecture = (REPOSITORY / "tx-pool/docs/ARCHITECTURE.md").read_text()
    for required in (SUBJECT_COMMIT, SUBJECT_TREE, control["single_current_root"]):
        if required not in architecture:
            fail(f"architecture identity missing: {required}")
    for forbidden in (
        "migration active",
        "This is now a normative design decision",
        "### 3.6 Current final A/B status",
    ):
        if forbidden in architecture:
            fail(f"stale architecture narrative survived: {forbidden}")

    checker = subprocess.run(
        ("python3", "-B", "tx-pool/scripts/check_all.py"),
        cwd=REPOSITORY,
        check=False,
        capture_output=True,
        text=True,
    )
    if checker.returncode != 0:
        fail(f"structural control checker failed: {checker.stdout.strip()} {checker.stderr.strip()}")

    durable = evidence["durable_pre_handoff"]
    checkpoint_manifest = Path(durable["checkpoint_path"]) / "MANIFEST.json"
    if checkpoint_manifest.is_file() and sha256(checkpoint_manifest) != durable["checkpoint_manifest_sha256"]:
        fail("checkpoint404 manifest mismatch")
    bundle = Path(durable["true_shard_bundle_path"])
    if bundle.is_file() and sha256(bundle) != durable["true_shard_bundle_sha256"]:
        fail("true-shard bundle mismatch")

    print(
        json.dumps(
            {
                "current_root": control["single_current_root"],
                "handoff_manifest_sha256": sha256(ROOT / "MANIFEST.json"),
                "head": command("git", "rev-parse", "HEAD^{commit}"),
                "subject": SUBJECT_COMMIT,
                "valid": True,
            },
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
