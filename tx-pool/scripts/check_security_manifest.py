#!/usr/bin/env python3
"""Validate and project the single current tx-pool architecture contract."""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
CONTRACT = ROOT / "tx-pool/architecture-contract.json"
HANDOFF = ROOT / "tx-pool/docs/handoff/txpool-v8/HANDOFF.json"
OPERATING = ROOT / "tx-pool/docs/handoff/txpool-v8/OPERATING_SYSTEM.md"
MANIFEST = ROOT / "tx-pool/security-regression-manifest.json"
PROGRESS = ROOT / "tx-pool/.release-progress"

METHOD = "txpool-production-architecture-v4"
SCHEMA = 40
GOAL = (
    "从 CKB 原始协议与交易依赖/冲突语义出发，在一致性、安全、兼容与资源有界为硬约束下，"
    "求出可证明全局静态最优、实测性能最强且实现/证明复杂度最小的 tx-pool 架构；"
    "独立工作最大并行，耦合事实只在唯一权威的最小原子切口排序。"
)
CURRENT_ROOT = "B8_TRUE_SHARD_GLOBAL_TERMINAL_AUDIT_AND_ROOT_REPAIR_R1"
PHASES = [
    "terminal_correctness_and_root_repair",
    "hard_and_static_proof",
    "measured_performance",
    "complexity_minimum",
    "security",
    "acceptance",
]
CLAIMS = [
    "HARD_FEASIBILITY",
    "STATIC_GLOBAL_BOTTOM",
    "MEASURED_STRONGEST",
    "SEMANTIC_ZERO_AND_ENGINEERING_MINIMUM",
    "FINAL_ADVERSARIAL_SECURITY",
    "NEW_COLD_JOINED_ACCEPTED_UNIVERSE",
]
AUTHORITIES = [
    "AGENTS.md",
    "tx-pool/AGENTS.md",
    "tx-pool/architecture-contract.json",
    "tx-pool/docs/handoff/txpool-v8/HANDOFF.json",
    "tx-pool/docs/handoff/txpool-v8/CONTROL_KERNEL.json",
    "tx-pool/docs/handoff/txpool-v8/METHOD_LEDGER.json",
    "tx-pool/docs/handoff/txpool-v8/OPERATING_SYSTEM.md",
    "tx-pool/docs/handoff/txpool-v8/RESUME_PROMPT.md",
    "tx-pool/docs/handoff/txpool-v8/AUDIT_PLAN.json",
    "tx-pool/docs/handoff/txpool-v8/FINDINGS_LEDGER.json",
    "tx-pool/docs/handoff/txpool-v8/DOCUMENT_AUTHORITY.json",
    "tx-pool/docs/handoff/txpool-v8/DOCUMENT_AUDIT.json",
    "tx-pool/docs/handoff/txpool-v8/EVIDENCE.json",
    "tx-pool/docs/handoff/txpool-v8/SUBAGENT_RESULTS.json",
    "tx-pool/docs/handoff/txpool-v8/CONTEXT_LOAD_POLICY.json",
    "tx-pool/docs/handoff/txpool-v8/CKB_AUTHORITY_INPUT_LEDGER.md",
    "tx-pool/docs/handoff/txpool-v8/VERIFY_HANDOFF.py",
    "tx-pool/scripts/check_security_manifest.py",
    "tx-pool/scripts/check_all.py",
]
CONTRACT_FIELDS = {
    "schema_version",
    "method_id",
    "final_goal",
    "authority",
    "method",
    "current",
    "claims",
    "phase_order",
    "continuous_gates",
    "release_surface",
    "historical_convergence",
    "historical_evidence",
    "safety_guards",
}


def canonical(value):
    return json.dumps(
        value, ensure_ascii=False, sort_keys=True, separators=(",", ":")
    ).encode()


def digest(data):
    return hashlib.sha256(data).hexdigest()


def file_digest(path):
    return digest(path.read_bytes())


