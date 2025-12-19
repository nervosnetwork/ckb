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

### Conventional Commit Messages

Release-plz assumes you are using [Conventional Commit messages](https://www.conventionalcommits.org/).

The most important prefixes you should have in mind are:

- `fix:`: represents bug fixes, and results in a SemVer patch bump.
- `feat:`: represents a new feature, and results in a SemVer minor bump.
- `<prefix>!:` (e.g. `feat!:`): represents a breaking change (indicated by the !) and results in a SemVer major bump.

Commits that don't follow the Conventional Commit format result in a SemVer patch bump.

### Important Notes

⚠️ **Breaking Changes**: Release-plz may not catch all breaking changes automatically. Manual version bumping is appreciated when you know a change is breaking.

⚠️ **New Crates**: When adding a new crate to the workspace:
   - The crate must be manually published to crates.io the first time before release-plz can manage it automatically
   - **Trusted publisher must be added** (see [Trusted Publishing Setup](#trusted-publishing-setup) below)
   - After the initial manual publication, release-plz will handle subsequent releases

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

### Minor Release

Minor releases are the regular release cycle for the CKB project, occurring approximately every **6 weeks**. These releases bump the minor version (`v{major}.{minor + 1}.0`) and include new features, improvements, and bug fixes accumulated during the development cycle.

The release process consists of three phases: **Freeze**, **RC** (Release Candidate), and **Release**.

#### Phase 1: Freeze

The freeze phase marks the beginning of the release cycle. At this point, feature development is frozen, and the focus shifts to stabilization and testing.

1. **Merge Release-plz PR**: Merge any pending release-plz PR.

2. **Create RC Branch**: Create an RC branch from `develop` following the pattern `rc/v{major}.{minor}.x`.
   ```bash
   git checkout develop
   git checkout -b rc/v0.204.x
   git push -u upstream rc/v0.204.x
   ```

3. **Update Assume Valid Target**: Update the default assume valid target in the RC branch.
   ```bash
   devtools/release/update_default_valid_target.sh
   # Review and commit the changes
   ```

4. **Bump Version for RC**: Bump version with RC suffix (e.g., `0.204.0-rc1`) and generate CHANGELOG using the script `bump.sh`.
   ```bash
   devtools/release/bump.sh ${major}.${minor}.${patch}-rc${rc_number}
   # Review the generated CHANGELOG and commit the changes, then
   git push -u upstream rc/v0.204.x
   ```

5. **Notify Stakeholders**: Create a public issue with release information (`devtools/release/create-release-issue.py`) and notify relevant Discord channels to inform the community about the upcoming release.

#### Phase 2: RC (Release Candidate)

The RC phase involves building, testing, and validating the release candidate. Multiple RCs may be created if issues are discovered.

1. **Bump Version for RC**: Bump version and generate CHANGELOG if this is not the first RC version.
   ```bash
   devtools/release/bump.sh ${major}.${minor}.${patch}-rc${rc_number}
   # Review the generated CHANGELOG and commit the changes, then
   git push -u upstream rc/v0.204.x
   ```

2. **Trigger Packaging CI**: Push the release candidate from the RC branch to a `pkg/` branch to trigger the packaging workflow
   ```bash
   devtools/release/release-pkg.sh push-pkg
   ```
   This creates a branch `pkg/v{major}.{minor}.{patch}-rc{rc_number}` which automatically triggers `.github/workflows/package.yaml`

3. **Wait for Packaging**: Monitor the [packaging CI](https://github.com/nervosnetwork/ckb/actions/workflows/package.yaml) to ensure all platform builds complete successfully

4. **Smoking Test**: Download the RC binary and verify it can synchronize with mainnet
   ```bash
   ckb run
   ```

5. **Publish RC Release**: Publish the RC version on GitHub
   - Edit the release title to match `ckb --version` output
   - Add release notes summarizing changes and improvements

6. **Deploy and Test**: Deploy RC to testnet and run comprehensive regression tests

⚠️ **Important**: Proceed to final release only after no critical bugs are found for at least **one week** during RC testing. If issues are discovered, create a new RC with fixes and restart the testing period.

#### Phase 3: Release

The final release phase involves creating the production release, publishing artifacts, and updating the main branches.

1. **Bump Final Version**: Bump to the final release version (remove RC suffix) in the RC branch.
   ```bash
   devtools/release/bump.sh ${major}.${minor}.${patch}
   ```

2. **Update CHANGELOG**: Edit CHANGELOG to consolidate all RC versions into a single final release entry, then commit the changes

3. **Push to pkg/ Branch**: Push to the packaging branch to trigger the final build
   ```bash
   devtools/release/release-pkg.sh push-pkg
   ```
   This creates a branch `pkg/v{major}.{minor}.{patch}` which automatically triggers `.github/workflows/package.yaml`

4. **Wait for Packaging**: Wait for the packaging CI to complete and verify all platform builds succeed

5. **Smoking Test**: Perform a final verification that the release binary works correctly

6. **Create and Push Tag**: Create the signed release tag and push it to the repository
   ```bash
   devtools/release/release-pkg.sh tag
   devtools/release/release-pkg.sh push-tag
   ```
   The tag automatically triggers `.github/workflows/publish-root-ckb-crate.yaml` to publish the root crate to crates.io

7. **Publish Release**: Publish the final version on GitHub
   - Edit the release title to match `ckb --version` output
   - Add comprehensive release notes highlighting key features and changes

8. **Merge Back**: Merge the RC branch back to `develop` and `master` to synchronize the release
   ```bash
   git checkout develop
   git merge rc/v{major}.{minor}.x
   git push upstream develop

   git checkout master
   git merge rc/v{major}.{minor}.x
   git push upstream master
   ```

9. **Notify Stakeholders**: Notify Discord channels and deploy the new version to production

### Patch Release

Patch releases are typically used for hotfixes that need to be deployed quickly to address critical bugs or security issues.

The patch release process involves:
- Merging hotfix changes into **all affected RC branches**
- Only the **latest RC branch** is merged back into `master` and `develop`
- Patch releases may publish **RC versions for preview** before the final release

#### Process

1. **Identify Hotfix**: Determine the issue that requires a patch release

2. **Apply Fix**: Make the necessary changes to fix the issue

3. **Merge to All Affected RC Branches**: Merge the hotfix into all affected RC branches
   ```bash
   # For each affected RC branch
   git checkout rc/v{major}.{minor}.x
   git merge hotfix/v{major}.{minor}.{patch}
   git push upstream rc/v{major}.{minor}.x
   ```

4. **Optional: Publish RC Version for Preview**: If testing is needed before final release, publish an RC version
   - **Bump Version for RC**: Bump version with RC suffix
     ```bash
     devtools/release/bump.sh ${major}.${minor}.${patch}-rc${rc_number}
     ```
   - **Push to pkg/ Branch**: Push to packaging branch
     ```bash
     devtools/release/release-pkg.sh push-pkg
     ```
   - **Wait for Packaging**: Wait for packaging CI to complete
   - **Publish RC Release**: Publish the RC version on GitHub for preview and testing
   - **Test**: Perform testing and validation
   - If issues are found, fix and create a new RC; otherwise proceed to final release

5. **Bump Final Version**: Bump to the final patch version (remove RC suffix if RC was published)
   ```bash
   devtools/release/bump.sh ${major}.${minor}.${patch}
   ```

6. **Update CHANGELOG**: Edit CHANGELOG to consolidate RC versions (if any) into the final release entry, then commit

7. **Push to pkg/ Branch**: Push to packaging branch for the final release
   ```bash
   devtools/release/release-pkg.sh push-pkg
   ```

8. **Wait for Packaging**: Wait for packaging CI to complete

9. **Smoking Test**: Verify the patch release binary works correctly

10. **Create and Push Tag**: Create the release tag and push it
    ```bash
    devtools/release/release-pkg.sh tag
    devtools/release/release-pkg.sh push-tag
    ```

11. **Publish Release**: Publish the final version on GitHub
    - Edit the release title to match `ckb --version` output
    - Add release notes explaining the hotfix

12. **Merge Back**: Merge **only the latest RC branch** back to `develop` and `master`
    ```bash
    git checkout develop
    git merge rc/v{major}.{minor}.x
    git push upstream develop

    git checkout master
    git merge rc/v{major}.{minor}.x
    git push upstream master
    ```

13. **Notify Stakeholders**: Notify Discord channels and deploy the patch release

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

- **`devtools/release/bump.sh`**: Bumps version and generate CHANGELOG for the root crate
- **`devtools/release/release-pkg.sh`**: Helper script for packaging workflow
  - `push-pkg`: Push current branch to `pkg/v{version}` branch
  - `tag`: Create a signed release tag
  - `push-tag`: Push the release tag
- **`devtools/release/update_default_valid_target.sh`**: Update assume valid target

## Best Practices

1. **Version Bumping**: Always ensure version bumps follow semantic versioning
2. **Breaking Changes**: Manually verify and bump versions for breaking changes in non-root crates
3. **Version Consistency**: Ensure the version in `Cargo.toml` matches the tag version before publishing
4. **RC Testing**: Always smoke test RC versions before final release
5. **Release Review**: Review draft releases and binary packages before publishing
6. **Security**: Review security advisories before creating RC branches
7. **Stakeholder Communication**: Notify relevant channels and create public issues for transparency
8. **Wait Period**: Release only after no bugs found in RC for at least a week
