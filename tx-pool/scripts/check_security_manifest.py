#!/usr/bin/env python3
"""Validate and project the tx-pool minimal-kernel control structure."""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
PLAN = ROOT / "tx-pool/.independent-execution-plan"
CONTRACT = ROOT / "tx-pool/architecture-contract.json"
MANIFEST = ROOT / "tx-pool/security-regression-manifest.json"
PROGRESS = ROOT / "tx-pool/.release-progress"
METHOD = "txpool-minimal-kernel-v2"
GOAL = (
    "从 CKB 原始协议与交易依赖/冲突语义出发，在一致性、安全、兼容与资源有界为硬约束下，"
    "求出可证明全局静态最优、实测性能最强且实现/证明复杂度最小的 tx-pool 架构；"
    "独立工作最大并行，耦合事实只在唯一权威的最小原子切口排序。"
)
RECOVERY = {
    "ref": "refs/codex/checkpoints/txpool-proof-kernel-reset-recovery-20260814",
    "commit": "bc3a2c65e604212a2493fc0e12db27313179816c",
    "tree": "f54256aa52c071ffdaa9dfe1e4af7dc5424de592",
}
OLD_U = "d88f2fd7e3dba27513b515fc16db72db42f30f7118644ee756242bef0b79355f"
REPORTS = {
    "/private/tmp/txpool-x3-proof-burden-retirement-audit-56de.md":
        "43fb8dc497d0ca4c440b6e5fbdee43cf3c551db7f7d1f22e5295ba2c4ab57800",
    "/private/tmp/txpool-acceptance-control-plane-attack-56de7ef5.md":
        "8bae396de1c0328b9aa65392218d59aa46e0d7ec85ac6f077d96a9e4710694ac",
}
PHASES = [
    "method_same_subject_review", "partner_a_method_review", "legacy_generation_retirement",
    "hard_and_static_proof", "measured_performance", "complexity_minimum",
    "acceptance_universe_freeze", "acceptance_cold_join",
]
REVIEWS = {
    "internal": "tx-pool/optimization-evidence/reviews/method-self-review-v2.json",
    "partner_a": "tx-pool/optimization-evidence/reviews/partner-a-minimal-kernel-v2.json",
}
AUTHORITIES = [
    "AGENTS.md", "tx-pool/AGENTS.md", "tx-pool/.independent-execution-plan",
    "tx-pool/architecture-contract.json", "tx-pool/scripts/check_security_manifest.py",
    "tx-pool/scripts/check_all.py",
]
CONTROL_RE = re.compile(
    r"<!-- txpool-plan-control-v5:start -->\s*```json\s*(\{.*?\})\s*```\s*"
    r"<!-- txpool-plan-control-v5:end -->", re.DOTALL,
)


def need(condition, message, errors):
    if not condition:
        errors.append(message)


def obj(value):
    return value if isinstance(value, dict) else {}