def need(condition, message, errors):
    if not condition:
        errors.append(message)


def object_value(value):
    return value if isinstance(value, dict) else {}


def read_json(path, errors):
    try:
        value = json.loads(path.read_text())
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        errors.append(f"cannot parse {path}: {error}")
        return {}
    need(isinstance(value, dict), f"{path}:object", errors)
    return value if isinstance(value, dict) else {}


def git(*arguments):
    result = subprocess.run(
        ["git", *arguments],
        cwd=ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        check=False,
    )
    return result.stdout.decode().strip() if result.returncode == 0 else None


def validate_contract(contract, handoff, operating, errors):
    need(set(contract) == CONTRACT_FIELDS, "contract:fields", errors)
    need(contract.get("schema_version") == SCHEMA, "contract:schema", errors)
    need(contract.get("method_id") == METHOD, "contract:method", errors)
    goal = object_value(contract.get("final_goal"))
    need(goal.get("verbatim_zh") == GOAL, "contract:goal", errors)
    need(goal.get("canonical_delivery_goal")
         == "G0_CURRENT_FROZEN_CANDIDATE_SET_NEXT_GENERATION",
         "contract:canonical_delivery_goal", errors)
    need(goal.get("open_architecture_class_research")
         == "G0_OPEN_ARCHITECTURE_CLASS_RESEARCH_OPEN_FROZEN",
         "contract:open_class_boundary", errors)
    need(handoff.get("objective_literal") == GOAL, "handoff:goal", errors)

    authority = object_value(contract.get("authority"))
    need(
        authority
        == {
            "current_state": "tx-pool/docs/handoff/txpool-v8/HANDOFF.json",
            "control_kernel": "tx-pool/docs/handoff/txpool-v8/CONTROL_KERNEL.json",
            "semantic_method": "tx-pool/docs/handoff/txpool-v8/METHOD_LEDGER.json",
            "audit_plan": "tx-pool/docs/handoff/txpool-v8/AUDIT_PLAN.json",
            "findings": "tx-pool/docs/handoff/txpool-v8/FINDINGS_LEDGER.json",
            "document_authority": "tx-pool/docs/handoff/txpool-v8/DOCUMENT_AUTHORITY.json",
            "document_audit": "tx-pool/docs/handoff/txpool-v8/DOCUMENT_AUDIT.json",
            "authority_inputs": "tx-pool/docs/handoff/txpool-v8/CKB_AUTHORITY_INPUT_LEDGER.md",
            "architecture_contract_role": "STABLE_OBJECTIVE_HARD_CONSTRAINT_PHASE_AND_ACCEPTANCE_CONTRACT_NOT_LIVE_EXECUTION_STATE",
            "projector": "tx-pool/scripts/check_security_manifest.py",
            "entrypoint": "tx-pool/scripts/check_all.py",
            "generated_manifest": "tx-pool/security-regression-manifest.json",
            "generated_progress": "tx-pool/.release-progress",
            "external_checkpoint_role": "durable_mirror_only",
            "review_role": "advisory_independent_evidence_not_gate",
        },
        "contract:authority",
        errors,
    )

    method = object_value(contract.get("method"))
    need(method.get("relations") == ["Q_H", "C", "I_D", "MU"],
         "contract:relations", errors)
    need(method.get("production_bridge") == "RHO", "contract:bridge", errors)

    current = object_value(contract.get("current"))
    need(current.get("state") == "terminal_audit_and_root_repair", "current:state", errors)
    need(current.get("phase") == PHASES[0], "current:phase", errors)
    need(current.get("root") == CURRENT_ROOT, "current:root", errors)
    need(handoff.get("single_current_root") == CURRENT_ROOT,
         "handoff:root", errors)
    need(CURRENT_ROOT in operating, "operating:root", errors)
    need("唯一有序执行计划" in operating,
         "operating:single_plan", errors)
    need(isinstance(current.get("confirmed_blockers"), list)
         and bool(current.get("confirmed_blockers")), "current:blockers", errors)
    need(isinstance(current.get("success"), list)
         and bool(current.get("success")), "current:success", errors)
    need(isinstance(current.get("forbidden_shortcuts"), list)
         and bool(current.get("forbidden_shortcuts")), "current:forbidden", errors)

    base = object_value(current.get("implementation_base"))
    commit, tree = base.get("commit"), base.get("tree")
    need(isinstance(commit, str) and git("rev-parse", "--verify", f"{commit}^{{commit}}") == commit,
         "current:base_commit", errors)
    need(isinstance(tree, str) and git("rev-parse", f"{commit}^{{tree}}") == tree,
         "current:base_tree", errors)

    need(contract.get("phase_order") == PHASES, "contract:phase_order", errors)
    claims = contract.get("claims")
    need(isinstance(claims, list), "claims:type", errors)
    if isinstance(claims, list):
        need([object_value(item).get("id") for item in claims] == CLAIMS,
             "claims:order", errors)
        for item in claims:
            claim = object_value(item)
            need(set(claim) == {"id", "phase", "status"},
                 f"claim:{claim.get('id')}:fields", errors)
            need(claim.get("phase") in PHASES,
                 f"claim:{claim.get('id')}:phase", errors)
            need(claim.get("status") == "open",
                 f"claim:{claim.get('id')}:premature_closure", errors)

    gates = object_value(contract.get("continuous_gates"))
    need(isinstance(gates.get("architecture"), list)
         and "no_partial_migration_can_be_called_a_candidate"
         in gates.get("architecture", []), "gates:partial_migration", errors)
    need(isinstance(gates.get("complexity_measures"), list)
         and {"production_LoC", "locks", "trusted_semantic_kernel"}
         <= set(gates.get("complexity_measures", [])), "gates:complexity", errors)

    release = object_value(contract.get("release_surface"))
    need(bool(release.get("compatibility_policy"))
         and bool(release.get("residual_risk_policy")), "contract:release", errors)
    history = object_value(contract.get("historical_convergence"))
    need(bool(history.get("historical_develop_baseline")) and bool(history.get("rule")),
         "contract:convergence", errors)

    safety = object_value(contract.get("safety_guards"))
    required_true = {
        "preserve_durable_evidence",
        "true_shard_completion_required",
        "true_shard_requires_real_disjoint_commit_overlap_without_global_serial_fallback",
        "rollback_to_single_lock_forbidden",
        "unbounded_live_develop_follow_forbidden",
        "targeted_pre_acceptance_develop_reconciliation_required",
        "external_partner_is_not_a_gate",
        "package_or_protocol_progress_is_not_product_progress",
        "repository_handoff_is_the_only_live_current_state",
        "stale_documents_cannot_override_source_or_manifest_bound_handoff",
    }
    need(set(safety) == required_true and all(safety.get(key) is True for key in required_true),
         "contract:safety", errors)

    documents = [
        ROOT / "tx-pool/README.md",
        ROOT / "tx-pool/CHANGELOG.md",
        *(ROOT / "tx-pool/docs").glob("*.md"),
    ]
    for path in documents:
        text = path.read_text()
        need("/optimization_goal" not in text,
             f"docs:retired_goal_pointer:{path.relative_to(ROOT)}", errors)
    architecture = (ROOT / "tx-pool/docs/ARCHITECTURE.md").read_text()
    need("Current terminal-audit root" in architecture
         and CURRENT_ROOT in architecture, "docs:terminal_audit_boundary", errors)


