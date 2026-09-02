#!/usr/bin/env python3
"""Validate stable tx-pool contracts and project the sole live state."""

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
CONTROL_ROOT = ROOT / "tx-pool/control/txpool-v8"
CONTRACT = ROOT / "tx-pool/architecture-contract.json"
STATE = CONTROL_ROOT / "STATE.json"
CONTROL = CONTROL_ROOT / "CONTROL_KERNEL.json"
AUDIT = CONTROL_ROOT / "AUDIT_PLAN.json"
FINDINGS = CONTROL_ROOT / "FINDINGS_LEDGER.json"
MANIFEST = ROOT / "tx-pool/security-regression-manifest.json"
PROGRESS = ROOT / "tx-pool/.release-progress"
TODO = CONTROL_ROOT / "TODO.md"

METHOD = "txpool-production-architecture-v5"
SCHEMA = 41
GOAL = (
    "从 CKB 原始协议与交易依赖/冲突语义出发，在一致性、安全、兼容与资源有界为硬约束下，"
    "求出可证明全局静态最优、实测性能最强且实现/证明复杂度最小的 tx-pool 架构；"
    "独立工作最大并行，耦合事实只在唯一权威的最小原子切口排序。"
)
PRIMARY_ROLE = "G0_ACCOUNTABLE_PRIMARY_ENGINEERING_OWNER"
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
DECISIONS = [f"CKB-AUTH-{index:04d}" for index in range(1, 10)]
CONTRACT_FIELDS = {
    "schema_version",
    "method_id",
    "role",
    "final_goal",
    "authority_topology",
    "primary_role_boundary",
    "authority_decision_envelope",
    "method_boundary",
    "claim_vocabulary",
    "phase_order",
    "optimization_contract",
    "continuous_gates",
    "release_surface",
    "candidate_generation_rule",
    "safety_guards",
}
CONTRACT_OBJECT_FIELDS = {
    "final_goal": {
        "authority_decision",
        "canonical_delivery_goal",
        "open_architecture_class_research",
        "verbatim_zh",
    },
    "authority_topology": {
        "active_audit",
        "authority_decisions",
        "evidence_index",
        "finding_detail",
        "live_state",
        "rule",
        "stable_contract",
        "stable_control",
    },
    "primary_role_boundary": {
        "cannot_unilaterally_change",
        "owns",
        "role",
        "state_role",
    },
    "method_boundary": {"production_bridge", "relations", "rule"},
    "optimization_contract": {
        "hard_constraints",
        "maximize",
        "minimize",
        "selection_order",
    },
    "continuous_gates": {"architecture", "complexity_measures", "security_rule"},
    "release_surface": {"compatibility_policy", "residual_risk_policy"},
    "candidate_generation_rule": {"historical_develop_baseline", "next_generation", "rule"},
}
STATE_FIELDS = {
    "active_audit",
    "blocker_census",
    "claim_status",
    "cluster_queue_after_active",
    "current_phase",
    "current_source",
    "forbidden_execution_state",
    "goal",
    "material_transition_chain",
    "next_atomic_action",
    "not_completed",
    "parallel_pr_track",
    "primary_role",
    "recovery",
    "role",
    "schema",
    "single_current_root",
    "status",
}
CONTROL_FIELDS = {
    "authority_layers",
    "concurrency_assurance",
    "evidence_contract",
    "execution_kernel",
    "forbidden_roots",
    "fresh_eyes",
    "gate_cadence",
    "hard_invariants",
    "parallelism",
    "persistence",
    "primary",
    "priority_order",
    "progress_rule",
    "role",
    "schema",
}
CONTROL_OBJECT_FIELDS = {
    "primary": {"accountability", "owns", "role", "state_boundary"},
    "authority_layers": {"evidence", "live", "on_demand_detail", "rule", "stable"},
    "execution_kernel": {
        "canary_stop",
        "commit_boundary",
        "rollback_boundary",
        "root_loop",
        "root_stop",
        "round_frontier",
        "wip_limit",
    },
    "evidence_contract": {"nonclaims", "order", "rule"},
    "concurrency_assurance": {
        "diagnostic_boundary",
        "lock_control",
        "observer_effect",
        "ordering",
        "true_shard",
    },
    "fresh_eyes": {"blind_lane", "nonconvergence", "primary_merge", "saturation"},
    "parallelism": {"dispatch_only_if", "primary_discipline"},
    "gate_cadence": {"final_identity", "integration", "per_root", "rule"},
    "persistence": {"clean_cut", "external_state", "material_boundary", "not_a_boundary"},
}
AUDIT_FIELDS = {
    "cluster_states",
    "execution_kernel_ref",
    "fresh_eyes_policy_ref",
    "known_integration_observation",
    "role",
    "round",
    "round_gates",
    "schema",
    "state_ref",
    "terminal_exit_contract_ref",
    "terminal_exit_status",
}
AUDIT_OBJECT_FIELDS = {
    "round": {"frontier", "id", "status"},
    "known_integration_observation": {"classification", "forbidden", "identity", "result"},
    "round_gates": {
        "active_root_focused",
        "blind_full_fresh_eyes",
        "fmt_check_clippy_integration_after_all_cluster_states_terminal",
        "independent_delta_confirmation",
        "make_test_checker_diff_on_final_terminal_identity",
        "targeted_rbf_after_active_root",
    },
}
AUTHORITIES = [
    "AGENTS.md",
    "tx-pool/AGENTS.md",
    "tx-pool/architecture-contract.json",
    "tx-pool/control/txpool-v8/STATE.json",
    "tx-pool/control/txpool-v8/CONTROL_KERNEL.json",
    "tx-pool/control/txpool-v8/AUDIT_PLAN.json",
    "tx-pool/control/txpool-v8/FINDINGS_LEDGER.json",
    "tx-pool/control/txpool-v8/CKB_AUTHORITY_INPUT_LEDGER.md",
    "tx-pool/control/txpool-v8/CONTEXT_LOAD_POLICY.json",
    "tx-pool/control/txpool-v8/VERIFY_STATE.py",
    "tx-pool/scripts/check_security_manifest.py",
    "tx-pool/scripts/check_all.py",
]


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


