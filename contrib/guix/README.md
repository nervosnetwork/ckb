# Guix Release Builder

This directory now contains two Guix entry points with different goals:

- `guix-build`
  Builds a deterministic `x86_64-unknown-linux-gnu` CKB release tarball using a
  pinned Guix environment. It first vendors Rust crates into a deterministic
  source archive, then builds that archive inside `guix time-machine shell
  --container --pure`, following the same broad model used by Bitcoin Core.
- `guix.scm`
  Defines a Guix package recipe for building `ckb` as a Guix package.

## Release Tarball Build

From the top of a clean checkout:

```sh
./contrib/guix/guix-build
```

This flow:

- enters a pinned Guix shell defined by `manifest.scm`
- derives `VERSION` from an exact `HEAD` tag or a 12-character commit ID
- derives `SOURCE_DATE_EPOCH` from the `HEAD` commit timestamp, and rejects an
  inherited ambient value unless `FORCE_SOURCE_DATE_EPOCH=1` is set
- creates a source archive with `git archive`
- materializes Cargo dependencies from `Cargo.lock` via Guix's
  `cargo-inputs-from-lockfile` before entering the pure container, while
  bypassing Guix substitute servers for those crate source fetches
- builds `ckb` with deterministic environment settings
- stages an install tree
- emits a deterministic release tarball plus `SHA256SUMS`

The initial refactor only supports `HOSTS=x86_64-unknown-linux-gnu`.

Build outputs are written under:

```text
guix-build-<version>/output/x86_64-unknown-linux-gnu/
```

## Package Recipe

If you want the Guix-native package output instead of a conventional release
tarball, use:

```sh
guix time-machine -C contrib/guix/channels.scm -- build -f contrib/guix/guix.scm
```
