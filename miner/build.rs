fn main() {
    cc::Build::new()
        .file("src/worker/cuckoo.c")
        .include("src/worker")
        .flag("-O3")
        // .flag("-march=native")
        .static_flag(true)
        .compile("libmean.a");
}