def exact_object(value, fields, label, errors):
    need(isinstance(value, dict), f"{label}:object", errors)
    result = object_value(value)
    need(set(result) == fields, f"{label}:fields", errors)
    return result


def read_json(path, errors):
    try:
        value = json.loads(path.read_text())
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        errors.append(f"cannot parse {path}: {error}")
        return {}
    need(isinstance(value, dict), f"{path}:object", errors)
    return value if isinstance(value, dict) else {}


def git(*arguments):
    environment = {
        "PATH": os.environ.get("PATH", ""),
        "GIT_NO_REPLACE_OBJECTS": "1",
        "LC_ALL": "C",
    }
    result = subprocess.run(
        ["git", *arguments],
        cwd=ROOT,
        env=environment,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        check=False,
    )
    return result.stdout.decode().strip() if result.returncode == 0 else None


def validate_contract(contract, state, control, audit, findings, errors):
    need(set(contract) == CONTRACT_FIELDS, "contract:fields", errors)
    for field, fields in CONTRACT_OBJECT_FIELDS.items():
        exact_object(contract.get(field), fields, f"contract:{field}", errors)
    need(contract.get("schema_version") == SCHEMA, "contract:schema", errors)
    need(contract.get("method_id") == METHOD, "contract:method", errors)
    need(
        contract.get("role")
        == "STABLE_G0_HARD_CONSTRAINT_PHASE_AND_COMPATIBILITY_CONTRACT_NOT_LIVE_STATE",
        "contract:role",
        errors,
    )
    need("current" not in contract, "contract:must_not_own_live_current", errors)

    goal = object_value(contract.get("final_goal"))
    need(goal.get("verbatim_zh") == GOAL, "contract:goal", errors)
    need(
        goal.get("canonical_delivery_goal")
        == "G0_CURRENT_FROZEN_CANDIDATE_SET_NEXT_GENERATION",
        "contract:canonical_goal",
        errors,
    )
    need(
        goal.get("open_architecture_class_research")
        == "G0_OPEN_ARCHITECTURE_CLASS_RESEARCH_OPEN_FROZEN",
        "contract:open_research",
        errors,
    )

    authority = object_value(contract.get("authority_topology"))
    need(
        authority.get("live_state")
        == "tx-pool/control/txpool-v8/STATE.json",
        "contract:live_state",
        errors,
    )
    need(
        authority.get("active_audit")
        == "tx-pool/control/txpool-v8/AUDIT_PLAN.json",
        "contract:active_audit",
        errors,
    )
    need(
        authority.get("rule")
        == "STABLE_CONTRACT_NEVER_OWNS_CURRENT_IDENTITY_ROOT_BLOCKERS_PROGRESS_OR_NEXT_ACTION",
        "contract:authority_rule",
        errors,
    )

    primary = object_value(contract.get("primary_role_boundary"))
    need(primary.get("role") == PRIMARY_ROLE, "contract:primary_role", errors)
    need(
        primary.get("state_role")
        == "PERSIST_EXACT_STATE_FOR_RECOVERY_NOT_REPLACE_PRIMARY_JUDGMENT",
        "contract:state_boundary",
        errors,
    )

    envelope = contract.get("authority_decision_envelope")
    need(isinstance(envelope, list), "contract:decision_envelope_type", errors)
    if isinstance(envelope, list):
        need(
            [object_value(item).get("id") for item in envelope] == DECISIONS,
            "contract:decision_envelope_ids",
            errors,
        )
        for item in envelope:
            decision = exact_object(
                item, {"id", "invariant"}, "contract:decision_envelope_item", errors
            )
            need(
                isinstance(decision.get("invariant"), str)
                and bool(decision.get("invariant")),
                f"contract:decision:{decision.get('id')}:invariant",
                errors,
            )

    method = object_value(contract.get("method_boundary"))
    need(method.get("relations") == ["Q_H", "C", "I_D", "MU"], "contract:relations", errors)
    need(method.get("production_bridge") == "RHO", "contract:bridge", errors)

    need(contract.get("phase_order") == PHASES, "contract:phase_order", errors)
    vocabulary = contract.get("claim_vocabulary")
    need(isinstance(vocabulary, list), "contract:claim_vocabulary_type", errors)
    if isinstance(vocabulary, list):
        need(
            [object_value(item).get("id") for item in vocabulary] == CLAIMS,
            "contract:claim_vocabulary_ids",
            errors,
        )
        for item in vocabulary:
            claim = object_value(item)
            need(set(claim) == {"id", "phase"}, f"contract:claim:{claim.get('id')}:fields", errors)
            need(claim.get("phase") in PHASES, f"contract:claim:{claim.get('id')}:phase", errors)

    optimization = object_value(contract.get("optimization_contract"))
    need(
        isinstance(optimization.get("hard_constraints"), list)
        and len(optimization.get("hard_constraints", [])) == 5,
        "contract:hard_constraints",
        errors,
    )
    gates = object_value(contract.get("continuous_gates"))
    need(
        "NO_SECOND_AUTHORITY_SEMANTIC_ENGINE_OR_ORDINARY_GLOBAL_SERIAL_FALLBACK"
        in gates.get("architecture", []),
        "contract:no_serial_fallback",
        errors,
    )
    release = object_value(contract.get("release_surface"))
    need(bool(release.get("compatibility_policy")), "contract:compatibility", errors)
    need(bool(release.get("residual_risk_policy")), "contract:residual_risk", errors)

    need(set(state) == STATE_FIELDS, "state:fields", errors)
    need(state.get("schema") == "txpool-v8-live-state-v2", "state:schema", errors)
    need(state.get("primary_role") == PRIMARY_ROLE, "state:primary_role", errors)
    need(
        object_value(state.get("goal")).get("canonical")
        == goal.get("canonical_delivery_goal"),
        "state:canonical_goal",
        errors,
    )
    need(
        object_value(state.get("goal")).get("open_research")
        == goal.get("open_architecture_class_research"),
        "state:open_research",
        errors,
    )
    phase = state.get("current_phase")
    need(phase in PHASES, "state:phase", errors)
    claim_status = object_value(state.get("claim_status"))
    need(set(claim_status) == set(CLAIMS), "state:claim_ids", errors)
    need(all(value == "open" for value in claim_status.values()), "state:premature_claim_closure", errors)

    source = object_value(state.get("current_source"))
    commit, tree = source.get("production_subject"), source.get("production_tree")
    need(
        isinstance(commit, str)
        and git("rev-parse", "--verify", f"{commit}^{{commit}}") == commit,
        "state:subject_commit",
        errors,
    )
    need(
        isinstance(tree, str) and git("rev-parse", f"{commit}^{{tree}}") == tree,
        "state:subject_tree",
        errors,
    )
    need(
        isinstance(state.get("material_transition_chain"), list),
        "state:material_transition_chain",
        errors,
    )

    need(set(audit) == AUDIT_FIELDS, "audit:fields", errors)
    for field, fields in AUDIT_OBJECT_FIELDS.items():
        exact_object(audit.get(field), fields, f"audit:{field}", errors)
    need(audit.get("schema") == "txpool-v8-active-terminal-audit-v4", "audit:schema", errors)
    need(audit.get("state_ref") == "STATE.json", "audit:state_ref", errors)

    need(findings.get("subject_commit") == commit, "findings:subject_commit", errors)
    need(findings.get("subject_tree") == tree, "findings:subject_tree", errors)
    clusters = findings.get("cluster_census")
    candidates = findings.get("blocking_candidates")
    need(isinstance(clusters, list) and len(set(clusters)) == len(clusters), "findings:clusters", errors)
    need(isinstance(candidates, list), "findings:candidates", errors)
    candidate_ids = [object_value(item).get("id") for item in candidates or []]
    need(
        all(isinstance(candidate_id, str) and bool(candidate_id) for candidate_id in candidate_ids)
        and len(candidate_ids) == len(set(candidate_ids)),
        "findings:candidate_ids_unique",
        errors,
    )
    census = object_value(state.get("blocker_census"))
    need(census.get("clusters") == len(clusters or []), "state:cluster_count", errors)
    need(census.get("candidates") == len(candidates or []), "state:candidate_count", errors)

    states = audit.get("cluster_states")
    need(isinstance(states, list), "audit:cluster_states_type", errors)
    if isinstance(states, list):
        for item in states:
            exact_object(
                item,
                {"id", "next_evidence", "status"},
                "audit:cluster_state",
                errors,
            )
    state_ids = [object_value(item).get("id") for item in states or []]
    need(set(state_ids) == set(clusters or []) and len(state_ids) == len(set(state_ids)), "audit:cluster_closure", errors)
    state_cut = object_value(state.get("next_atomic_action"))
    need(state_cut.get("cluster") in set(clusters or []), "active_cut:known_cluster", errors)
    cut_candidate_ids = state_cut.get("candidate_ids")
    candidate_by_id = {
        object_value(item).get("id"): object_value(item) for item in candidates or []
    }
    need(
        isinstance(cut_candidate_ids, list)
        and bool(cut_candidate_ids)
        and len(cut_candidate_ids) == len(set(cut_candidate_ids))
        and all(
            candidate_id in candidate_by_id
            and candidate_by_id[candidate_id].get("cluster") == state_cut.get("cluster")
            for candidate_id in cut_candidate_ids
        ),
        "active_cut:candidate_binding",
        errors,
    )
    need(
        all(
            isinstance(state_cut.get(field), str) and bool(state_cut.get(field))
            for field in ("id", "objective", "decision", "stop", "forbidden")
        )
        and isinstance(state_cut.get("required_output"), list)
        and bool(state_cut.get("required_output")),
        "active_cut:complete_discriminator",
        errors,
    )
    queue = state.get("cluster_queue_after_active")
    need(isinstance(queue, list), "state:cluster_queue_type", errors)
    need(
        [state_cut.get("cluster"), *(queue or [])] == state_ids,
        "state:cluster_queue_order",
        errors,
    )
    active_state = object_value((states or [{}])[0])
    need(
        active_state.get("id") == state_cut.get("cluster")
        and active_state.get("next_evidence") == state_cut.get("id")
        and str(active_state.get("status", "")).startswith("ACTIVE_STATE_NEXT_ACTION"),
        "audit:active_state_reference",
        errors,
    )

    reproduction = object_value(findings.get("primary_reproduction"))
    need(reproduction.get("status") == "PARTIAL_NOT_COMPLETE", "findings:reproduction_truth", errors)
    pending = reproduction.get("strongest_counterexplanation_pending")
    recorded = reproduction.get("strongest_counterexplanation_recorded")
    need(
        isinstance(pending, list)
        and isinstance(recorded, list)
        and len(pending) == len(set(pending))
        and len(recorded) == len(set(recorded))
        and set(pending).isdisjoint(recorded)
        and set(pending) | set(recorded) == set(candidate_ids),
        "findings:counterexplanation_closure",
        errors,
    )

    need(set(control) == CONTROL_FIELDS, "control:fields", errors)
    for field, fields in CONTROL_OBJECT_FIELDS.items():
        exact_object(control.get(field), fields, f"control:{field}", errors)
    need(control.get("schema") == "txpool-v8-primary-control-kernel-v2", "control:schema", errors)
    need(object_value(control.get("primary")).get("role") == PRIMARY_ROLE, "control:primary_role", errors)
    need(
        object_value(control.get("execution_kernel")).get("wip_limit")
        == "ONE_ACTIVE_ROOT_CLUSTER_AND_ONE_NEXT_ATOMIC_ACTION",
        "control:wip",
        errors,
    )


