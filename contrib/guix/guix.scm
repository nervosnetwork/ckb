(use-modules (guix gexp)
             (guix git-download)
             (guix import crate)
             ((guix licenses) #:prefix license:)
             (guix packages)
             (guix utils)
             (guix build-system cargo)
             (gnu packages llvm)
             (gnu packages rust)
             (gnu packages perl)
             (gnu packages pkg-config)
             (gnu packages sqlite))

(define source-root
  (current-source-directory))

(define cargo-lockfile
  (string-append source-root "/../../Cargo.lock"))

(define commit
  "b75a785c6af54243fafc6a1be7a79239451c0565")

(package
  (name "ckb")
  ;; Upstream release tag v0.205.0.
  (version "0.205.0")
  (source
   (origin
     (method git-fetch)
     (uri (git-reference
           (url "https://github.com/nervosnetwork/ckb.git")
           (commit commit)))
     (file-name (git-file-name name version))
     (sha256
      (base32 "0sswg9r0br37qr2qa2p6n3cxl1japapp2hk6hc0kiyvclr59lknl"))))
  (build-system cargo-build-system)
  (arguments
   (list
    ;; CKB pins Rust 1.92.0 in rust-toolchain.toml.
    #:rust rust-1.92
    ;; CKB's test suite pulls in long-running integration tests and extra
    ;; services; keep the package focused on producing the node binary.
    #:tests? #f
    #:install-source? #f
    ;; Vendored Rust crates ship binary test fixtures (.dll, .der, etc.)
    ;; that trigger this audit phase but are not used to build the node.
    #:phases #~(modify-phases %standard-phases
                 (add-after 'unpack 'set-reproducible-rustflags
                   (lambda _
                     ;; Strip the ephemeral Guix build root from compiler-
                     ;; generated paths so panic locations and debug metadata
                     ;; do not depend on /tmp/guix-build-... prefixes.
                     (let* ((source (getcwd))
                            (remap (string-append
                                    "--remap-path-prefix=" source "=."))
                            (existing (or (getenv "RUSTFLAGS") "")))
                       (setenv "RUSTFLAGS"
                               (if (string-null? existing)
                                   remap
                                   (string-append remap " " existing))))))
                 (delete 'check-for-pregenerated-files))
    #:cargo-build-flags ''("--release" "--bin" "ckb")
    #:cargo-install-paths ''(".")))
  (native-inputs
   (list clang
         perl
         pkgconf))
  ;; New Rust packaging model: use the cargo inputs generated from the
  ;; release lockfile directly instead of hand-maintaining crate package
  ;; definitions in a separate generated file.
  (inputs
   (append (list sqlite)
           (cargo-inputs-from-lockfile cargo-lockfile)))
  (home-page "https://github.com/nervosnetwork/ckb")
  (synopsis "Nervos CKB node")
  (description
   "CKB is the layer 1 of Nervos Network, a public and permissionless
blockchain.  This package builds the @command{ckb} node binary from the Cargo
workspace using Guix's lockfile-based Rust packaging model.")
  (license license:expat))
