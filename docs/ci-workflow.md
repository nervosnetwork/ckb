# CI Workflow Documentation

This document explains how the CKB CI (Continuous Integration) workflow operates, including the difference between required and optional checks, and how duplicate runs are prevented.

## Overview

The CI workflow runs tests and checks across multiple operating systems (Ubuntu, macOS, Windows) for various job types (quick checks, unit tests, integration tests, benchmarks, linters, etc.). The workflow is designed to:

- Run all tests automatically on PRs and protected branches
- Make Ubuntu jobs required for PR merges while other OS jobs are optional (but still block if they fail)
- Prevent duplicate workflow runs on both PR events and push events
- Support manual workflow triggering for testing purposes

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

## Manual Workflow Testing

All CI workflows support manual triggering via `workflow_dispatch` and can run on any branch. This allows you to:

1. Go to the Actions tab in GitHub
2. Select the workflow you want to run
3. Click "Run workflow"
4. Choose any branch to run on (not limited to master, develop, or rc/*)

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
- `push`: On all branches (master, develop, rc/*, and any other branch)
- `merge_group`: For merge queue
- `workflow_dispatch`: For manual triggering

This means:
- PR events always run
- Push events run on any branch
- Manual dispatch always runs

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
