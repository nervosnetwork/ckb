(use-modules ((gnu packages bash) #:select (bash-minimal))
             ((gnu packages base) #:select (coreutils findutils grep sed tar))
             ((gnu packages compression) #:select (gzip))
             (gnu packages commencement)
             ((gnu packages llvm) #:select (clang))
             ((gnu packages nss) #:select (nss-certs))
             ((gnu packages perl) #:select (perl))
             (gnu packages pkg-config)
             ((gnu packages python) #:select (python-minimal))
             ((gnu packages rust) #:select (rust-1.92))
             ((gnu packages elf) #:select (patchelf))
             ((gnu packages sqlite) #:select (sqlite))
             ((gnu packages tls) #:select (openssl))
             ((gnu packages version-control) #:select (git-minimal))
             (gnu packages gawk)
             (guix profiles))

(packages->manifest
 (list bash-minimal
       coreutils
       findutils
       gawk
       grep
       gnu-make
       sed
       tar
       gzip
       git-minimal
       gcc-toolchain
       clang
       pkg-config
       perl
       python-minimal
       nss-certs
       openssl
       patchelf
       sqlite
       rust-1.92
       `(,rust-1.92 "cargo")))
