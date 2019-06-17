use includedir_codegen::Compression;
use numext_fixed_hash::H256;
use std::env;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use system_cells::CODE_HASH_SECP256K1_BLAKE160_SIGHASH_ALL;

fn main() {
    let mut bundled = includedir_codegen::start("BUNDLED");

    for f in &["ckb.toml", "ckb-miner.toml"] {
        bundled
            .add_file(f, Compression::Gzip)
            .expect("add files to resource bundle");
    }

    bundled.build("bundled.rs").expect("build resource bundle");

    let out_path = Path::new(&env::var("OUT_DIR").unwrap()).join("code_hashes.rs");
    let mut out_file = BufWriter::new(File::create(&out_path).expect("create code_hashes.rs"));

    write!(
        &mut out_file,
        "pub const CODE_HASH_SECP256K1_BLAKE160_SIGHASH_ALL: H256 = {:?};",
        H256(CODE_HASH_SECP256K1_BLAKE160_SIGHASH_ALL)
    )
    .expect("write to code_hashes.rs");
}
