//! Build script for crate `ckb-tx-pool`.

use molecule_codegen::{Compiler, Language};

fn compile_schema(schema: &str) {
    println!("cargo:rerun-if-changed={}", schema);
    let mut compiler = Compiler::new();
    compiler
        .input_schema_file(schema)
        .generate_code(Language::Rust)
        .output_dir_set_default()
        .run()
        .unwrap();
}

fn main() {
    compile_schema("schemas/persisted.mol");
}
