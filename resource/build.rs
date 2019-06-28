use includedir_codegen::Compression;
use numext_fixed_hash::H256;
use std::env;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use walkdir::WalkDir;

use ckb_system_scripts::CODE_HASH_DAO;
use ckb_system_scripts::CODE_HASH_SECP256K1_BLAKE160_SIGHASH_ALL;

fn main() {
    let mut bundled = includedir_codegen::start("BUNDLED");

    for f in &["ckb.toml", "ckb-miner.toml"] {
        bundled
            .add_file(f, Compression::Gzip)
            .expect("add files to resource bundle");
    }

    for entry in WalkDir::new("specs").follow_links(true).into_iter() {
        match entry {
            Ok(ref e)
                if !e.file_type().is_dir() && !e.file_name().to_string_lossy().starts_with(".") =>
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

    write!(
        &mut out_file,
        "pub const CODE_HASH_SECP256K1_BLAKE160_SIGHASH_ALL: H256 = {:?};\n",
        H256(CODE_HASH_SECP256K1_BLAKE160_SIGHASH_ALL)
    )
    .expect("write to code_hashes.rs");

    write!(
        &mut out_file,
        "pub const CODE_HASH_DAO: H256 = {:?};\n",
        H256(CODE_HASH_DAO)
    )
    .expect("write to code_hashes.rs");
}
