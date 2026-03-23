(use-modules ((gnu packages bash) #:select (bash-minimal))
             ((gnu packages base) #:select (coreutils findutils grep sed tar glibc))
             ((gnu packages compression) #:select (gzip))
             (gnu packages commencement)
             ((gnu packages elf) #:select (patchelf))
             ((gnu packages gcc) #:select (gcc-14))
             ((gnu packages llvm) #:select (clang))
             ((gnu packages nss) #:select (nss-certs))
             ((gnu packages perl) #:select (perl))
             (gnu packages pkg-config)
             ((gnu packages python) #:select (python-minimal))
             ((gnu packages python-xyz) #:select (python-lief))
             ((gnu packages rust) #:select (rust-1.92))
             ((gnu packages sqlite) #:select (sqlite))
             ((gnu packages tls) #:select (openssl))
             ((gnu packages version-control) #:select (git-minimal))
             (gnu packages gawk)
             (guix download)
             (guix git-download)
             (guix packages)
             (guix profiles)
             ((guix utils) #:select (substitute-keyword-arguments)))

(define-syntax-rule (search-our-patches file-name ...)
  "Return the list of absolute file names corresponding to each
FILE-NAME found in ./patches relative to the current file."
  (parameterize
      ((%patch-path (list (string-append (dirname (current-filename)) "/patches"))))
    (list (search-patch file-name) ...)))

(define building-on
  (string-append "--build="
                 (list-ref (string-split (%current-system) #\-) 0)
                 "-guix-linux-gnu"))

(define make-gcc-toolchain
  (@@ (gnu packages commencement) make-gcc-toolchain))

(define (package-with-extra-patches original patches)
  "Return package ORIGINAL with all PATCHES appended to its list of patches."
  (package
    (inherit original)
    (source
     (origin
       (inherit (package-source original))
       (patches
        (append (origin-patches (package-source original))
                patches))))))

(define (toolchain-inputs package)
  (list (list (package-name package) package)))

(define linux-base-gcc
  (package
    (inherit (package-with-extra-patches
              gcc-14
              (search-our-patches "gcc-fixed-store-remap.patch"
                                  "gcc-ssa-generation.patch")))
    (arguments
     (substitute-keyword-arguments (package-arguments gcc-14)
       ((#:configure-flags flags)
        `(append ,flags
                 (list "--enable-initfini-array=yes"
                       "--enable-default-ssp=yes"
                       "--enable-default-pie=yes"
                       "--enable-host-bind-now=yes"
                       "--enable-standard-branch-protection=yes"
                       "--enable-cet=yes"
                       "--enable-gprofng=no"
                       "--disable-gcov"
                       "--disable-libgomp"
                       "--disable-libquadmath"
                       "--disable-libsanitizer"
                       ,building-on)))
       ((#:phases phases)
        `(modify-phases ,phases
           ;; Replace Guix's default runtime rpath injection with rpath-link
           ;; so release binaries do not inherit /gnu/store runtime paths.
           (add-after 'pre-configure 'replace-rpath-with-rpath-link
             (lambda _
               (substitute* (cons "gcc/config/rs6000/sysv4.h"
                                  (find-files "gcc/config"
                                              "^gnu-user.*\\.h$"))
                 (("-rpath=") "-rpath-link="))
               #t))))))))

(define-public glibc-2.31
  (let ((commit "7b27c450c34563a28e634cccb399cd415e71ebfe"))
    (package
      (inherit glibc)
      (version "2.31")
      (source
       (origin
         (method git-fetch)
         (uri (git-reference
               (url "https://sourceware.org/git/glibc.git")
               (commit commit)))
         (file-name (git-file-name "glibc" commit))
         (sha256
          (base32
           "017qdpr5id7ddb4lpkzj2li1abvw916m3fc6n7nw28z4h5qbv2n0"))
         (patches (search-our-patches "glibc-fixed-store-remap.patch"))))
      (arguments
       (substitute-keyword-arguments (package-arguments glibc)
         ((#:configure-flags flags)
          `(append ,flags
                   (list "--enable-stack-protector=all"
                         "--enable-cet"
                         "--enable-bind-now"
                         "--disable-werror"
                         "--disable-timezone-tools"
                         "--disable-profile"
                         ,building-on)))
         ((#:phases phases)
          `(modify-phases ,phases
             ;; glibc 2.31 still wants to install rpc metadata into /etc by
             ;; default, which fails in the Guix build environment.
             (add-before 'configure 'set-etc-rpc-installation-directory
               (lambda* (#:key outputs #:allow-other-keys)
                 (let ((out (assoc-ref outputs "out")))
                   (substitute* "sunrpc/Makefile"
                     (("^\\$\\(inst_sysconfdir\\)/rpc(.*)$" _ suffix)
                      (string-append out "/etc/rpc" suffix "\n"))
                     (("^install-others =.*$")
                      (string-append "install-others = " out "/etc/rpc\n")))
                   #t))))))))))

(define-public gcc-glibc-2.31-toolchain
  (package
    (inherit (make-gcc-toolchain linux-base-gcc glibc-2.31))
    (name "gcc-glibc-2.31-toolchain")))

(define-public rust-1.92-glibc-2.31
  (package
    (inherit (package-with-c-toolchain
              rust-1.92
              (toolchain-inputs gcc-glibc-2.31-toolchain)))
    (name "rust-1.92-glibc-2.31")))

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
       gcc-glibc-2.31-toolchain
       `(,gcc-glibc-2.31-toolchain "static")
       clang
       pkg-config
       perl
       python-minimal
       python-lief
       nss-certs
       openssl
       patchelf
       sqlite
       rust-1.92-glibc-2.31
       `(,rust-1.92-glibc-2.31 "cargo")))