def method_identity(contract):
    hashes = {path: file_digest(ROOT / path) for path in AUTHORITIES}
    return {
        "method_id": METHOD,
        "contract_sha256": digest(canonical(contract)),
        "authority_hashes": hashes,
        "bundle_sha256": digest(canonical({"method_id": METHOD, "authority_hashes": hashes})),
    }


def projection(contract, state):
    source = state["current_source"]
    claims = [
        {
            "id": item["id"],
            "phase": item["phase"],
            "status": state["claim_status"][item["id"]],
        }
        for item in contract["claim_vocabulary"]
    ]
    return {
        "schema_version": 3,
        "kind": "generated_live_state_projection_not_proof",
        "method_identity": method_identity(contract),
        "checkout": {
            "commit": source["production_subject"],
            "tree": source["production_tree"],
        },
        "current": {
            "status": state["status"],
            "phase": state["current_phase"],
            "root": state["single_current_root"],
            "blocker_census": copy.deepcopy(state["blocker_census"]),
            "next_atomic_action": copy.deepcopy(state["next_atomic_action"]),
        },
        "claims": claims,
        "phase_order": copy.deepcopy(contract["phase_order"]),
        "warnings": [
            "generated projection is not live authority; read STATE.json",
            "all product claims remain open",
            "terminal correctness has a frozen eight-cluster eleven-candidate census",
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
            f"- phase/root: `{current['phase']}` / `{current['root']}`",
            f"- implementation base: `{value['checkout']['commit']}` / `{value['checkout']['tree']}`",
            f"- blockers: {current['blocker_census']['clusters']} clusters / {current['blocker_census']['candidates']} candidates",
            f"- next atomic action: `{current['next_atomic_action']['id']}`",
            f"- claims: {claims}",
            "",
        ]
    )


