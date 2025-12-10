#!/usr/bin/env python3
"""
Script to create a release issue using the template.

Usage:
    python3 devtools/release/create-release-issue.py [--dry-run]
"""

import argparse
import re
import subprocess
import sys
from datetime import datetime, timedelta
from pathlib import Path


def get_repo_root():
    """Get the repository root directory."""
    script_dir = Path(__file__).parent.resolve()
    return script_dir.parent.parent


def calculate_dates():
    """Calculate RC date (tomorrow) and release date (tomorrow + 7 days)."""
    tomorrow = datetime.now() + timedelta(days=1)
    release_date = tomorrow + timedelta(days=7)
    return tomorrow.strftime("%Y-%m-%d"), release_date.strftime("%Y-%m-%d")


def extract_assume_valid_targets(repo_root):
    """Extract mainnet and testnet hashes from latest_assume_valid_target.rs."""
    assume_valid_file = repo_root / "util/constant/src/latest_assume_valid_target.rs"

    if not assume_valid_file.exists():
        print(f"Error: {assume_valid_file} not found", file=sys.stderr)
        sys.exit(1)

    content = assume_valid_file.read_text()

    # Extract mainnet hash - match until the closing brace of the mainnet module
    mainnet_match = re.search(
        r'mod mainnet\s*\{.*?DEFAULT_ASSUME_VALID_TARGET.*?"(0x[0-9a-f]+)".*?\n\}',
        content,
        re.DOTALL,
    )

    # Extract testnet hash - match until the closing brace of the testnet module
    testnet_match = re.search(
        r'mod testnet\s*\{.*?DEFAULT_ASSUME_VALID_TARGET.*?"(0x[0-9a-f]+)".*?\n\}',
        content,
        re.DOTALL,
    )

    if not mainnet_match or not testnet_match:
        print(
            f"Error: Could not extract assume valid targets from {assume_valid_file}",
            file=sys.stderr,
        )
        sys.exit(1)

    return mainnet_match.group(1), testnet_match.group(1)


def get_last_release_tag(repo_root):
    """Get the last release tag (non-RC, format: v[0-9]+.[0-9]+.[0-9]+)."""
    try:
        result = subprocess.run(
            ["git", "tag", "--list", "v[0-9]*", "--sort=-version:refname"],
            cwd=repo_root,
            capture_output=True,
            text=True,
            check=True,
        )

        tags = result.stdout.strip().split("\n")
        # Filter for non-RC tags (exclude tags with -rc suffix)
        release_tags = [
            tag for tag in tags if re.match(r"^v[0-9]+\.[0-9]+\.[0-9]+$", tag)
        ]

        if not release_tags:
            print("Error: Could not find last release tag", file=sys.stderr)
            sys.exit(1)

        return release_tags[0]
    except subprocess.CalledProcessError as e:
        print(f"Error running git command: {e}", file=sys.stderr)
        sys.exit(1)


def read_changelog(repo_root):
    """Read changelog from .git/changes/out.md and adjust heading levels."""
    changelog_file = repo_root / ".git/changes/out.md"

    if not changelog_file.exists():
        print(
            f"Warning: {changelog_file} not found. Changelog will be empty.",
            file=sys.stderr,
        )
        return "*No changelog available. Please generate it using github-changelog.py*"

    return changelog_file.read_text()


def get_current_branch(repo_root):
    """Get current branch or HEAD commit."""
    try:
        result = subprocess.run(
            ["git", "rev-parse", "--abbrev-ref", "HEAD"],
            cwd=repo_root,
            capture_output=True,
            text=True,
            check=True,
        )
        branch = result.stdout.strip()
        # If detached HEAD, get the commit SHA instead
        if branch == "HEAD":
            result = subprocess.run(
                ["git", "rev-parse", "HEAD"],
                cwd=repo_root,
                capture_output=True,
                text=True,
                check=True,
            )
            return result.stdout.strip()
        return branch
    except subprocess.CalledProcessError:
        return None


