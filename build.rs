extern crate build_info;

fn main() {
    // Don't rebuild even if nothing changed
    println!("cargo:rerun-if-changed=build.rs");
    // forward git repo hashes we build at
    println!(
        "cargo:rustc-env=COMMIT_DESCRIBE={}",
        build_info::get_commit_describe().unwrap_or_default()
    );
    println!(
        "cargo:rustc-env=COMMIT_DATE={}",
        build_info::get_commit_date().unwrap_or_default()
    );
}
