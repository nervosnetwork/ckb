#!/usr/bin/env python3
"""Verify the repository-owned txpool-v8 project state and frozen source cut."""

from __future__ import annotations

import hashlib
import json
import os
import copy
from pathlib import Path
import stat
import subprocess
import sys


CONTROL_ROOT = Path(__file__).resolve().parent
REPOSITORY = CONTROL_ROOT.parents[2]
MANIFEST_FILES = {
    "STATE.json",
    "CONTROL_KERNEL.json",
    "AUDIT_PLAN.json",
    "FINDINGS_LEDGER.json",
    "CKB_AUTHORITY_INPUT_LEDGER.md",
    "CONTEXT_LOAD_POLICY.json",
    "VERIFY_STATE.py",
}
ALLOWED_AFTER_SUBJECT = {
    ".gitignore",
    "AGENTS.md",
    "tx-pool/AGENTS.md",
    "tx-pool/.release-progress",
    "tx-pool/CHANGELOG.md",
    "tx-pool/PROFILING.md",
    "tx-pool/README.md",
    "tx-pool/architecture-contract.json",
    "tx-pool/security-regression-manifest.json",
    "tx-pool/scripts/check_all.py",
    "tx-pool/scripts/check_security_manifest.py",
    "tx-pool/docs/ARCHITECTURE.md",
    "tx-pool/docs/REVIEW_GUIDE.md",
}
ALLOWED_AFTER_SUBJECT_PREFIXES = (
    "tx-pool/control/txpool-v8/",
    "tx-pool/docs/",
    "tx-pool/optimization-evidence/",
)
RETIRED_CURRENT_POINTERS = (
    "tx-pool/docs/handoff/txpool-v8",
    "tx-pool/control/txpool-v8/HANDOFF.json",
    "tx-pool/control/txpool-v8/VERIFY_HANDOFF.py",
)
GENESIS_MATERIAL_STATE = {
    "production_subject": "51d282345d1d83119c46cdde8f1115f14561b4ac",
    "production_tree": "1e19719c764c7349a178d7ac0b7bf4999542966f",
    "phase": "terminal_correctness_and_root_repair",
    "root": "B8_TRUE_SHARD_GLOBAL_TERMINAL_AUDIT_AND_ROOT_REPAIR_R1",
}
PHASE_ORDER = [
    "terminal_correctness_and_root_repair",
    "hard_and_static_proof",
    "measured_performance",
    "complexity_minimum",
    "security",
    "acceptance",
]
DANGEROUS_GIT_ENVIRONMENT = {
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
    "GIT_REPLACE_REF_BASE",
}


def fail(message: str) -> None:
    raise SystemExit(f"INVALID TXPOOL-V8 PROJECT STATE: {message}")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def load(path: Path) -> dict:
    try:
        value = json.loads(path.read_text())
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        fail(f"cannot load {path}: {error}")
    if not isinstance(value, dict):
        fail(f"top level is not object: {path}")
    return value


