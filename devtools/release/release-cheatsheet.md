# Release Cheatsheet

This document outlines the release process for the CKB project.

## Overview

The CKB project uses a dual-release system:
- **Non-root crates**: Managed automatically by `release-plz`
- **Root `ckb` crate and binary packages**: Released together via a unified workflow

## Non-Root Crates (release-plz)

All crates in the workspace **except** the root `ckb` crate are managed by [release-plz](https://github.com/release-plz/release-plz).

### How It Works

1. **Automatic PR Creation**: When changes are pushed to the `develop` branch, release-plz automatically:
   - Detects version changes needed
   - Creates a release PR with version bumps
   - Updates changelogs

2. **Automatic Publishing**: Once the release PR is merged:
   - New versions are automatically published to crates.io
   - No manual intervention required

### Important Notes

⚠️ **Breaking Changes**: Release-plz may not catch all breaking changes automatically. Manual version bumping is appreciated when you know a change is breaking.

⚠️ **New Crates**: When adding a new crate to the workspace:
   - The crate must be manually published to crates.io the first time before release-plz can manage it automatically
   - **Trusted publisher must be added** (see [Trusted Publishing Setup](#trusted-publishing-setup) below)
   - After the initial manual publication, release-plz will handle subsequent releases

### Configuration

The root crate is excluded from release-plz via `.release-plz.toml`:
```toml
[[package]]
name = "ckb"
release = false
```

### Trusted Publishing Setup

For release-plz to automatically publish crates to crates.io, trusted publishing must be configured for each crate:

1. **Navigate to crates.io**: Go to crate settings page such as https://crates.io/crates/ckb/settings
2. **Add GitHub Actions as Trusted Publisher**:
   - Click "Add"
   - Enter the workflow file: `release-plz.yml`
   - Save the publisher

**Note**: This must be done for each crate that will be published. The repository owner or someone with crate ownership permissions can configure this.

## Root Crate (`ckb`) and Binary Packages

The root `ckb` crate and binary packages are released together through a unified workflow that combines binary packaging and crate publishing.

### Release Phases

The release process consists of three phases: **Freeze**, **RC** (Release Candidate), and **Release**.

#### Phase 1: Freeze

1. **Security Review**: Review [security issues](https://github.com/nervosnetwork/ckb/security/advisories)

2. **Create RC Branch**: Create a release candidate branch from `develop` following the pattern `rc/v{major}.{minor}.x`
   ```bash
   git checkout develop
   git checkout -b rc/v0.204.x
   git push -u origin rc/v0.204.x
   ```

3. **Update Assume Valid Target**: Update the default assume valid target in the RC branch
   ```bash
   devtools/release/update_default_valid_target.sh
   # Commit the changes
   ```

4. **Generate Changelog**: Generate CHANGELOG from Pull Requests using [github-changelog.py](https://gist.github.com/doitian/0ce3a86cc737bc9e0153d2ab2a52746e)
   ```bash
   github-changelog.py v${last_release}
   ```

5. **Notify Stakeholders**: Create a public issue with release information and notify relevant Discord channels

#### Phase 2: RC (Release Candidate)

1. **Bump Version for RC**: Bump version with RC suffix and push to `pkg/` branch
   ```bash
   devtools/release/bump.sh ${major}.${minor}.${patch}-rc${rc_number}
   # Commit the changes, then
   devtools/release/release-pkg.sh push-pkg
   ```
   This creates a branch `pkg/v{major}.{minor}.{patch}-rc{rc_number}` which triggers `.github/workflows/package.yaml`

2. **Wait for Packaging**: Monitor the [packaging CI](https://github.com/nervosnetwork/ckb/actions/workflows/package.yaml)

3. **Smoking Test**: Download the RC binary and verify it can synchronize with mainnet
   ```bash
   ckb run
   ```

4. **Publish Docker Images**: Build and publish Docker images
   ```bash
   make docker
   make docker-aarch64
   make docker-publish
   ```

5. **Publish RC Release**: Publish the RC version on GitHub
   - Edit the release title to match `ckb --version` output
   - Generate release notes

6. **Deploy and Test**: Deploy RC to testnet and run regression tests

⚠️ **Release when no bugs found in a week**

#### Phase 3: Release

1. **Update CHANGELOG**: Update the CHANGELOG with final release notes

2. **Bump Final Version**: Bump to final version
   ```bash
   devtools/release/bump.sh ${major}.${minor}.${patch}
   ```

3. **Push to pkg/ Branch**: Push to packaging branch
   ```bash
   devtools/release/release-pkg.sh push-pkg
   ```
   This creates a branch `pkg/v{major}.{minor}.{patch}` which triggers `.github/workflows/package.yaml`

4. **Wait for Packaging**: Wait for packaging CI to complete

5. **Smoking Test**: Verify the release binary works correctly

6. **Create and Push Tag**: Create the release tag and push it
   ```bash
   devtools/release/release-pkg.sh tag
   devtools/release/release-pkg.sh push-tag
   ```
   The tag automatically triggers `.github/workflows/publish-root-ckb-crate.yaml` to publish the root crate to crates.io

7. **Publish Release**: Publish the version on GitHub
   - Edit the release title to match `ckb --version` output
   - Generate release notes

8. **Publish Docker Images**: Build and publish Docker images
   ```bash
   make docker
   make docker-aarch64
   make docker-publish
   ```

9. **Merge Back**: Merge master back to develop
    ```bash
    devtools/git/merge-master.sh
    ```

10. **Bump Develop Version**: Bump version in develop to next minor version
    ```bash
    devtools/release/bump.sh ${major}.${minor}.0-pre
    ```

11. **Notify Stakeholders**: Notify Discord channels and deploy the new version

### Version Requirements

- **RC Branch Format**: `rc/v{major}.{minor}.x` (e.g., `rc/v0.204.x`)
- **RC Package Branch Format**: `pkg/v{major}.{minor}.{patch}-rc{rc_number}` (e.g., `pkg/v0.204.0-rc1`)
- **Release Package Branch Format**: `pkg/v{major}.{minor}.{patch}` (e.g., `pkg/v0.204.0`)
- **Tag Format**: `v{major}.{minor}.{patch}` (e.g., `v0.204.0`)

### Version Verification

The root crate publishing workflow includes a verification step that ensures:
- Tag version (e.g., `v0.204.0` → `0.204.0`) matches `Cargo.toml` version
- If versions don't match, the workflow fails and publishing is aborted

### Packaging Workflow

When a `pkg/*` branch is pushed, `.github/workflows/package.yaml` automatically:
- Creates a draft GitHub release with the version tag
- Builds and packages binaries for multiple platforms:
  - Linux (x86_64, aarch64)
  - macOS (x86_64, aarch64)
  - Windows (x86_64)
  - CentOS (x86_64)
  - Portable variants for Linux and macOS
- Signs the packages with GPG
- Uploads artifacts to the draft release

## Workflow Files

- **Release-plz**: `.github/workflows/release-plz.yml`
  - Runs on pushes to `develop` branch
  - Creates release PRs and publishes non-root crates

- **Binary packaging**: `.github/workflows/package.yaml`
  - Runs on pushes to branches matching `pkg/*`
  - Creates draft releases and packages binaries for all supported platforms

- **Root crate publishing**: `.github/workflows/publish-root-ckb-crate.yaml`
  - Runs on tag pushes matching `v[0-9]+.[0-9]+.[0-9]+`
  - Publishes the root `ckb` crate to crates.io
  - Triggered automatically when a release is published

## Helper Scripts

- **`devtools/release/bump.sh`**: Bumps version in `Cargo.toml` and `README.md`
- **`devtools/release/release-pkg.sh`**: Helper script for packaging workflow
  - `push-pkg`: Push current branch to `pkg/v{version}` branch
  - `tag`: Create a signed release tag
  - `push-tag`: Push the release tag
- **`devtools/release/update_default_valid_target.sh`**: Update assume valid target
- **`devtools/git/merge-master.sh`**: Merge master back to develop

## Best Practices

1. **Version Bumping**: Always ensure version bumps follow semantic versioning
2. **Breaking Changes**: Manually verify and bump versions for breaking changes in non-root crates
3. **Version Consistency**: Ensure the version in `Cargo.toml` matches the tag version before publishing
4. **RC Testing**: Always smoke test RC versions before final release
5. **Release Review**: Review draft releases and binary packages before publishing
6. **Security**: Review security advisories before creating RC branches
7. **Stakeholder Communication**: Notify relevant channels and create public issues for transparency
8. **Wait Period**: Release only after no bugs found in RC for at least a week
