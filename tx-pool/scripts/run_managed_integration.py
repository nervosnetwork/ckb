#!/usr/bin/env python3
"""Run the canonical tx-pool integration universe through make integration."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import shlex
import subprocess
import sys


REPO_ROOT = Path(__file__).resolve().parents[2]
IMPACT_PATH = REPO_ROOT / "tx-pool" / "integration-impact.json"
REGISTRY_PATH = REPO_ROOT / "tx-pool" / "review-behaviors.json"
MAKE_NAME = re.compile(r"^[A-Za-z0-9_-]+$")
VARIABLE_NAME = re.compile(r"^[A-Z][A-Z0-9_]*$")
SPEC_NAME = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--anchors",
        action="store_true",
        help="run the focused security anchors instead of the complete impact set",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="validate and print the derived invocation without running it",
    )
    return parser.parse_args()


def load_json(path: Path) -> dict:
    try:
        value = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"cannot load {path.relative_to(REPO_ROOT)}: {error}") from error
    if not isinstance(value, dict):
        raise ValueError(f"{path.relative_to(REPO_ROOT)} must contain one object")
    return value


def unique_names(values: list[object], owner: str) -> list[str]:
    names: list[str] = []
    seen: set[str] = set()
    for value in values:
        if not isinstance(value, str) or SPEC_NAME.fullmatch(value) is None:
            raise ValueError(f"{owner} contains an invalid spec name: {value!r}")
        if value in seen:
            raise ValueError(f"{owner} contains duplicate spec {value}")
        seen.add(value)
        names.append(value)
    if not names:
        raise ValueError(f"{owner} contains no specs")
    return names


def derive_invocation(anchors: bool) -> tuple[list[str], str, list[str], int]:
    impact = load_json(IMPACT_PATH)
    registry = load_json(REGISTRY_PATH)
    groups = impact.get("groups")
    if impact.get("schema_version") != 1 or not isinstance(groups, dict):
        raise ValueError("integration-impact.json has an unsupported schema")
    if not all(isinstance(values, list) for values in groups.values()):
        raise ValueError("integration-impact.json contains a non-list group")
    impact_specs = unique_names(
        [name for values in groups.values() for name in values],
        "integration-impact.json",
    )

    runner = registry.get("integration_runner")
    if not isinstance(runner, dict):
        raise ValueError("review-behaviors.json has no integration runner")
    target = runner.get("make_target")
    variable = runner.get("arguments_variable")
    common = runner.get("common_arguments")
    if not isinstance(target, str) or MAKE_NAME.fullmatch(target) is None:
        raise ValueError("integration make target is invalid")
    if not isinstance(variable, str) or VARIABLE_NAME.fullmatch(variable) is None:
        raise ValueError("integration arguments variable is invalid")
    if not isinstance(common, list) or not all(
        isinstance(argument, str)
        and argument
        and not any(char.isspace() for char in argument)
        for argument in common
    ):
        raise ValueError("integration common arguments are invalid")
    if impact.get("runner") != f"make {target}":
        raise ValueError("integration runner authorities disagree")

    if anchors:
        evidence = registry.get("integration_evidence")
        if not isinstance(evidence, list):
            raise ValueError("review-behaviors.json has no integration evidence")
        if not all(
            isinstance(entry, dict) and "anchor" in entry for entry in evidence
        ):
            raise ValueError("review-behaviors.json contains invalid integration evidence")
        specs = unique_names(
            [entry["anchor"] for entry in evidence],
            "review-behaviors.json integration evidence",
        )
        missing = sorted(set(specs) - set(impact_specs))
        if missing:
            raise ValueError(f"security anchors are outside the impact set: {missing}")
    else:
        specs = sorted(impact_specs)
    return ["make", target], variable, [*common, *specs], len(specs)


def main() -> int:
    args = parse_args()
    try:
        command, variable, arguments, spec_count = derive_invocation(args.anchors)
    except ValueError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    label = "security anchors" if args.anchors else "managed impact specs"
    make_argument = f"{variable}={' '.join(arguments)}"
    print(f"validated {spec_count} {label}", flush=True)
    if args.dry_run:
        print(shlex.join([*command, make_argument]))
        return 0
    return subprocess.run(
        [*command, make_argument], cwd=REPO_ROOT, check=False
    ).returncode


if __name__ == "__main__":
    raise SystemExit(main())
