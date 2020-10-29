//! Build script for crate `ckb-script`.
use std::env;

fn main() {
    let target_pointer_width = env::var("CARGO_CFG_TARGET_POINTER_WIDTH").unwrap();
    let target_family = env::var("CARGO_CFG_TARGET_FAMILY").unwrap_or_default();
    let is_windows = target_family == "windows";
    let is_unix = target_family == "unix";
    let can_enable_asm = (target_pointer_width == "64") && (is_windows || is_unix);

    if cfg!(feature = "asm") && (!can_enable_asm) {
        panic!("asm feature can only be enabled on 64-bit Linux, macOS and Windows platforms!");
    }

    if cfg!(any(feature = "asm", feature = "detect-asm")) && can_enable_asm {
        println!("cargo:rustc-cfg=has_asm");
    }
}
