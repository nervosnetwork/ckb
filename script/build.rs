//! Build script for crate `ckb-script`.
use std::env;

fn main() {
    let target_family = env::var("CARGO_CFG_TARGET_FAMILY").unwrap_or_default();
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let is_windows = target_family == "windows";
    let is_unix = target_family == "unix";
    let is_x86_64 = target_arch == "x86_64";
    let can_enable_asm = is_x86_64 && (is_windows || is_unix);

    if cfg!(feature = "asm") && (!can_enable_asm) {
        panic!("asm feature can only be enabled on x86_64 Linux, macOS and Windows platforms!");
    }

    if cfg!(any(feature = "asm", feature = "detect-asm")) && can_enable_asm {
        println!("cargo:rustc-cfg=has_asm");
    }
}