def method_identity(contract):
    hashes = {path: file_digest(ROOT / path) for path in AUTHORITIES}
    return {
        "method_id": METHOD,
        "contract_sha256": digest(canonical(contract)),
        "authority_hashes": hashes,
        "bundle_sha256": digest(canonical({"method_id": METHOD, "authority_hashes": hashes})),
    }


def projection(contract):
    return {
        "schema_version": 2,
        "kind": "current_state_projection_not_proof",
        "method_identity": method_identity(contract),
        "checkout": copy.deepcopy(contract["current"]["implementation_base"]),
        "current": copy.deepcopy(contract["current"]),
        "claims": copy.deepcopy(contract["claims"]),
        "phase_order": copy.deepcopy(contract["phase_order"]),
        "warnings": [
            "all product claims remain open",
            "the ordinary true-shard route migration is synchronized but terminal correctness has blocking findings",
            "generated projection is not the live control plane; use the manifest-bound repository handoff",
        ],
    }


def progress(value):
    current = value["current"]
    claims = ", ".join(f"{item['id']}={item['status']}" for item in value["claims"])
    return "\n".join(
        [
            "# Tx-Pool Current Architecture Progress",
            "",
            "Generated mechanically; disposable and not proof.",
            "",
            f"- method: `{METHOD}`",
            f"- state/phase: `{current['state']}` / `{current['phase']}`",
            f"- sole root: `{current['root']}`",
            f"- implementation base: `{current['implementation_base']['commit']}` / "
            f"`{current['implementation_base']['tree']}`",
            f"- claims: {claims}",
            "- next: reproduce and cluster every terminal-audit blocker, then implement one minimal root per upheld cluster; "
            "do not advance performance, final security or Acceptance first.",
            "",
        ]
    )


