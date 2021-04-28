//! Build script for the binary crate `ckb`.
use std::path::Path;

fn rerun_if_changed(path_str: &str) -> bool {
    let path = Path::new(path_str);

    if path.starts_with("benches")
        || path.starts_with("devtools")
        || path.starts_with("docker")
        || path.starts_with("docs")
        || path.starts_with("test")
        || path.starts_with(".github")
    {
        return false;
    }

    !matches!(
        path_str,
        "COPYING" | "Makefile" | "clippy.toml" | "rustfmt.toml" | "rust-toolchain"
    )
}

#[allow(clippy::manual_strip)]
fn main() {
    let files_stdout = std::process::Command::new("git")
        .args(&["ls-tree", "-r", "--name-only", "HEAD"])
        .output()
        .ok()
        .and_then(|r| String::from_utf8(r.stdout).ok());

    if files_stdout.is_some() {
        println!(
            "cargo:rustc-env=COMMIT_DESCRIBE={}",
            ckb_build_info::get_commit_describe().unwrap_or_default()
        );
        println!(
            "cargo:rustc-env=COMMIT_DATE={}",
            ckb_build_info::get_commit_date().unwrap_or_default()
        );

        println!("cargo:rerun-if-changed=build.rs");
        println!("cargo:rerun-if-changed=.git/HEAD");

        let head = std::fs::read_to_string(".git/HEAD").unwrap_or_default();
        if head.starts_with("ref: ") {
            let path_str = format!(".git/{}", head[5..].trim());
            let path = Path::new(&path_str);
            if path.exists() {
                println!("cargo:rerun-if-changed={}", path_str);
            }
        }
    }

    for file in files_stdout.iter().flat_map(|stdout| stdout.lines()) {
        if rerun_if_changed(file) {
            println!("cargo:rerun-if-changed={}", file);
        }
    }
}
