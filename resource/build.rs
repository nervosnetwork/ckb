use includedir_codegen::Compression;
use walkdir::WalkDir;

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
}
