//! Build script for crate `ckb-script`.
use std::env;

fn main() {
    let target_family = env::var("CARGO_CFG_TARGET_FAMILY").unwrap_or_default();
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let is_windows = target_family == "windows";
    let is_unix = target_family == "unix";
    let is_x86_64 = target_arch == "x86_64";
    let is_aarch64 = target_arch == "aarch64";
    let x64_asm = is_x86_64 && (is_windows || is_unix);
    let aarch64_asm = is_aarch64 && is_unix;
    let can_enable_asm = x64_asm || aarch64_asm;

    if cfg!(feature = "asm") && (!can_enable_asm) {
        panic!(
            "ASM feature is not available for target {} on {}!",
            target_arch, target_family
        );
    }

    if cfg!(any(feature = "asm", feature = "detect-asm")) && can_enable_asm {
        println!("cargo:rustc-cfg=has_asm");
    }
}
