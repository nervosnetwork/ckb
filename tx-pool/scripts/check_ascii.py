#!/usr/bin/env python3
"""Reject non-ASCII styling from tx-pool technical source and contracts."""

from __future__ import annotations

from pathlib import Path
import sys


REPO_ROOT = Path(__file__).resolve().parents[2]
TX_POOL = REPO_ROOT / "tx-pool"
TEXT_SUFFIXES = {
    ".json",
    ".md",
    ".py",
    ".rs",
    ".sh",
    ".toml",
    ".txt",
    ".yaml",
    ".yml",
}
SPECIAL_TEXT_FILES = {".release-progress"}
PROFILE = TX_POOL / "scripts" / "profile.py"
EXTERNAL_MICROSECOND_SIGN = chr(0xB5)


def technical_sources() -> list[Path]:
    return [
        path
        for path in sorted(TX_POOL.rglob("*"))
        if path.is_file()
        and "__pycache__" not in path.parts
        and (path.suffix in TEXT_SUFFIXES or path.name in SPECIAL_TEXT_FILES)
    ]


def allowed(path: Path, character: str) -> bool:
    # Samply's profile schema emits this exact thread-CPU unit token. The parser
    # must spell the external token faithfully; it is not prose styling.
    return path == PROFILE and character == EXTERNAL_MICROSECOND_SIGN


def validate() -> list[str]:
    errors: list[str] = []
    for path in technical_sources():
        try:
            text = path.read_text()
        except UnicodeDecodeError as error:
            errors.append(
                f"cannot decode technical source {path.relative_to(REPO_ROOT)}: {error}"
            )
            continue
        for line_number, line in enumerate(text.splitlines(), start=1):
            for column, character in enumerate(line, start=1):
                if ord(character) > 0x7F and not allowed(path, character):
                    errors.append(
                        f"non-ASCII U+{ord(character):04X} in "
                        f"{path.relative_to(REPO_ROOT)}:{line_number}:{column}"
                    )
    return errors


def main() -> int:
    errors = validate()
    if errors:
        for error in errors:
            print(f"error: {error}", file=sys.stderr)
        return 1
    print(f"validated ASCII technical text in {len(technical_sources())} tx-pool files")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
