#!/usr/bin/env python3
"""Run the single active tx-pool proof-control checker."""

from __future__ import annotations

import argparse
from pathlib import Path
import subprocess
import sys


REPO_ROOT = Path(__file__).resolve().parents[2]
CHECKER = Path(__file__).resolve().with_name("check_security_manifest.py")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--light",
        action="store_true",
        help="compatibility spelling for the same lightweight structural check",
    )
    parser.parse_args()
    command = [sys.executable, str(CHECKER)]
    completed = subprocess.run(command, cwd=REPO_ROOT, check=False)
    if completed.returncode == 0:
        print("validated the single active tx-pool proof-control checker")
    return completed.returncode


if __name__ == "__main__":
    raise SystemExit(main())
