#!/usr/bin/env python3
"""Verify the single cold-recovery cut for the txpool-v8 G0 project."""

from __future__ import annotations

import copy
import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parents[3]
CONTROL = ROOT / "tx-pool/control/txpool-v8"
OWNER_PATHS = (
    ROOT / "tx-pool/architecture-contract.json",
    CONTROL / "CONTROL_KERNEL.json",
    CONTROL / "STATE.json",
    CONTROL / "EVIDENCE.json",
    CONTROL / "FINDINGS_LEDGER.json",
    CONTROL / "CKB_AUTHORITY_INPUT_LEDGER.md",
    CONTROL / "VERIFY_STATE.py",
)
MANIFEST = CONTROL / "MANIFEST.json"
LIVE_EXCLUSIONS = {
    "tx-pool/control/txpool-v8/STATE.json",
    "tx-pool/control/txpool-v8/MANIFEST.json",
}
CLAIMS = {
    "HARD_FEASIBILITY",
    "STATIC_GLOBAL_BOTTOM",
    "MEASURED_STRONGEST",
    "SEMANTIC_ZERO_AND_ENGINEERING_MINIMUM",
    "FINAL_ADVERSARIAL_SECURITY",
    "NEW_COLD_JOINED_ACCEPTED_UNIVERSE",
}


def fail(message: str) -> None:
    raise SystemExit(f"txpool-v8 state invalid: {message}")


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def load(path: Path) -> dict:
    try:
        if stat.S_ISLNK(path.lstat().st_mode) or not path.is_file():
            fail(f"owner is not a regular file: {path.relative_to(ROOT)}")
        value = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot load {path.relative_to(ROOT)}: {error}")
    if not isinstance(value, dict):
        fail(f"owner is not an object: {path.relative_to(ROOT)}")
    return value


def git(*arguments: str) -> bytes:
    environment = {
        "PATH": os.environ.get("PATH", ""),
        "LC_ALL": "C",
        "GIT_NO_REPLACE_OBJECTS": "1",
    }
    completed = subprocess.run(
        ("git", *arguments), cwd=ROOT, env=environment, capture_output=True, check=False
    )
    if completed.returncode:
        fail(
            f"git {' '.join(arguments)} failed: "
            + completed.stderr.decode(errors="replace").strip()
        )
    return completed.stdout


def git_text(*arguments: str) -> str:
    return git(*arguments).decode().strip()


def resolve(reference: str, evidence: dict):
    prefix = "EVIDENCE.json#/"
    if not reference.startswith(prefix):
        fail(f"noncanonical evidence reference: {reference}")
    value = evidence
    for component in reference[len(prefix) :].split("/"):
        if not isinstance(value, dict) or component not in value:
            fail(f"missing evidence reference: {reference}")
        value = value[component]
    return value


def within(path: str, scopes: list[str]) -> bool:
    return any(path == scope or path.startswith(scope.rstrip("/") + "/") for scope in scopes)


def nested_keys(value) -> set[str]:
    if isinstance(value, dict):
        return set(value).union(*(nested_keys(item) for item in value.values()))
    if isinstance(value, list):
        return set().union(*(nested_keys(item) for item in value))
    return set()


def validate_contract(contract: dict, control: dict, ledger: str) -> None:
    decisions = contract.get("authority_decision_envelope")
    expected = [f"CKB-AUTH-{index:04d}" for index in range(1, 10)]
    if not isinstance(decisions, list) or [item.get("id") for item in decisions] != expected:
        fail("architecture contract must contain the nine ordered authority decisions")
    by_id = {item["id"]: item.get("invariant", "") for item in decisions}
    if "READ_ONLY_CELL_DEP" not in by_id["CKB-AUTH-0008"]:
        fail("shared read-only cell-dep independence disappeared")
    if "EFFECTS_BECOME_DURABLE" not in by_id["CKB-AUTH-0009"]:
        fail("atomic effect durability obligation disappeared")
    hard = " ".join(control.get("hard_invariants", []))
    if "READ_ONLY_CELL_DEP" not in hard or "NO_GLOBAL" not in hard:
        fail("control kernel weakened true-shard independence")
    if "external_partner" in control.get("collaboration", {}):
        fail("external collaboration must remain retired")
    for decision in expected:
        if decision not in ledger:
            fail(f"authority ledger is missing {decision}")


def validate_shapes(state: dict, control: dict, evidence: dict, contract: dict) -> None:
    if state.get("schema") != "txpool-v8-live-state-v6":
        fail("unknown live-state schema")
    if evidence.get("schema") != "txpool-v8-evidence-v2":
        fail("unknown evidence schema")
    source = state.get("current_source")
    action = state.get("next_atomic_action")
    if not isinstance(source, dict) or not isinstance(action, dict):
        fail("source or next action is absent")
    if action.get("id") != state.get("single_current_root"):
        fail("the action and single root disagree")
    if set(state.get("claim_status", {})) != CLAIMS:
        fail("claim vocabulary drifted")
    if action.get("continuation_refs") != []:
        fail("external receipts cannot gate continuation")
    refs = action.get("input_evidence_refs")
    if not isinstance(refs, list) or not refs:
        fail("the action has no evidence root")
    for reference in refs:
        resolve(reference, evidence)
    scopes = action.get("write_scope")
    if not isinstance(scopes, list) or not scopes:
        fail("the action has no write scope")
    if any(not within(scope, [source.get("owned_scope", "")]) for scope in scopes):
        fail("action write scope escapes the owned tree")
    forbidden = {
        "current_source",
        "current_phase",
        "single_current_root",
        "next_atomic_action",
        "claim_status",
    }
    for name, owner in (("control", control), ("evidence", evidence), ("contract", contract)):
        overlap = forbidden & nested_keys(owner)
        if overlap:
            fail(f"{name} duplicates live state keys: {sorted(overlap)}")


