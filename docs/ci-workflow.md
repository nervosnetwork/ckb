# CI Workflow Documentation

This document explains how the CKB CI (Continuous Integration) workflow operates, including the difference between required and optional checks, and how duplicate runs are prevented.

## Overview

The CI workflow runs tests and checks across multiple operating systems (Ubuntu, macOS, Windows) for various job types (quick checks, unit tests, integration tests, benchmarks, linters, etc.). The workflow is designed to:

- Run all Ubuntu jobs automatically on PRs and protected branches
- Run only quick macOS/Windows checks on PRs
- Run full macOS/Windows jobs on merge queue, protected branches, and manual dispatch
- Make Ubuntu jobs required for PR merges while other OS jobs are optional (but still block if they fail)
- Prevent duplicate workflow runs on both PR events and push events
- Support manual workflow triggering for testing purposes

## Manual Workflow Testing

All CI workflows support manual triggering via `workflow_dispatch` and can run on any branch. This allows you to:

1. Go to the Actions tab in GitHub
2. Select the workflow you want to run
3. Click "Run workflow"
4. Choose any branch to run on (not limited to master, develop, or rc/**)

This is useful for testing workflow changes on dedicated branches without creating a PR. To test changes:

1. Push your changes to a dedicated test branch (e.g., `test-ci-changes`)
2. Manually trigger the workflow on that branch
3. Verify the workflow runs as expected

## Required vs Optional Checks

### Ubuntu Jobs: Essential Path

Ubuntu jobs are configured as **required status checks** in the repository settings. This means:

- ✅ PRs **cannot be merged** until all Ubuntu jobs pass
- ✅ Ubuntu jobs block PR merges immediately if they fail
- ✅ These are the "essential path" - the minimum validation required

### macOS/Windows Jobs: Layered Desktop Path

macOS and Windows jobs are split into quick PR checks and full protected-branch checks:

- ✅ PRs run quick macOS/Windows checks, including `cargo check`, for early cross-platform signal
- ✅ Full macOS/Windows jobs run in `merge_group`, `develop`, `master`, `rc/**`, `pkg/*`, and manual dispatch
- ✅ This keeps regular PR feedback cheaper while preserving full desktop validation before or after protected-branch changes
- ✅ If a Ubuntu CI workflow fails, is cancelled, or times out, `ci_cancel_desktop_on_ubuntu_failure` cancels queued or running macOS/Windows CI for the same commit

### Why This Design?

1. **Speed**: Ubuntu runners provide full PR feedback quickly
2. **Cost control**: PRs avoid running the most expensive full macOS/Windows jobs by default
3. **Safety**: merge queue and protected branches still run full desktop validation
4. **Flexibility**: maintainers can manually dispatch full desktop workflows when needed

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
- `push`: Ubuntu workflows run on all branches; desktop workflows run on `develop`, `master`, `rc/**`, and `pkg/*`
- `merge_group`: For merge queue
- `workflow_dispatch`: For manual triggering

This means:

- PR events always run
- Ubuntu push events run on any branch
- Desktop push events run only on protected branches
- Manual dispatch always runs

### 3. Workflow Execution Flow

When a PR is opened/updated:

1. Ubuntu workflows run the full CI set
2. macOS and Windows quick-check workflows run
3. Full macOS/Windows workflows wait for `merge_group`, protected-branch pushes, or manual dispatch

When a PR is merged (push to `develop`):

1. The merge commit triggers `push` event
2. Ubuntu workflows run again on the protected branch
3. Full macOS/Windows workflows run on the protected branch

## Workflow Files

CI workflows are organized by job type and OS:

- `ci_quick_checks_ubuntu.yaml` / `ci_quick_checks_macos.yaml` / `ci_quick_checks_windows.yaml`
- `ci_unit_tests_ubuntu.yaml` / `ci_unit_tests_macos.yaml` / `ci_unit_tests_windows.yaml`
- `ci_integration_tests_ubuntu.yaml` / `ci_integration_tests_macos.yaml` / `ci_integration_tests_windows.yaml`
- `ci_benchmarks_ubuntu.yaml` / `ci_benchmarks_macos.yaml` / `ci_benchmarks_windows.yaml`
- `ci_linters_ubuntu.yaml` / `ci_linters_macos.yaml`
- `ci_cargo_deny_ubuntu.yaml`
- `ci_aarch64_build_ubuntu.yaml`

## Troubleshooting

### Jobs are not running as expected

1. Check the workflow is triggered by the correct event
2. Verify concurrency groups are properly configured
3. For manual testing, use workflow_dispatch to trigger on any branch

### Duplicate runs are occurring

1. Verify concurrency groups are properly configured
2. Check if workflows are triggered by both PR and push events for the same commit

### Required checks not passing

1. Ubuntu jobs must pass - these are required status checks
2. macOS/Windows jobs can be in progress, but if they finish and fail, they will block the PR
3. Check the workflow run logs for specific failure details
