#!/bin/bash
set -eu

# Script to strip [dev-dependencies] sections from all Cargo.toml files
# This is used before release-plz to ensure dev-dependencies are not included in releases

# Find all Cargo.toml files, excluding those in target directory
find . -path "*/target" -prune -o -name "Cargo.toml" -type f -print | while read -r cargo_toml; do
  # Skip if file doesn't exist (shouldn't happen, but be safe)
  if [ ! -f "$cargo_toml" ]; then
    continue
  fi

  # Check if file contains [dev-dependencies]
  if grep -q "^\[dev-dependencies\]" "$cargo_toml"; then
    echo "Stripping dev-dependencies from $cargo_toml"

    # Use awk to remove the [dev-dependencies] section
    # This removes everything from [dev-dependencies] until the next [ section or end of file
    awk '
        BEGIN { in_dev_deps = 0 }
        /^\[dev-dependencies\]/ { in_dev_deps = 1; next }
        /^\[/ && in_dev_deps { in_dev_deps = 0; print; next }
        !in_dev_deps { print }
        ' "$cargo_toml" >"$cargo_toml.tmp" && mv "$cargo_toml.tmp" "$cargo_toml"
  fi
done

echo "Finished stripping dev-dependencies from all Cargo.toml files"
