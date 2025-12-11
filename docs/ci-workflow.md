# CI Workflow Documentation

This document explains how the CKB CI (Continuous Integration) workflow operates, including how to skip jobs, the difference between required and optional checks, and how duplicate runs are prevented.

## Overview

The CI workflow runs tests and checks across multiple operating systems (Ubuntu, macOS, Windows) for various job types (quick checks, unit tests, integration tests, benchmarks, linters, etc.). The workflow is designed to:

- Allow selective job execution via PR comments
- Make Ubuntu jobs required for PR merges while other OS jobs are optional (but still block if they fail)
- Prevent duplicate workflow runs on both PR events and push events

## Workflow Structure

Each CI workflow follows a consistent pattern:

1. **Prologue Job**: Determines which jobs should run and on which OS
2. **Test/Check Jobs**: Execute the actual tests or checks based on prologue decisions

### Concurrency Control

Each workflow uses GitHub Actions concurrency groups to prevent duplicate runs:

```yaml
concurrency:
  group: <workflow_name>-${{ github.ref }}
  cancel-in-progress: true
```

This ensures that:
- Only one workflow run per branch/PR is active at a time
- New pushes/PR updates cancel in-progress runs
- The same workflow won't run simultaneously on both PR and push events for the same commit

## Skipping Jobs

You can control which jobs run and on which operating systems by adding special comments to your PR description or commit message.

### How It Works

The `ci_prologue.sh` script parses PR comments or commit messages to determine:
- Which operating systems should run (`ci-runs-on:`)
- Which job types should run (`ci-runs-only:`)
- Whether to use self-hosted runners (`ci-uses-self-runner:`)

### Syntax

Add these directives to your **PR description** or **commit message**:

#### Skip Operating Systems

```
ci-runs-on: [ ubuntu, macos, windows ]
```

- Default: All three OS run (`[ ubuntu, macos, windows ]`)
- To run only Ubuntu: `ci-runs-on: [ ubuntu ]`
- To skip macOS: `ci-runs-on: [ ubuntu, windows ]`

#### Skip Job Types

```
ci-runs-only: [ quick_checks, unit_tests, integration_tests, benchmarks, linters, cargo_deny, aarch64_build ]
```

- Default: All jobs run
- To run only quick checks: `ci-runs-only: [ quick_checks ]`
- To skip benchmarks: `ci-runs-only: [ quick_checks, unit_tests, integration_tests, linters, cargo_deny, aarch64_build ]`

#### Use Self-Hosted Runners

```
ci-uses-self-runner: true
```

- Default: `false` (uses GitHub-hosted runners)
- Set to `true` to use self-hosted runners (requires appropriate permissions)

### Permission Requirements

**For PRs**, you can only customize CI runs if:
- You have the `ci-trust` label on your PR, OR
- You have `admin` or `write` permissions to the repository

**For pushes**, customization is available for:
- Commits to `master` branch
- Merge commits to `develop` branch (commits starting with "Merge pull request #")
- Commits to `rc/*` branches
- Any push in non-nervosnetwork repositories

**For merge_group events** (merge queue), customization is disabled - all jobs run.

### Examples

**Example 1: Run only quick checks on Ubuntu**
```
ci-runs-only: [ quick_checks ]
ci-runs-on: [ ubuntu ]
```

**Example 2: Skip macOS and Windows, run all jobs**
```
ci-runs-on: [ ubuntu ]
```

**Example 3: Run only unit tests and linters on all OS**
```
ci-runs-only: [ unit_tests, linters ]
```

## Required vs Optional Checks

### Ubuntu Jobs: Essential Path

Ubuntu jobs are configured as **required status checks** in the repository settings. This means:

- ✅ PRs **cannot be merged** until all Ubuntu jobs pass
- ✅ Ubuntu jobs block PR merges immediately if they fail
- ✅ These are the "essential path" - the minimum validation required

### Other OS Jobs: Optional but Blocking

