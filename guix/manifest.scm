(use-modules (guix channels))

(specifications->manifest (list
  "rust"
  "llvm"
  "gcc-toolchain"
  "perl"
  "make"
  "clang-toolchain@16.0.6"
  "rust-pkg-config"
  "pkg-config"))