def command(*args: str) -> str:
    environment = {
        "PATH": os.environ.get("PATH", ""),
        "GIT_NO_REPLACE_OBJECTS": "1",
        "LC_ALL": "C",
    }
    result = subprocess.run(
        args,
        cwd=REPOSITORY,
        env=environment,
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        fail(f"command failed ({' '.join(args)}): {result.stderr.strip()}")
    return result.stdout.strip()


def git_success(*args: str) -> bool:
    environment = {
        "PATH": os.environ.get("PATH", ""),
        "GIT_NO_REPLACE_OBJECTS": "1",
        "LC_ALL": "C",
    }
    return (
        subprocess.run(
            ("git", *args),
            cwd=REPOSITORY,
            env=environment,
            check=False,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        ).returncode
        == 0
    )


def require_regular_control_file(path: Path) -> None:
    try:
        mode = path.lstat().st_mode
    except OSError as error:
        fail(f"cannot stat state file {path}: {error}")
    if not stat.S_ISREG(mode) or path.is_symlink():
        fail(f"state file is not a regular non-symlink file: {path.name}")
    if path.resolve().parent != CONTROL_ROOT.resolve():
        fail(f"state file escapes canonical control directory: {path.name}")


def validate_material_transition_chain(state: dict) -> None:
    current = dict(GENESIS_MATERIAL_STATE)
    chain = state.get("material_transition_chain")
    if not isinstance(chain, list):
        fail("material transition chain missing")
    for index, transition in enumerate(chain):
        if not isinstance(transition, dict) or set(transition) != {
            "from",
            "to",
            "kind",
            "evidence",
        }:
            fail(f"material transition {index} shape mismatch")
        if transition["from"] != current:
            fail(f"material transition {index} does not continue the anchored chain")
        target = transition["to"]
        if not isinstance(target, dict) or set(target) != set(GENESIS_MATERIAL_STATE):
            fail(f"material transition {index} target shape mismatch")
        if transition["kind"] not in {"ROOT_REPAIR", "ROOT_REPLAN", "PHASE_ADVANCE"}:
            fail(f"material transition {index} kind is not allowed")
        evidence = transition["evidence"]
        if not isinstance(evidence, list) or not evidence or not all(
            isinstance(item, str) and item for item in evidence
        ):
            fail(f"material transition {index} has no evidence")
        command("git", "cat-file", "-e", f"{target['production_subject']}^{{commit}}")
        if command("git", "rev-parse", f"{target['production_subject']}^{{tree}}") != target[
            "production_tree"
        ]:
            fail(f"material transition {index} target tree mismatch")
        if not git_success(
            "merge-base",
            "--is-ancestor",
            current["production_subject"],
            target["production_subject"],
        ):
            fail(f"material transition {index} source does not descend from prior source")
        try:
            old_phase = PHASE_ORDER.index(current["phase"])
            new_phase = PHASE_ORDER.index(target["phase"])
        except ValueError:
            fail(f"material transition {index} phase is unknown")
        if transition["kind"] == "PHASE_ADVANCE":
            if new_phase != old_phase + 1:
                fail(f"material transition {index} skips or reverses phase order")
        elif new_phase != old_phase:
            fail(f"material transition {index} changes phase without PHASE_ADVANCE")
        if not isinstance(target["root"], str) or not target["root"]:
            fail(f"material transition {index} root missing")
        current = dict(target)

    source = state.get("current_source", {})
    declared = {
        "production_subject": source.get("production_subject"),
        "production_tree": source.get("production_tree"),
        "phase": state.get("current_phase"),
        "root": state.get("single_current_root"),
    }
    if current != declared:
        fail("STATE material facts do not equal the anchored transition-chain tip")


def run_negative_self_test(state: dict) -> None:
    variants = []
    changed = copy.deepcopy(state)
    changed["current_source"]["production_subject"] = "0" * 40
    variants.append(("unanchored_subject", changed))
    changed = copy.deepcopy(state)
    changed["current_phase"] = "acceptance"
    variants.append(("unanchored_phase", changed))
    changed = copy.deepcopy(state)
    changed["single_current_root"] = "UNANCHORED_ROOT"
    variants.append(("unanchored_root", changed))
    changed = copy.deepcopy(state)
    changed["material_transition_chain"] = [{}]
    variants.append(("malformed_transition", changed))
    for label, variant in variants:
        try:
            validate_material_transition_chain(variant)
        except SystemExit:
            continue
        fail(f"negative self-test escaped: {label}")
    if allowed_after_subject("tx-pool/src/authority/runtime.rs"):
        fail("negative self-test escaped: production path allowlist")
    if not allowed_after_subject("tx-pool/control/txpool-v8/STATE.json"):
        fail("negative self-test failed: canonical control path rejected")
    if not allowed_after_subject("AGENTS.md"):
        fail("negative self-test failed: repository instruction path rejected")


def allowed_after_subject(path: str) -> bool:
    return path in ALLOWED_AFTER_SUBJECT or path.startswith(ALLOWED_AFTER_SUBJECT_PREFIXES)


def main() -> int:
    self_test = sys.argv[1:] == ["--self-test"]
    if sys.argv[1:] and not self_test:
        fail("unsupported arguments")
    if CONTROL_ROOT.name != "txpool-v8" or CONTROL_ROOT.parent.name != "control":
        fail("noncanonical control directory")
    dangerous = sorted(
        key
        for key in os.environ
        if key in DANGEROUS_GIT_ENVIRONMENT or key.startswith("GIT_CONFIG_")
    )
    if dangerous:
        fail("dangerous Git environment overrides are set: " + ", ".join(dangerous))
    for retired in RETIRED_CURRENT_POINTERS:
        if (REPOSITORY / retired).exists():
            fail(f"retired current pointer returned: {retired}")

    require_regular_control_file(CONTROL_ROOT / "MANIFEST.json")
    manifest = load(CONTROL_ROOT / "MANIFEST.json")
    if manifest.get("schema") != "txpool-v8-project-state-manifest-v2":
        fail("manifest schema mismatch")
    files = manifest.get("files")
    if not isinstance(files, dict) or set(files) != MANIFEST_FILES:
        fail("manifest file set mismatch")
    for name, expected in files.items():
        path = CONTROL_ROOT / name
        require_regular_control_file(path)
        if sha256(path) != expected:
            fail(f"state file hash mismatch: {name}")

    state = load(CONTROL_ROOT / "STATE.json")
    control = load(CONTROL_ROOT / "CONTROL_KERNEL.json")
    audit = load(CONTROL_ROOT / "AUDIT_PLAN.json")
    findings = load(CONTROL_ROOT / "FINDINGS_LEDGER.json")
    context = load(CONTROL_ROOT / "CONTEXT_LOAD_POLICY.json")

    if state.get("schema") != "txpool-v8-live-state-v2":
        fail("state schema mismatch")
    if control.get("schema") != "txpool-v8-primary-control-kernel-v2":
        fail("control schema mismatch")
    if audit.get("schema") != "txpool-v8-active-terminal-audit-v4":
        fail("audit schema mismatch")
    if state.get("primary_role") != "G0_ACCOUNTABLE_PRIMARY_ENGINEERING_OWNER":
        fail("Primary role mismatch")
    if control.get("primary", {}).get("role") != state.get("primary_role"):
        fail("Primary role is not single-source consistent")
    validate_material_transition_chain(state)
    if self_test:
        run_negative_self_test(state)

    source = state.get("current_source", {})
    subject = source.get("production_subject")
    subject_tree = source.get("production_tree")
    if not isinstance(subject, str) or not isinstance(subject_tree, str):
        fail("production subject identity missing")
    command("git", "cat-file", "-e", f"{subject}^{{commit}}")
    if command("git", "rev-parse", f"{subject}^{{tree}}") != subject_tree:
        fail("production subject tree mismatch")
    if not git_success("merge-base", "--is-ancestor", subject, "HEAD"):
        fail("current HEAD does not contain the frozen production subject")
    branch = command("git", "branch", "--show-current")
    if branch != source.get("branch"):
        fail(f"branch mismatch: state={source.get('branch')} checkout={branch}")

    if command("git", "for-each-ref", "--format=%(refname)", "refs/replace"):
        fail("Git replace refs are forbidden for state verification")
    exceptional_index = [
        line
        for line in command("git", "ls-files", "-v").splitlines()
        if line and line[0] != "H"
    ]
    if exceptional_index:
        fail("assume-unchanged, skip-worktree or exceptional index entries are forbidden")
    if not git_success("diff", "--quiet", "HEAD", "--"):
        fail("tracked worktree differs from HEAD")
    if not git_success("diff", "--cached", "--quiet"):
        fail("index differs from HEAD")
    if command("git", "ls-files", "--others", "--exclude-standard"):
        fail("untracked files are present")
    if command("git", "status", "--porcelain", "--untracked-files=normal"):
        fail("repository is not clean")

    changed_after_subject = command("git", "diff", "--name-only", f"{subject}..HEAD")
    unexpected = [
        path
        for path in changed_after_subject.splitlines()
        if path and not allowed_after_subject(path)
    ]
    if unexpected:
        fail(
            "production source changed after the frozen subject outside declared control/doc paths: "
            + ", ".join(unexpected)
        )

    if findings.get("subject_commit") != subject or findings.get("subject_tree") != subject_tree:
        fail("findings subject mismatch")
    state_cut = state.get("next_atomic_action", {})

    clusters = findings.get("cluster_census")
    candidates = findings.get("blocking_candidates")
    states = audit.get("cluster_states")
    if not isinstance(clusters, list) or not isinstance(candidates, list) or not isinstance(states, list):
        fail("blocker census shape mismatch")
    state_ids = [item.get("id") for item in states if isinstance(item, dict)]
    if len(state_ids) != len(states) or set(state_ids) != set(clusters):
        fail("audit cluster closure mismatch")
    census = state.get("blocker_census", {})
    if census.get("clusters") != len(clusters) or census.get("candidates") != len(candidates):
        fail("live blocker census mismatch")
    queue = state.get("cluster_queue_after_active")
    if [state_cut.get("cluster"), *(queue if isinstance(queue, list) else [])] != state_ids:
        fail("cluster queue is not a total single-WIP order")
    active_state = states[0] if states and isinstance(states[0], dict) else {}
    if (
        active_state.get("id") != state_cut.get("cluster")
        or active_state.get("next_evidence") != state_cut.get("id")
        or not str(active_state.get("status", "")).startswith("ACTIVE_STATE_NEXT_ACTION")
    ):
        fail("audit does not reference the sole STATE next atomic action")

    ledger = (CONTROL_ROOT / "CKB_AUTHORITY_INPUT_LEDGER.md").read_text()
    section_nine = ledger.split("## 九、当前主仓吸收状态", 1)
    if len(section_nine) != 2:
        fail("authority ledger Section 9 missing")
    for index in range(1, 10):
        if f"CKB-AUTH-{index:04d}" not in section_nine[1]:
            fail(f"authority decision missing from Section 9: CKB-AUTH-{index:04d}")
    if "对 G0 负责的 Primary 工程负责人" not in section_nine[1]:
        fail("Primary stewardship decision missing from Section 9")

    first_read = context.get("primary_first_read_after_verification")
    if first_read != [
        "STATE.json",
        "CONTROL_KERNEL.json",
        "CKB_AUTHORITY_INPUT_LEDGER.md_SECTION_9_ONLY",
    ]:
        fail("cold-load policy is not the minimal Primary state set")

    checker = subprocess.run(
        (sys.executable, "-B", "tx-pool/scripts/check_security_manifest.py"),
        cwd=REPOSITORY,
        env={
            "PATH": os.environ.get("PATH", ""),
            "GIT_NO_REPLACE_OBJECTS": "1",
            "LC_ALL": "C",
        },
        check=False,
        capture_output=True,
        text=True,
    )
    if checker.returncode != 0:
        fail(f"project control checker failed: {checker.stderr.strip() or checker.stdout.strip()}")

    print(
        json.dumps(
            {
                "valid": True,
                "head": command("git", "rev-parse", "HEAD^{commit}"),
                "production_subject": subject,
                "production_tree": subject_tree,
                "phase": state.get("current_phase"),
                "root": state.get("single_current_root"),
                "next_atomic_action": state_cut.get("id"),
                "clusters": len(clusters),
                "candidates": len(candidates),
                "manifest_sha256": sha256(CONTROL_ROOT / "MANIFEST.json"),
                "negative_self_test": "PASS" if self_test else "NOT_REQUESTED",
            },
            ensure_ascii=False,
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