macOS and Windows jobs are **not** configured as required status checks, which means:

- ✅ PRs **can be merged** even if macOS/Windows jobs are still running
- ⚠️ However, if macOS/Windows jobs **finish and fail**, the PR **cannot be merged**
- ✅ This allows faster iteration - you don't have to wait for slower macOS/Windows runners
- ⚠️ But ensures cross-platform compatibility - failures still block merges

### Why This Design?

1. **Speed**: Ubuntu runners are typically faster and more available, allowing quicker feedback
2. **Flexibility**: Developers can merge PRs without waiting for all OS tests if Ubuntu passes
3. **Safety**: Cross-platform failures still prevent merges, ensuring compatibility
4. **Resource Efficiency**: macOS and Windows runners may be slower or have limited capacity

## Avoiding Duplicate Runs

The CI workflow prevents duplicate runs through several mechanisms:

### 1. Concurrency Groups

Each workflow uses a concurrency group based on the workflow name and git reference:

```yaml
concurrency:
  group: ci_integration_tests_ubuntu-${{ github.ref }}
  cancel-in-progress: true
```

- `${{ github.ref }}` is the same for both PR and push events on the same branch
- `cancel-in-progress: true` cancels any existing run when a new one starts
- This ensures only one run per workflow per branch/PR at a time

### 2. Event Triggers

Workflows trigger on:
- `pull_request`: `opened`, `synchronize`, `reopened`
- `push`: Only on specific branches (`master`, `develop`, `rc/*`)
- `merge_group`: For merge queue

The prologue job has additional conditions to prevent unnecessary runs on push events:

```yaml
if: |
  github.event_name != 'push' ||
  ( github.event_name == 'push' &&
   ( github.ref == 'refs/heads/master' ||
     (github.ref == 'refs/heads/develop' && startsWith(github.event.head_commit.message, 'Merge pull request #')) ||
     startsWith(github.ref, 'refs/heads/rc/')
   )
  ) || (github.repository_owner != 'nervosnetwork')
```

This means:
- PR events always run the prologue
- Push events only run on protected branches or merge commits
- Forks can always run (for testing)

### 3. Workflow Execution Flow

When a PR is opened/updated:
1. Workflow runs triggered by `pull_request` event
2. Concurrency group ensures only one run per workflow

When a PR is merged (push to `develop`):
1. The merge commit triggers `push` event
2. Concurrency group matches the PR's reference, canceling any remaining PR runs
3. New push-based run executes

This prevents the same commit from running tests twice - once as a PR and once as a push.

## Workflow Files

CI workflows are organized by job type and OS:

- `ci_quick_checks_ubuntu.yaml` / `ci_quick_checks_macos.yaml`
- `ci_unit_tests_ubuntu.yaml` / `ci_unit_tests_macos.yaml`
- `ci_integration_tests_ubuntu.yaml` / `ci_integration_tests_macos.yaml` / `ci_integration_tests_windows.yaml`
- `ci_benchmarks_ubuntu.yaml` / `ci_benchmarks_macos.yaml`
- `ci_linters_ubuntu.yaml` / `ci_linters_macos.yaml`
- `ci_aarch64_build_ubuntu.yaml`

Each workflow follows the same structure with a `prologue` job and one or more test/check jobs.

## Troubleshooting

### Jobs are being skipped unexpectedly

1. Check your PR description or commit message for `ci-runs-on:` or `ci-runs-only:` directives
2. Verify you have the required permissions (`ci-trust` label or `admin`/`write` access)
3. Check the prologue job output in the workflow run logs

### Duplicate runs are occurring

1. Verify concurrency groups are properly configured
2. Check if workflows are triggered by both PR and push events for the same commit
3. Review the prologue job's `if` condition to ensure it's not running unnecessarily

### Required checks not passing

1. Ubuntu jobs must pass - these are required status checks
2. macOS/Windows jobs can be in progress, but if they finish and fail, they will block the PR
3. Check the workflow run logs for specific failure details
