#!/usr/bin/env python3
"""Run the single tx-pool project-state verifier and its contract checker."""

from __future__ import annotations

import argparse
from pathlib import Path
import subprocess
import sys


REPO_ROOT = Path(__file__).resolve().parents[2]
VERIFIER = REPO_ROOT / "tx-pool/control/txpool-v8/VERIFY_STATE.py"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--light",
        action="store_true",
        help="compatibility spelling for the same lightweight structural check",
    )
    parser.parse_args()
    command = [sys.executable, "-B", str(VERIFIER)]
    completed = subprocess.run(command, cwd=REPO_ROOT, check=False)
    if completed.returncode == 0:
        print("validated the single active tx-pool project state and proof-control checker")
    return completed.returncode


if __name__ == "__main__":
    raise SystemExit(main())