def recovery_packet(state: dict) -> dict:
    source = state["current_source"]
    reference = source["checkpoint_ref"]
    commit = source["checkpoint_commit"]
    if git_text("rev-parse", reference) != commit:
        fail("checkpoint ref and commit disagree")
    if git_text("rev-parse", f"{commit}^{{tree}}") != source["checkpoint_tree"]:
        fail("checkpoint tree disagrees")
    if git_text("rev-parse", f"{commit}^") != source["base_head"]:
        fail("checkpoint parent disagrees")
    if git_text("rev-parse", "HEAD") != source["base_head"]:
        fail("workspace HEAD moved beyond the frozen base")
    if git_text("symbolic-ref", "--short", "HEAD") != source["branch"]:
        fail("workspace branch disagrees")
    changed = set(
        line
        for line in git_text("diff", "--name-only", commit, "--", source["owned_scope"]).splitlines()
        if line
    )
    changed.update(
        line
        for line in git_text(
            "ls-files", "--others", "--exclude-standard", "--", source["owned_scope"]
        ).splitlines()
        if line
    )
    changed -= LIVE_EXCLUSIONS
    scopes = state["next_atomic_action"]["write_scope"]
    foreign = sorted(path for path in changed if not within(path, scopes))
    if foreign:
        fail("workspace changes escape the active action: " + ", ".join(foreign))
    patch = git("diff", "--binary", commit, "--", source["owned_scope"])
    return {
        "status": "ACTION_DIRTY" if changed else "VALID",
        "checkpoint_commit": commit,
        "checkpoint_tree": source["checkpoint_tree"],
        "changed_paths": sorted(changed),
        "workspace_patch_sha256": sha256(patch),
        "root": state["single_current_root"],
        "next_action": state["next_atomic_action"]["id"],
    }


def manifest_payload() -> dict:
    return {
        "schema": "txpool-v8-project-state-manifest-v4",
        "files": {
            path.relative_to(ROOT).as_posix(): sha256(path.read_bytes()) for path in OWNER_PATHS
        },
    }


def write_manifest() -> None:
    payload = json.dumps(manifest_payload(), indent=2, sort_keys=True) + "\n"
    with tempfile.NamedTemporaryFile("w", dir=CONTROL, delete=False) as output:
        output.write(payload)
        temporary = Path(output.name)
    os.replace(temporary, MANIFEST)


def validate_manifest() -> None:
    actual = load(MANIFEST)
    if actual != manifest_payload():
        fail("manifest hashes or owner set drifted; run --write-manifest")


def self_test(state: dict, control: dict, evidence: dict, contract: dict) -> None:
    changed = copy.deepcopy(state)
    changed["next_atomic_action"]["input_evidence_refs"] = ["EVIDENCE.json#/missing"]
    try:
        validate_shapes(changed, control, evidence, contract)
    except SystemExit:
        pass
    else:
        fail("self-test accepted missing evidence")
    changed = copy.deepcopy(state)
    changed["next_atomic_action"]["continuation_refs"] = ["OBSOLETE_GATE"]
    try:
        validate_shapes(changed, control, evidence, contract)
    except SystemExit:
        pass
    else:
        fail("self-test accepted an obsolete continuation gate")
    changed = copy.deepcopy(state)
    changed["next_atomic_action"]["write_scope"] = ["outside-tx-pool"]
    try:
        validate_shapes(changed, control, evidence, contract)
    except SystemExit:
        pass
    else:
        fail("self-test accepted a scope escape")


def main() -> int:
    arguments = sys.argv[1:]
    allowed = {(), ("--self-test",), ("--recover-json",), ("--recover-self-test",), ("--write-manifest",)}
    if tuple(arguments) not in allowed:
        fail("unsupported arguments")
    state = load(CONTROL / "STATE.json")
    control = load(CONTROL / "CONTROL_KERNEL.json")
    evidence = load(CONTROL / "EVIDENCE.json")
    contract = load(ROOT / "tx-pool/architecture-contract.json")
    ledger = (CONTROL / "CKB_AUTHORITY_INPUT_LEDGER.md").read_text()
    validate_shapes(state, control, evidence, contract)
    validate_contract(contract, control, ledger)
    packet = recovery_packet(state)
    if arguments == ["--write-manifest"]:
        write_manifest()
    else:
        validate_manifest()
    if arguments in (["--self-test"], ["--recover-self-test"]):
        self_test(state, control, evidence, contract)
        packet["negative_self_test"] = "PASS"
    if arguments == ["--recover-self-test"]:
        packet["recovery_self_test"] = "PASS"
    print(json.dumps(packet, ensure_ascii=False, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
