#![allow(missing_docs)]

use std::env;
use std::path::Path;

fn main() {
    // Check if working directory is in test/ by looking for template/ckb.toml
    if !Path::new("template/ckb.toml").exists() {
        // Try test/template/ckb.toml
        if Path::new("test/template/ckb.toml").exists() {
            env::set_current_dir("test").expect("Failed to change directory to test/");
        } else {
            eprintln!("Error: Cannot find template/ckb.toml in current or test/ directory");
            std::process::exit(1);
        }
    }

    ckb_test::main_test();
}
