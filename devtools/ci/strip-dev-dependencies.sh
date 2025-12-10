#!/bin/bash
set -eu

# Script to strip specific dependencies from all Cargo.toml files
# This is used before release-plz to ensure certain dependencies are not included in releases

# Denylist of dependencies to remove
# Add more dependencies here as needed (match by the dependency declaration pattern)
DENYLIST=(
  "ckb-test-chain-utils"
)

# Build the pattern for awk to match any denylisted dependency
# Matches: dep.workspace, dep = {, etc.
dep_pattern=""
for dep in "${DENYLIST[@]}"; do
  if [ -z "$dep_pattern" ]; then
    dep_pattern="^[[:space:]]*${dep}[.]workspace|^[[:space:]]*${dep}[[:space:]]*="
  else
    dep_pattern="${dep_pattern}|^[[:space:]]*${dep}[.]workspace|^[[:space:]]*${dep}[[:space:]]*="
  fi
done

# Find all Cargo.toml files, excluding those in target directory
find . -path "*/target" -prune -o -name "Cargo.toml" -type f -print | while read -r cargo_toml; do
  echo "Processing $cargo_toml"

  awk -v pattern="$dep_pattern" '
    $0 ~ pattern {
      # Skip this line (it matches a denylisted dependency)
      next
    }
    { print }
  ' "$cargo_toml" >"$cargo_toml.tmp" && mv "$cargo_toml.tmp" "$cargo_toml"
done

echo "Finished stripping denylisted dependencies from all Cargo.toml files"