def canonical(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode()


def digest(data):
    return hashlib.sha256(data).hexdigest()


def file_digest(path):
    return digest(path.read_bytes())


def git(*args):
    result = subprocess.run(
        ["git", *args], cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL
    )
    return result.stdout if result.returncode == 0 else None


def read_json(path, errors):
    try:
        value = json.loads(path.read_text())
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        errors.append(f"cannot parse {path}: {exc}")
        return {}
    need(isinstance(value, dict), f"{path} must contain a JSON object", errors)
    return value if isinstance(value, dict) else {}


def parse_control(text, errors):
    blocks = CONTROL_RE.findall(text)
    need(len(blocks) == 1, "control:block", errors)
    if len(blocks) != 1:
        return {}
    try:
        value = json.loads(blocks[0])
    except json.JSONDecodeError as exc:
        errors.append(f"control:json:{exc}")
        return {}
    need(isinstance(value, dict), "control:type", errors)
    return value if isinstance(value, dict) else {}


def validate_control(control, errors):
    controller = obj(control.get("controller"))
    checks = [
        (control.get("schema_version") == 5, "control:schema"),
        (control.get("method_id") == METHOD, "control:method_id"),
        (control.get("states") == ["construction", "acceptance", "accepted"],
         "control:states"),
        (controller.get("path") == "tx-pool/scripts/run_txpool_acceptance.py",
         "control:controller"),
        (control.get("state_entry") == {
            "acceptance": "acceptance_universe_freeze", "accepted": "acceptance_cold_join"},
         "control:state_entry"),
    ]
    for condition, message in checks:
        need(condition, message, errors)
    phases = control.get("phases")
    need(isinstance(phases, list), "control:phases", errors)
    if not isinstance(phases, list):
        return
    ids = [obj(item).get("id") for item in phases]
    need(ids == PHASES, "control:phase_order", errors)
    seen: set[str] = set()
    for item in phases:
        phase = obj(item)
        phase_id, requires = phase.get("id"), phase.get("requires")
        need(isinstance(requires, list) and len(requires) == len(set(requires or [])),
             f"phase:{phase_id}:requires", errors)
        if isinstance(requires, list):
            need(all(isinstance(x, str) and x in seen for x in requires),
                 f"phase:{phase_id}:topology", errors)
        need(bool(phase.get("gate")) and bool(phase.get("evidence")),
             f"phase:{phase_id}:gate", errors)
        if isinstance(phase_id, str):
            seen.add(phase_id)
    groups = control.get("same_subject_groups")
    need(groups == [[PHASES[0], PHASES[1]], [PHASES[-2], PHASES[-1]]],
         "control:same_subject", errors)
    retirement = obj(control.get("legacy_retirement"))
    paths = retirement.get("forbidden_paths")
    valid_paths = isinstance(paths, list) and bool(paths) and all(
        isinstance(path, str) and path.startswith("tx-pool/") and ".." not in Path(path).parts
        for path in paths or []
    )
    need(valid_paths and len(paths) == len(set(paths)) if valid_paths else False,
         "control:legacy_paths", errors)
    pattern = retirement.get("documentation_pattern")
    need(isinstance(pattern, str) and bool(pattern), "control:legacy_doc_pattern", errors)
    try:
        re.compile(pattern or "(?!)")
    except re.error:
        errors.append("control:legacy_doc_pattern")
    need(bool(retirement.get("exit")), "control:legacy_exit", errors)


def validate_contract(contract, control, plan, errors):
    goal, authority = obj(contract.get("final_goal")), obj(contract.get("authority"))
    kernel, binding = obj(contract.get("minimal_kernel")), obj(contract.get("control_binding"))
    history, reviews = obj(contract.get("historical_evidence")), obj(contract.get("reviews"))
    recovery, safety = obj(history.get("recovery_checkpoint")), obj(contract.get("safety_guards"))
    authority_paths = {
        "semantic_method": "tx-pool/.independent-execution-plan",
        "contract": "tx-pool/architecture-contract.json",
        "projector": "tx-pool/scripts/check_security_manifest.py",
        "entrypoint": "tx-pool/scripts/check_all.py",
        "generated_manifest": "tx-pool/security-regression-manifest.json",
        "generated_progress": "tx-pool/.release-progress",
    }
    expected_reports = [{"path": path, "sha256": sha} for path, sha in REPORTS.items()]
    checks = [
        (set(contract) == {"schema_version", "method_id", "final_goal", "authority",
                           "minimal_kernel", "control_binding", "reviews", "claims",
                           "historical_evidence", "safety_guards"}, "contract:fields"),
        (contract.get("schema_version") == 34, "contract:schema"),
        (contract.get("method_id") == METHOD == control.get("method_id"), "contract:method_id"),
        (GOAL in plan and goal == {"verbatim_zh": GOAL}, "contract:final goal"),
        (set(authority) == {*authority_paths, "review_artifacts_are_authority",
                            "generated_projections_are_authority"} and
         all(authority.get(k) == v for k, v in authority_paths.items()), "contract:authority_paths"),
        (authority.get("review_artifacts_are_authority") is False and
         authority.get("generated_projections_are_authority") is False,
         "contract:authority_direction"),
        (set(kernel) == {"executable_reference_tx_pool", "relations", "production_bridge",
                         "architecture_normal_form"} and
         kernel.get("executable_reference_tx_pool") is False, "contract:executable_model"),
        (kernel.get("relations") == ["Q_H", "C", "I_D", "MU"], "contract:kernel"),
        (kernel.get("production_bridge") == "RHO", "contract:RHO"),
        (kernel.get("architecture_normal_form") ==
         ["Own", "Ord", "Cut", "Commit", "Work", "Life"], "contract:normal_form"),
        (binding == {"schema": "txpool-plan-control-v5", "controller_available": False,
                     "state_and_claim_status": "derived_not_stored"}, "contract:control"),
        (reviews == REVIEWS, "contract:reviews"),
        (set(history) == {"recovery_checkpoint", "retired_acceptance_universe",
                          "neutral_reports", "old_model_or_certificate_can_be_new_authority"},
         "contract:history_fields"),
        (set(recovery) == {*RECOVERY, "role"} and
         all(recovery.get(k) == v for k, v in RECOVERY.items()), "contract:recovery"),
        (history.get("retired_acceptance_universe") == OLD_U, "contract:retired_U"),
        (history.get("old_model_or_certificate_can_be_new_authority") is False,
         "contract:old_authority"),
        (history.get("neutral_reports") == expected_reports, "contract:reports"),
        (safety == {"production_rollback_forbidden": True,
                    "live_develop_reconciliation_forbidden": True}, "contract:safety"),
    ]
    for condition, message in checks:
        need(condition, message, errors)
    claims = contract.get("claims")
    need(isinstance(claims, list) and bool(claims), "claims:empty", errors)
    seen: set[str] = set()
    for claim in claims if isinstance(claims, list) else []:
        item = obj(claim)
        claim_id = item.get("id")
        need(isinstance(claim_id, str) and claim_id not in seen, "claims:identity", errors)
        need(item.get("phase") in PHASES, f"claim:{claim_id}:phase", errors)
        need(set(item) == {"id", "phase"}, f"claim:{claim_id}:fields", errors)
        if isinstance(claim_id, str):
            seen.add(claim_id)


def verify_recovery(errors):
    commit = git("rev-parse", "--verify", f"{RECOVERY['ref']}^{{commit}}")
    tree = git("rev-parse", f"{RECOVERY['commit']}^{{tree}}")
    need(commit is not None and commit.decode().strip() == RECOVERY["commit"],
         "recovery:ref_commit", errors)
    need(tree is not None and tree.decode().strip() == RECOVERY["tree"],
         "recovery:commit_tree", errors)
    for name, expected in REPORTS.items():
        path = Path(name)
        need(path.is_file() and file_digest(path) == expected,
             f"recovery:report:{name}", errors)


def method_identity(control, errors):
    paths = list(AUTHORITIES)
    workflow_dir = ROOT / ".github/workflows"
    for path in sorted(workflow_dir.glob("*.y*ml")):
        if "tx-pool/scripts/check_all.py" in path.read_text():
            paths.append(path.relative_to(ROOT).as_posix())
    need(any(x.startswith(".github/workflows/") for x in paths),
         "authority:no_CI_entrypoint", errors)
    entry = ROOT / "tx-pool/scripts/check_all.py"
    need(entry.is_file() and "check_security_manifest.py" in entry.read_text(),
         "authority:entrypoint", errors)
    hashes = {}
    for name in sorted(set(paths)):
        path = ROOT / name
        need(path.is_file(), f"authority:missing:{name}", errors)
        if path.is_file():
            hashes[name] = file_digest(path)
    control_sha = digest(canonical(control))
    bundle = {
        "schema_version": 1, "method_id": METHOD, "control_sha256": control_sha,
        "recovery": {"commit": RECOVERY["commit"], "tree": RECOVERY["tree"]},
        "authority_hashes": hashes,
    }
    return {
        "method_id": METHOD, "final_goal_sha256": digest(GOAL.encode()),
        "control_sha256": control_sha, "bundle_sha256": digest(canonical(bundle)),
        "authority_hashes": hashes, "recovery_commit": RECOVERY["commit"],
        "recovery_tree": RECOVERY["tree"],
    }


def checkout_diagnostics(identity):
    head, tree = git("rev-parse", "HEAD^{commit}"), git("rev-parse", "HEAD^{tree}")
    matches = all(
        (content := git("show", f"HEAD:{name}")) is not None and digest(content) == sha
        for name, sha in identity["authority_hashes"].items()
    )
    return {
        "head": head.decode().strip() if head else None,
        "head_tree": tree.decode().strip() if tree else None,
        "authority_matches_head": matches,
    }


def strings(value):
    return isinstance(value, list) and bool(value) and all(
        isinstance(item, str) and item.strip() for item in value
    )


def validate_review(review, kind, identity, errors, verify_files=True):
    start = len(errors)
    fields = {
        "schema_version", "kind", "method_id", "lifecycle", "verdict",
        "unresolved_findings", "coverage", "strongest_counterexamples", "subject", "report",
    }
    lifecycle = ["START", "DONE"] if kind == "internal" else ["WAITING", "START", "DONE"]
    coverage, subject, report = obj(review.get("coverage")), obj(review.get("subject")), obj(review.get("report"))
    unresolved = review.get("unresolved_findings")
    checks = [
        (set(review) == fields, f"review:{kind}:fields"),
        (review.get("schema_version") == 1 and review.get("kind") == kind and
         review.get("method_id") == METHOD, f"review:{kind}:identity"),
        (review.get("lifecycle") == lifecycle, f"review:{kind}:lifecycle"),
        (review.get("verdict") in {"pass", "fail"}, f"review:{kind}:verdict"),
        (isinstance(unresolved, int) and not isinstance(unresolved, bool) and unresolved >= 0,
         f"review:{kind}:findings"),
        (review.get("verdict") != "pass" or unresolved == 0, f"review:{kind}:pass_findings"),
        (set(coverage) == {"local_magnification", "global_propagation"} and
         all(strings(coverage.get(key)) for key in coverage), f"review:{kind}:coverage"),
        (strings(review.get("strongest_counterexamples")), f"review:{kind}:counterexamples"),
        (set(subject) == {"commit", "tree", "bundle_sha256", "authority_hashes"},
         f"review:{kind}:subject"),
        (subject.get("bundle_sha256") == identity["bundle_sha256"], f"review:{kind}:bundle"),
        (subject.get("authority_hashes") == identity["authority_hashes"], f"review:{kind}:authorities"),
        (set(report) == {"path", "sha256"} and
         bool(re.fullmatch(r"[0-9a-f]{64}", str(report.get("sha256", "")))),
         f"review:{kind}:report"),
    ]
    for condition, message in checks:
        need(condition, message, errors)
    commit, tree = str(subject.get("commit", "")), str(subject.get("tree", ""))
    need(bool(re.fullmatch(r"[0-9a-f]{40}", commit)) and
         bool(re.fullmatch(r"[0-9a-f]{40}", tree)), f"review:{kind}:commit_tree", errors)
    if verify_files:
        resolved, resolved_tree = git("rev-parse", "--verify", f"{commit}^{{commit}}"), git("rev-parse", f"{commit}^{{tree}}")
        need(resolved is not None and resolved.decode().strip() == commit and
             resolved_tree is not None and resolved_tree.decode().strip() == tree,
             f"review:{kind}:frozen_subject", errors)
        for name, sha in identity["authority_hashes"].items():
            content = git("show", f"{commit}:{name}")
            need(content is not None and digest(content) == sha,
                 f"review:{kind}:authority:{name}", errors)
        raw = Path(str(report.get("path", "")))
        report_path = raw if raw.is_absolute() else ROOT / raw
        need(report_path.is_file() and file_digest(report_path) == report.get("sha256"),
             f"review:{kind}:report_hash", errors)
    valid = len(errors) == start
    return valid, valid and review.get("verdict") == "pass" and unresolved == 0, subject


def reviews(identity, errors, warnings):
    result, subjects = {}, {}
    for kind, name in REVIEWS.items():
        path = ROOT / name
        if not path.is_file():
            result[kind] = {"present": False, "valid": False, "passed": False}
            warnings.append(f"{kind} method review is missing")
            continue
        review = read_json(path, errors)
        valid, passed, subject = validate_review(review, kind, identity, errors)
        result[kind] = {"present": True, "valid": valid, "passed": passed,
                        "commit": subject.get("commit"), "tree": subject.get("tree")}
        subjects[kind] = subject
        if valid and not passed:
            warnings.append(f"{kind} method review reports fail")
    need(len(subjects) < 2 or subjects["internal"] == subjects["partner_a"],
         "reviews:same_subject", errors)
    return result


def legacy_routes(control):
    routes = []
    retirement = obj(control.get("legacy_retirement"))
    for name in retirement.get("forbidden_paths", []):
        if (ROOT / name).exists():
            routes.append(f"legacy_artifact:{name}")
    try:
        doc_pattern = re.compile(retirement.get("documentation_pattern", "(?!)"))
    except re.error:
        doc_pattern = re.compile("(?!)")
    documents = [ROOT / "tx-pool/README.md", *(ROOT / "tx-pool/docs").rglob("*.md")]
    for path in sorted(documents):
        if path.is_file() and doc_pattern.search(path.read_text()):
            routes.append(f"active_legacy_proof_document:{path.relative_to(ROOT)}")
    scanned = [ROOT / "tx-pool/scripts/check_all.py", *(ROOT / ".github/workflows").glob("*.y*ml")]
    for path in scanned:
        if path.is_file():
            for checker in re.findall(r"check_[A-Za-z0-9_]+\.py", path.read_text()):
                if checker not in {"check_all.py", "check_security_manifest.py"}:
                    routes.append(f"direct_legacy_checker_reference:{path.relative_to(ROOT)}:{checker}")
    return sorted(set(routes))


def phase_projection(control, review, legacy):
    gates = {
        "method_same_subject_review": review["internal"]["passed"],
        "partner_a_method_review": review["partner_a"]["passed"],
        "legacy_generation_retirement": not legacy,
    }
    done: set[str] = set()
    entries = []
    for phase in control.get("phases", []):
        ready = all(item in done for item in phase["requires"])
        complete = ready and gates.get(phase["id"], False)
        if complete:
            done.add(phase["id"])
        entries.append({"id": phase["id"], "status":
                        "complete" if complete else "ready_incomplete" if ready else "blocked"})
    active = next((x["id"] for x in entries if x["status"] == "ready_incomplete"), None)
    state = "accepted" if PHASES[-1] in done else "acceptance" if PHASES[-2] in done else "construction"
    return {"state": state, "completed": [x for x in PHASES if x in done],
            "active": active, "entries": entries, "controller_available": False}


def progress(value):
    identity, phases, lines = value["method_identity"], value["phase_projection"], []
    for kind, item in value["reviews"].items():
        lines.append(f"- review {kind}: " + ("PASS" if item["passed"] else
                     "FAIL" if item["present"] else "MISSING"))
    lines.append(f"- legacy artifacts/routes: `{len(value['legacy_routes'])}` (manifest owns detail)")
    return "\n".join([
        "# Tx-Pool Minimal-Kernel Progress Projection", "",
        "Generated mechanically; disposable and not proof.", "",
        f"- method/bundle: `{METHOD}` / `{identity['bundle_sha256']}`",
        f"- recovery: `{RECOVERY['commit']}` / `{RECOVERY['tree']}`",
        f"- state/completed/active: `{phases['state']}` / "
        f"`{','.join(phases['completed']) or 'none'}` / `{phases['active']}`",
        "- Acceptance: not established; controller absent; machine phases fail closed",
        *(lines or ["- legacy/reviews: none"]), "",
        f"Next: continue only `{phases['active']}`; freeze one commit/tree before review.",
        f"Never restore retired Acceptance `{OLD_U}` or roll back production成果.", "",
    ])


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


def canaries(plan, contract, control, identity):
    failures, caught = [], []
    changed = copy.deepcopy(contract)
    changed["final_goal"]["verbatim_zh"] = "mutated"
    validate_contract(changed, control, plan, caught)
    if not any("final goal" in item for item in caught):
        failures.append("goal canary escaped")
    changed, caught = copy.deepcopy(control), []
    changed["method_id"] = "mutated"
    validate_control(changed, caught)
    if not any("method_id" in item for item in caught):
        failures.append("method-id canary escaped")
    forged = {
        "schema_version": 1, "kind": "internal", "method_id": METHOD,
        "lifecycle": ["START", "DONE"], "verdict": "pass", "unresolved_findings": 0,
        "coverage": {"local_magnification": ["slice"]},
        "strongest_counterexamples": ["counterexample"],
        "subject": {"commit": RECOVERY["commit"], "tree": RECOVERY["tree"],
                    "bundle_sha256": "0" * 64, "authority_hashes": identity["authority_hashes"]},
        "report": {"path": next(iter(REPORTS)), "sha256": next(iter(REPORTS.values()))},
    }
    caught = []
    validate_review(forged, "internal", identity, caught, verify_files=False)
    if not any("coverage" in item for item in caught) or not any("bundle" in item for item in caught):
        failures.append("forged-review canary escaped")
    return failures


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write-projections", action="store_true")
    parser.add_argument("--print-method-identity", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    errors, warnings = [], []
    try:
        plan = PLAN.read_text()
    except (OSError, UnicodeError) as exc:
        print(f"ERROR: cannot read plan: {exc}", file=sys.stderr)
        return 1
    contract = read_json(CONTRACT, errors)
    control = parse_control(plan, errors)
    validate_control(control, errors)
    validate_contract(contract, control, plan, errors)
    verify_recovery(errors)
    identity = method_identity(control, errors)
    controller = ROOT / "tx-pool/scripts/run_txpool_acceptance.py"
    need(not controller.exists(), "unexpected controller refreezes this method generation", errors)
    if not controller.exists():
        warnings.append("controller absent; machine phases fail closed")
    review = reviews(identity, errors, warnings)
    legacy = legacy_routes(control)
    if legacy:
        warnings.append(f"{len(legacy)} active legacy proof routes remain")
    phases = phase_projection(control, review, legacy)
    accepted = PHASES[-1] in phases["completed"]
    claims = [{**claim, "status": "proved" if accepted else "open"}
              for claim in contract.get("claims", [])]
    value = {
        "schema_version": 1, "kind": "disposable_structural_projection_not_proof",
        "method_identity": identity, "reviews": review, "legacy_routes": legacy,
        "phase_projection": phases, "claims": claims,
        "retired_acceptance_universe": OLD_U, "warnings": sorted(set(warnings)),
    }
    manifest_data = json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True).encode() + b"\n"
    progress_data = progress(value).encode()
    if args.self_test:
        failures = canaries(plan, contract, control, identity)
        errors.extend(failures)
        if not failures:
            print("structural negative canaries passed")
    if args.print_method_identity:
        print(json.dumps({"method_identity": identity,
                          "checkout_diagnostics": checkout_diagnostics(identity)},
                         ensure_ascii=False, indent=2, sort_keys=True))
    if not errors and args.write_projections:
        atomic_write(MANIFEST, manifest_data)
        atomic_write(PROGRESS, progress_data)
    elif not errors:
        need(MANIFEST.is_file() and MANIFEST.read_bytes() == manifest_data,
             "generated manifest stale; use --write-projections", errors)
        need(PROGRESS.is_file() and PROGRESS.read_bytes() == progress_data,
             "generated progress stale; use --write-projections", errors)
    for warning in sorted(set(warnings)):
        print(f"INCOMPLETE: {warning}")
    for error in errors:
        print(f"ERROR: {error}", file=sys.stderr)
    if errors:
        return 1
    print(f"validated {METHOD}: state={phases['state']} active={phases['active']} legacy={len(legacy)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
