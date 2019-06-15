use hash::new_blake2b;
use includedir_codegen::Compression;
use numext_fixed_hash::H256;
use std::collections::BTreeMap;
use std::env;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::Path;
use walkdir::WalkDir;

const BUF_SIZE: usize = 8 * 1024;

fn main() {
    let cells = Some(OsStr::new("cells"));
    let mut buf = [0u8; BUF_SIZE];
    let mut code_hashes = BTreeMap::new();
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

                if e.path().parent().and_then(|p| p.file_name()) == cells {
                    let mut blake2b = new_blake2b();
                    let mut fd = File::open(e.path()).expect("open file");
                    loop {
                        let read_bytes = fd.read(&mut buf).expect("read file");
                        if read_bytes > 0 {
                            blake2b.update(&buf[..read_bytes]);
                        } else {
                            break;
                        }
                    }
                    let code_hash = {
                        let mut result = [0u8; 32];
                        blake2b.finalize(&mut result);
                        H256::from_slice(&result).unwrap()
                    };

                    code_hashes.insert(
                        format!(
                            "CODE_HASH_{}",
                            e.file_name().to_string_lossy().to_owned().to_uppercase()
                        ),
                        code_hash,
                    );
                }
            }
            _ => (),
        }
    }

    bundled.build("bundled.rs").expect("build resource bundle");

    let out_path = Path::new(&env::var("OUT_DIR").unwrap()).join("code_hashes.rs");
    let mut out_file = BufWriter::new(File::create(&out_path).expect("create code_hashes.rs"));

    for (name, hash) in code_hashes {
        write!(&mut out_file, "pub const {}: H256 = {:?};", name, hash)
            .expect("write to code_hashes.rs");
    }
}