def atomic_write(path, data):
    descriptor, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        with os.fdopen(descriptor, "wb") as handle:
            handle.write(data)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def self_test(contract, handoff, operating):
    failures = []
    variants = []
    changed = copy.deepcopy(contract)
    changed["final_goal"]["verbatim_zh"] = "mutated"
    variants.append(("goal", changed))
    changed = copy.deepcopy(contract)
    changed["current"]["root"] = "PACKAGE_ONLY_PROGRESS"
    variants.append(("root", changed))
    changed = copy.deepcopy(contract)
    changed["claims"][0]["status"] = "proved"
    variants.append(("claim", changed))
    for label, variant in variants:
        errors = []
        validate_contract(variant, handoff, operating, errors)
        if not errors:
            failures.append(f"{label} negative canary escaped")
    return failures


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write-projections", action="store_true")
    parser.add_argument("--print-method-identity", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    arguments = parser.parse_args()

    errors = []
    try:
        operating = OPERATING.read_text()
    except (OSError, UnicodeError) as error:
        print(f"ERROR: cannot read operating manual: {error}", file=sys.stderr)
        return 1
    contract = read_json(CONTRACT, errors)
    handoff = read_json(HANDOFF, errors)
    validate_contract(contract, handoff, operating, errors)
    if arguments.self_test:
        errors.extend(self_test(contract, handoff, operating))
        if not errors:
            print("structural negative canaries passed")

    value = projection(contract)
    manifest_data = json.dumps(
        value, ensure_ascii=False, indent=2, sort_keys=True
    ).encode() + b"\n"
    progress_data = progress(value).encode()

    if not errors and arguments.write_projections:
        atomic_write(MANIFEST, manifest_data)
        atomic_write(PROGRESS, progress_data)
    elif not errors and not arguments.self_test:
        need(MANIFEST.is_file() and MANIFEST.read_bytes() == manifest_data,
             "generated manifest stale; use --write-projections", errors)
        need(PROGRESS.is_file() and PROGRESS.read_bytes() == progress_data,
             "generated progress stale; use --write-projections", errors)

    if arguments.print_method_identity:
        print(json.dumps(
            {"method_identity": value["method_identity"], "checkout": value["checkout"]},
            ensure_ascii=False,
            indent=2,
            sort_keys=True,
        ))
    for error in errors:
        print(f"ERROR: {error}", file=sys.stderr)
    if errors:
        return 1
    print(f"validated {METHOD}: state={value['current']['state']} "
          f"root={value['current']['root']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