def todo(state, audit):
    cut = state["next_atomic_action"]
    lines = [
        "# txpool-v8 全局进度",
        "",
        "Generated mechanically from `STATE.json` and `AUDIT_PLAN.json`; disposable and not proof.",
        "",
        "## 当前原子动作",
        "",
        f"- [→] `{cut['id']}` / `{cut['cluster']}`",
        f"- 目标：{cut['objective']}",
        f"- 停止：{cut['stop']}",
        "",
        "## 当前根簇",
        "",
    ]
    for cluster_state in audit["cluster_states"]:
        marker = "→" if cluster_state["id"] == cut["cluster"] else " "
        lines.append(
            f"- [{marker}] `{cluster_state['id']}` — {cluster_state['status']}"
        )
    lines.extend(
        [
            "",
            "## PR 与阶段边界",
            "",
            f"- PR：{state['parallel_pr_track']['status']}",
            f"- 当前 phase：`{state['current_phase']}`",
            "- [ ] 当前 census 闭合后才运行完整 integration 和 fresh-eyes rounds。",
            "- [ ] terminal exit 后才进入 hard/static；performance、final security、Acceptance 不提前。",
            "",
            "## 非进度",
            "",
            "agent、报告、单个绿测试、文档、TODO 和 generated projection 都不能关闭 claim。",
            "",
        ]
    )
    return "\n".join(lines).encode()


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


