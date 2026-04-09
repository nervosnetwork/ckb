#!/usr/bin/env bash
export LC_ALL=C
set -euo pipefail

source_tree="${SOURCE_TREE:?not set}"
channels_file="${GUIX_CHANNELS_FILE:?not set}"
guix_bin="${GUIX_BIN:-guix}"
lockfile="${source_tree}/Cargo.lock"
vendor_dir="${source_tree}/guix-vendor"
scheme_file="$(mktemp)"
crate_paths_file="$(mktemp)"
crate_metadata_file="$(mktemp)"

cleanup() {
    rm -f "$scheme_file" "$crate_paths_file" "$crate_metadata_file"
}
trap cleanup EXIT

cat >"$scheme_file" <<EOF
(use-modules (guix import crate))
(cargo-inputs-from-lockfile "${lockfile}")
EOF

rm -rf "$vendor_dir" "${source_tree}/.cargo"
mkdir -p "$vendor_dir" "${source_tree}/.cargo"

"$guix_bin" time-machine -C "$channels_file" -- \
    build --no-substitutes -f "$scheme_file" > "$crate_paths_file"

while IFS= read -r crate_archive; do
    [[ -n "$crate_archive" ]] || continue
    crate_dir="${vendor_dir}/$(basename "$crate_archive")"
    crate_sha256="$(sha256sum "$crate_archive" | awk '{print $1}')"
    mkdir -p "$crate_dir"
    tar xf "$crate_archive" -C "$crate_dir" --strip-components 1
    printf '%s\t%s\n' "$crate_dir" "$crate_sha256" >> "$crate_metadata_file"
done < "$crate_paths_file"

"$guix_bin" time-machine -C "$channels_file" -- \
    repl -- /dev/stdin <<EOF
(use-modules (guix build cargo-utils))
(generate-all-checksums "${vendor_dir}")
EOF

while IFS=$'\t' read -r crate_dir crate_sha256; do
    sed -i \
        "s/\"package\":\"[^\"]*\"/\"package\":\"${crate_sha256}\"/" \
        "${crate_dir}/.cargo-checksum.json"
done < "$crate_metadata_file"

cat > "${source_tree}/.cargo/config.toml" <<'EOF'
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "guix-vendor"
EOF
