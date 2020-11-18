//! Build script for crate `ckb-resource` to bundle the resources.
use ckb_types::H256;
use includedir_codegen::Compression;
use std::env;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use walkdir::WalkDir;

use ckb_system_scripts::{
    CODE_HASH_DAO, CODE_HASH_SECP256K1_BLAKE160_MULTISIG_ALL,
    CODE_HASH_SECP256K1_BLAKE160_SIGHASH_ALL, CODE_HASH_SECP256K1_DATA,
};

fn main() {
    let mut bundled = includedir_codegen::start("BUNDLED");

    for f in &["ckb.toml", "ckb-miner.toml", "default.db-options"] {
        bundled
            .add_file(f, Compression::Gzip)
            .expect("add files to resource bundle");
    }

    for entry in WalkDir::new("specs").follow_links(true).into_iter() {
        match entry {
            Ok(ref e)
                if !e.file_name().to_string_lossy().starts_with('.')
                    && e.file_name().to_string_lossy().ends_with(".toml") =>
            {
                bundled
                    .add_file(e.path(), Compression::Gzip)
                    .expect("add files to resource bundle");
            }
            _ => (),
        }
    }

    bundled.build("bundled.rs").expect("build resource bundle");

    let out_path = Path::new(&env::var("OUT_DIR").unwrap()).join("code_hashes.rs");
    let mut out_file = BufWriter::new(File::create(&out_path).expect("create code_hashes.rs"));

    writeln!(
        &mut out_file,
        "/// Data hash of the cell containing secp256k1 data.\n\
        pub const CODE_HASH_SECP256K1_DATA: H256 = {:?};",
        H256(CODE_HASH_SECP256K1_DATA)
    )
    .expect("write to code_hashes.rs");

    writeln!(
        &mut out_file,
        "/// Data hash of the cell containing secp256k1 blake160 sighash all lock script.\n\
        pub const CODE_HASH_SECP256K1_BLAKE160_SIGHASH_ALL: H256 = {:?};",
        H256(CODE_HASH_SECP256K1_BLAKE160_SIGHASH_ALL)
    )
    .expect("write to code_hashes.rs");

    writeln!(
        &mut out_file,
        "/// Data hash of the cell containing secp256k1 blake160 multisig all lock script.\n\
        pub const CODE_HASH_SECP256K1_BLAKE160_MULTISIG_ALL: H256 = {:?};",
        H256(CODE_HASH_SECP256K1_BLAKE160_MULTISIG_ALL)
    )
    .expect("write to code_hashes.rs");

    writeln!(
        &mut out_file,
        "/// Data hash of the cell containing DAO type script.\n\
        pub const CODE_HASH_DAO: H256 = {:?};",
        H256(CODE_HASH_DAO)
    )
    .expect("write to code_hashes.rs");
}
