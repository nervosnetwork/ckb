#!/usr/bin/env python3
"""Run the discovered tx-pool review contracts through one stable entry point."""

from __future__ import annotations

import argparse
from pathlib import Path
import subprocess
import sys


REPO_ROOT = Path(__file__).resolve().parents[2]
SCRIPT_DIR = Path(__file__).resolve().parent
SELF = Path(__file__).resolve()
LIGHT_EXCLUDED_CHECKS = {
    "check_formal_models.py",
    "check_security_manifest.py",
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--light",
        action="store_true",
        help="skip validators that require Rust test discovery or external TLC",
    )
    return parser.parse_args()


def discovered_checks(light: bool) -> list[Path]:
    checks = [
        path
        for path in sorted(SCRIPT_DIR.glob("check_*.py"))
        if path.resolve() != SELF
    ]
    if light:
        checks = [path for path in checks if path.name not in LIGHT_EXCLUDED_CHECKS]
    return checks


def validate_python_sources() -> list[str]:
    errors: list[str] = []
    for path in sorted(SCRIPT_DIR.glob("*.py")):
        try:
            compile(path.read_text(), str(path), "exec")
        except (OSError, SyntaxError) as error:
            errors.append(f"cannot compile {path.relative_to(REPO_ROOT)}: {error}")
    return errors


def main() -> int:
    args = parse_args()
    syntax_errors = validate_python_sources()
    if syntax_errors:
        for error in syntax_errors:
            print(f"error: {error}", file=sys.stderr)
        return 1

    checks = discovered_checks(args.light)
    if not checks:
        print("error: no tx-pool review contracts were discovered", file=sys.stderr)
        return 1
    for check in checks:
        relative = check.relative_to(REPO_ROOT)
        print(f"running {relative}", flush=True)
        completed = subprocess.run(
            [sys.executable, str(check)],
            cwd=REPO_ROOT,
            check=False,
        )
        if completed.returncode != 0:
            return completed.returncode
    mode = "light" if args.light else "complete"
    print(f"validated {len(checks)} discovered tx-pool contracts ({mode})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