def get_compare_url(repo_root, last_release):
    """Generate GitHub compare URL between last release and current branch."""
    # Hardcoded repo URL
    repo_url = "https://github.com/nervosnetwork/ckb"
    current_branch = get_current_branch(repo_root)

    if not current_branch:
        return None

    return f"{repo_url}/compare/{last_release}...{current_branch}"


def get_issue_title(repo_root, last_release):
    """Generate issue title based on current branch or last release."""
    try:
        result = subprocess.run(
            ["git", "rev-parse", "--abbrev-ref", "HEAD"],
            cwd=repo_root,
            capture_output=True,
            text=True,
            check=True,
        )
        branch = result.stdout.strip()

        # If on an RC branch, use that for the title
        rc_match = re.match(r"rc/v([0-9]+\.[0-9]+)\.x", branch)
        if rc_match:
            version = rc_match.group(1)
            return f"Release v{version}.0"

        # Otherwise, try to infer next version from last release
        version_match = re.match(r"v([0-9]+)\.([0-9]+)\.([0-9]+)", last_release)
        if version_match:
            major, minor, patch = version_match.groups()
            # Assume next minor version
            next_minor = int(minor) + 1
            return f"Release v{major}.{next_minor} RC"
    except subprocess.CalledProcessError:
        pass

    # Fallback
    return "Release RC"


def create_issue_with_gh(title, body, dry_run=False):
    """Create GitHub issue using gh CLI."""
    if dry_run:
        print("=" * 80, file=sys.stderr)
        print("DRY RUN MODE - Issue would be created with:", file=sys.stderr)
        print("=" * 80, file=sys.stderr)
        print(f"\nTitle: {title}\n", file=sys.stderr)
        print("Body:", file=sys.stderr)
        print("-" * 80, file=sys.stderr)
        print(body, file=sys.stderr)
        print("-" * 80, file=sys.stderr)
        return None

    try:
        # Check if gh is available
        subprocess.run(["gh", "--version"], capture_output=True, check=True)
    except (subprocess.CalledProcessError, FileNotFoundError):
        print(
            "Error: 'gh' CLI tool not found. Please install it from https://cli.github.com/",
            file=sys.stderr,
        )
        sys.exit(1)

    # Create issue using gh CLI
    try:
        result = subprocess.run(
            ["gh", "issue", "create", "--title", title, "--body", body],
            capture_output=True,
            text=True,
            check=True,
        )
        issue_url = result.stdout.strip()
        print(f"Created issue: {issue_url}", file=sys.stderr)
        return issue_url
    except subprocess.CalledProcessError as e:
        print(f"Error creating issue: {e}", file=sys.stderr)
        if e.stderr:
            print(e.stderr, file=sys.stderr)
        sys.exit(1)


def main():
    """Main function to generate the release issue."""
    parser = argparse.ArgumentParser(
        description="Create a release issue using the template"
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Print the issue content without creating it",
    )
    args = parser.parse_args()

    repo_root = get_repo_root()

    # Calculate dates
    rc_date, release_date = calculate_dates()

    # Extract assume valid targets
    mainnet_hash, testnet_hash = extract_assume_valid_targets(repo_root)

    # Get last release tag
    last_release = get_last_release_tag(repo_root)

    # Read changelog
    changelog = read_changelog(repo_root)

    # Get compare URL
    compare_url = get_compare_url(repo_root, last_release)
    compare_section = ""
    if compare_url:
        compare_section = f"\n[Compare changes]({compare_url})\n"

    # Generate the release issue content
    body = f"""- RC Date: {rc_date}
- Release Date: {release_date}
- Assume Valid Target: (Can be found in the file util/constant/src/latest_assume_valid_target.rs)
    - Mainnet: [{mainnet_hash}](https://explorer.nervos.org/block/{mainnet_hash})
    - Testnet: [{testnet_hash}](https://testnet.explorer.nervos.org/block/{testnet_hash})


## Changes since {last_release}{compare_section}

{changelog}
"""

    # Generate issue title
    title = get_issue_title(repo_root, last_release)

    # Create issue or print in dry-run mode
    create_issue_with_gh(title, body, dry_run=args.dry_run)


if __name__ == "__main__":
    main()
