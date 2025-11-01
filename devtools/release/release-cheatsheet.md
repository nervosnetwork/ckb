# CKB Release Cheatsheet

Switch to stable rust since release tools do not support rust 1.85.0

```
rustup override set stable
```

Install tools

```
cargo binstall cargo-release cargo-smart-release cargo-workspaces
```

Check which crates should be published

```
cargo workspaces changed
```

Bump the version and release (dry run)

```
cargo smart-release ...
```
