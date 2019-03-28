use includedir_codegen::Compression;

fn main() {
    includedir_codegen::start("FILES")
        .dir("chainspecs", Compression::Gzip)
        .build("chainspecs.rs")
        .unwrap();
}
