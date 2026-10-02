{
  description = "CKB cross-compilation environment for aarch64-linux-android";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
          config.allowUnfree = true;
          config.android_sdk.accept_license = true;
        };

        rustToolchain = pkgs.rust-bin.stable."1.92.0".default.override {
          extensions = [ "rust-src" ];
          targets = [ "aarch64-linux-android" ];
        };

        # NDK cross toolchain (clang) for aarch64 Android.
        androidPkgs = pkgs.pkgsCross.aarch64-android-prebuilt;
        ndkCC = androidPkgs.stdenv.cc;
        ndkBintools = androidPkgs.stdenv.cc.bintools.bintools;
        ndkPrefix = ndkCC.targetPrefix; # e.g. "aarch64-unknown-linux-android-"
        sysroot = "${ndkCC}/sysroot";

      in
      {
        devShells.default = pkgs.mkShell {
          nativeBuildInputs = [
            rustToolchain
            ndkCC
            pkgs.pkg-config
            pkgs.cmake
            pkgs.perl
            pkgs.python3
            pkgs.llvmPackages.libclang
            pkgs.git
          ];

          shellHook = ''
            # The pkgsCross devShell pollutes CC/CXX/LD/AR/RANLIB with the NDK
            # cross tools.  Force them back to host tools for HOST_* (used by
            # cc-rs for build-script compilation) and use the NDK tools for
            # CC/CXX so that rocksdb's `build_detect_platform` shell script
            # probes the *target* compiler when checking for SSE / etc.
            export HOST_CC=gcc
            export HOST_CXX=g++
            export HOST_AR=ar
            export HOST_LD=ld
            export HOST_RANLIB=ranlib
            export CC=${ndkCC}/bin/${ndkPrefix}cc
            export CXX=${ndkCC}/bin/${ndkPrefix}c++
            export LD=${ndkCC}/bin/${ndkPrefix}cc
            export AR=${ndkBintools}/bin/${ndkPrefix}ar
            export RANLIB=${ndkBintools}/bin/${ndkPrefix}ranlib
            export STRIP=${ndkBintools}/bin/${ndkPrefix}strip

            export NDK_PREFIX=${ndkPrefix}
            export NDK_BIN=${ndkCC}/bin
            export NDK_SYSROOT=${sysroot}

            export CC_aarch64_linux_android=${ndkCC}/bin/${ndkPrefix}cc
            export CXX_aarch64_linux_android=${ndkCC}/bin/${ndkPrefix}c++
            export AR_aarch64_linux_android=${ndkBintools}/bin/${ndkPrefix}ar
            export RANLIB_aarch64_linux_android=${ndkBintools}/bin/${ndkPrefix}ranlib
            export STRIP_aarch64_linux_android=${ndkBintools}/bin/${ndkPrefix}strip

            export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=${ndkCC}/bin/${ndkPrefix}cc
            export CARGO_TARGET_AARCH64_LINUX_ANDROID_AR=${ndkBintools}/bin/${ndkPrefix}ar

            # Force the host (build-scripts) target to use the system gcc as
            # linker; otherwise rust-overlay's rustc picks up `clang` from PATH
            # which is the NDK clang and lacks Linux glibc CRT files.
            export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc

            # bindgen needs to find Android headers; libclang must be findable.
            # Use NDK's bundled libclang (clang 18) to match the NDK clang
            # binary; nixpkgs' default libclang is a different major version
            # which can cause header lookups to fail.
            export LIBCLANG_PATH=${ndkBintools}/toolchain/musl/lib
            BINDGEN_FLAGS="--sysroot=${ndkBintools}/sysroot --target=aarch64-linux-android -D__ANDROID_API__=35 -I${ndkBintools}/sysroot/usr/include -I${ndkBintools}/sysroot/usr/include/aarch64-linux-android"
            export BINDGEN_EXTRA_CLANG_ARGS_aarch64_linux_android="$BINDGEN_FLAGS"
            # Older bindgen versions don't honour the per-target form.
            export BINDGEN_EXTRA_CLANG_ARGS="$BINDGEN_FLAGS"

            # Some build scripts (jemalloc, secp256k1) inspect generic CC/AR.
            export TARGET_CC=${ndkCC}/bin/${ndkPrefix}cc
            export TARGET_CXX=${ndkCC}/bin/${ndkPrefix}c++
            export TARGET_AR=${ndkBintools}/bin/${ndkPrefix}ar
            export TARGET_RANLIB=${ndkBintools}/bin/${ndkPrefix}ranlib

            # Hints for rocksdb's `build_detect_platform` shell script so it
            # uses the cross arch/OS branch and skips x86 SSE flags.
            export TARGET_OS=OS_ANDROID_CROSSCOMPILE
            export TARGET_ARCHITECTURE=aarch64

            # OpenSSL vendored requires perl already provided.
            unset OPENSSL_DIR OPENSSL_LIB_DIR OPENSSL_INCLUDE_DIR

            # NDK 23+ removed libgcc. Rustc still passes `-lgcc` when targeting
            # *-linux-android, so create a stub libgcc.a that redirects to
            # libunwind.  Place it on a per-shell directory and add it to the
            # Android linker search path.
            export NDK_GCC_STUB="$PWD/.android-libgcc-stub"
            mkdir -p "$NDK_GCC_STUB"
            if [ ! -f "$NDK_GCC_STUB/libgcc.a" ]; then
              echo 'INPUT(-lunwind)' > "$NDK_GCC_STUB/libgcc.a"
            fi
            export CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS="-L $NDK_GCC_STUB"

            echo "CKB Android cross-compile shell ready."
            echo "  NDK prefix : $NDK_PREFIX"
            echo "  sysroot    : $NDK_SYSROOT"
            echo "Run: cargo build --release --target aarch64-linux-android"
          '';
        };
      });
}