def self_test(contract, state, control, audit, findings):
    failures = []
    variants = []

    changed_contract = copy.deepcopy(contract)
    changed_contract["final_goal"]["verbatim_zh"] = "mutated"
    variants.append(("goal", changed_contract, state, audit, findings))

    changed_contract = copy.deepcopy(contract)
    changed_contract["authority_decision_envelope"][0]["id"] = "CKB-AUTH-9999"
    variants.append(("decision", changed_contract, state, audit, findings))

    changed_contract = copy.deepcopy(contract)
    changed_contract["authority_topology"]["current_root"] = "DUPLICATE_LIVE_FACT"
    variants.append(("contract_live_fact_injection", changed_contract, state, audit, findings))

    changed_state = copy.deepcopy(state)
    changed_state["current_phase"] = "package_progress"
    variants.append(("phase", contract, changed_state, audit, findings))

    changed_state = copy.deepcopy(state)
    changed_state["next_atomic_action"]["id"] = "DIFFERENT"
    variants.append(("active_cut", contract, changed_state, audit, findings))

    changed_state = copy.deepcopy(state)
    changed_state["next_atomic_action"]["candidate_ids"] = [
        "READY_HEAD_PEER_REVOCATION_RESERVATION"
    ]
    variants.append(("active_cut_candidate", contract, changed_state, audit, findings))

    changed_findings = copy.deepcopy(findings)
    changed_findings["cluster_census"] = changed_findings["cluster_census"][:-1]
    variants.append(("cluster", contract, state, audit, changed_findings))

    changed_findings = copy.deepcopy(findings)
    changed_findings["blocking_candidates"].append(
        copy.deepcopy(changed_findings["blocking_candidates"][0])
    )
    changed_state = copy.deepcopy(state)
    changed_state["blocker_census"]["candidates"] += 1
    variants.append(("duplicate_candidate", contract, changed_state, audit, changed_findings))

    changed_findings = copy.deepcopy(findings)
    duplicated = changed_findings["primary_reproduction"][
        "strongest_counterexplanation_pending"
    ][0]
    changed_findings["primary_reproduction"][
        "strongest_counterexplanation_recorded"
    ].append(duplicated)
    variants.append(("counter_partition", contract, state, audit, changed_findings))

    changed_state = copy.deepcopy(state)
    changed_state["current_source"]["production_tree"] = "0" * 40
    variants.append(("subject_tree", contract, changed_state, audit, findings))

    changed_audit = copy.deepcopy(audit)
    changed_audit["round"]["root"] = "DUPLICATE_LIVE_FACT"
    variants.append(("audit_live_fact_injection", contract, state, changed_audit, findings))

    changed_control = copy.deepcopy(control)
    changed_control["execution_kernel"]["next_atomic_action"] = "DUPLICATE_LIVE_FACT"
    variants.append(
        ("control_live_fact_injection", contract, state, audit, findings, changed_control)
    )

    for variant in variants:
        label, checked_contract, checked_state, checked_audit, checked_findings = variant[:5]
        checked_control = variant[5] if len(variant) == 6 else control
        errors = []
        validate_contract(
            checked_contract,
            checked_state,
            checked_control,
            checked_audit,
            checked_findings,
            errors,
        )
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
    contract = read_json(CONTRACT, errors)
    state = read_json(STATE, errors)
    control = read_json(CONTROL, errors)
    audit = read_json(AUDIT, errors)
    findings = read_json(FINDINGS, errors)
    validate_contract(contract, state, control, audit, findings, errors)
    if arguments.self_test:
        errors.extend(self_test(contract, state, control, audit, findings))
        if not errors:
            print("structural negative canaries passed")

    value = projection(contract, state)
    manifest_data = json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True).encode() + b"\n"
    progress_data = progress(value).encode()
    todo_data = todo(state, audit)

    if not errors and arguments.write_projections:
        atomic_write(MANIFEST, manifest_data)
        atomic_write(PROGRESS, progress_data)
        atomic_write(TODO, todo_data)
    elif not errors and not arguments.self_test:
        need(
            MANIFEST.is_file() and MANIFEST.read_bytes() == manifest_data,
            "generated manifest stale; use --write-projections",
            errors,
        )
        need(
            PROGRESS.is_file() and PROGRESS.read_bytes() == progress_data,
            "generated progress stale; use --write-projections",
            errors,
        )
        need(
            TODO.is_file() and TODO.read_bytes() == todo_data,
            "generated todo stale; use --write-projections",
            errors,
        )

    if arguments.print_method_identity:
        print(
            json.dumps(
                {"method_identity": value["method_identity"], "checkout": value["checkout"]},
                ensure_ascii=False,
                indent=2,
                sort_keys=True,
            )
        )
    for error in errors:
        print(f"ERROR: {error}", file=sys.stderr)
    if errors:
        return 1
    print(
        f"validated {METHOD}: phase={state['current_phase']} "
        f"root={state['single_current_root']} next={state['next_atomic_action']['id']}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
