;; Derived from Bitcoin Core's Guix manifest for reproducible builds.
;; Reference: https://github.com/bitcoin/bitcoin/blob/master/contrib/guix/manifest.scm
;;
;; Differences from Bitcoin: uses Rust/Cargo instead of C++/CMake, includes
;; a cross-compiled OpenSSL package, and does not target Windows or macOS.

(use-modules ((gnu packages bash) #:select (bash-minimal))
             ((gnu packages base) #:select (coreutils findutils gnu-make grep sed tar glibc))
             ((gnu packages compression) #:select (gzip))
             (gnu packages commencement)
             (gnu packages cross-base)
             ((gnu packages elf) #:select (patchelf))
             ((gnu packages gcc) #:select (gcc-14))
             ((gnu packages linux) #:select (linux-libre-headers-6.1))
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
             (guix build-system trivial)
             (guix download)
             (guix gexp)
             (guix git-download)
             (guix packages)
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
       ((#:phases phases #~%standard-phases)
        #~(modify-phases #$phases
            ;; Replace Guix's default runtime rpath injection with rpath-link
            ;; so release binaries do not inherit /gnu/store runtime paths.
            (add-after 'pre-configure 'replace-rpath-with-rpath-link
              (lambda _
                (substitute* (cons "gcc/config/rs6000/sysv4.h"
                                   (find-files "gcc/config"
                                               "^gnu-user.*\\.h$"))
                  (("-rpath=") "-rpath-link="))
                #t))))))))

(define (make-cross-toolchain target
                              base-gcc-for-libc
                              base-kernel-headers
                              base-libc
                              base-gcc)
  "Create a cross-compilation toolchain package for TARGET."
  (let* ((xbinutils (cross-binutils target))
         (xgcc-sans-libc (cross-gcc target
                                    #:xgcc base-gcc-for-libc
                                    #:xbinutils xbinutils))
         (xkernel (cross-kernel-headers target
                                        #:linux-headers base-kernel-headers
                                        #:xgcc xgcc-sans-libc
                                        #:xbinutils xbinutils))
         (xlibc (cross-libc target
                            #:libc base-libc
                            #:xgcc xgcc-sans-libc
                            #:xbinutils xbinutils
                            #:xheaders xkernel))
         (xgcc (cross-gcc target
                          #:xgcc base-gcc
                          #:xbinutils xbinutils
                          #:libc xlibc)))
    (package
      (name (string-append target "-toolchain"))
      (version (package-version xgcc))
      (source #f)
      (build-system trivial-build-system)
      (arguments '(#:builder (begin (mkdir %output) #t)))
      (propagated-inputs
       (list xbinutils
             xlibc
             xgcc
             `(,xlibc "static")
             `(,xgcc "lib")))
      (synopsis (string-append "Complete GCC tool chain for " target))
      (description (string-append "This package provides a complete GCC tool "
                                  "chain for " target " development."))
      (home-page (package-home-page xgcc))
      (license (package-license xgcc)))))

(define base-linux-kernel-headers linux-libre-headers-6.1)

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
                         ;; Newer libstdc++ in the host compiler stack is
                         ;; linked against newer glibc symbols, so older glibc
                         ;; must pretend C++ linking is unavailable.
                         "libc_cv_cxx_link_ok=no"
                         "--disable-timezone-tools"
                         "--disable-profile"
                         ,building-on)))
         ((#:phases phases)
         `(modify-phases ,phases
             ;; Old glibc releases do not install C.UTF-8 cleanly in Guix's
             ;; locale phase; Guix already carries the same workaround for
             ;; glibc-2.33 and glibc-2.35.
             (delete 'install-utf8-c-locale)
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

(define* (make-ckb-cross-toolchain target
                                   #:key
                                   (base-gcc-for-libc linux-base-gcc)
                                   (base-kernel-headers base-linux-kernel-headers)
                                   (base-libc glibc-2.31)
                                   (base-gcc linux-base-gcc))
  "Return the staged old-glibc cross toolchain used for CKB Linux releases."
  (make-cross-toolchain target
                        base-gcc-for-libc
                        base-kernel-headers
                        base-libc
                        base-gcc))

(define-public x86_64-linux-gnu-toolchain
  (make-ckb-cross-toolchain "x86_64-linux-gnu"))

;; Cross-compile OpenSSL using our existing x86_64-linux-gnu cross-toolchain
;; (which targets glibc-2.31).  The Guix-packaged openssl is compiled against
;; native glibc (2.39) and cannot be linked into the CKB release binary.
;; Instead of rebuilding the entire GCC toolchain natively (which has bootstrap
;; issues), we cross-compile OpenSSL with the same cross-gcc that builds CKB.
;; The cross-kernel-headers are needed because glibc headers reference
;; linux/limits.h and friends, but the cross-gcc sysroot doesn't include
;; them as a propagated output.
(define x86_64-linux-gnu-kernel-headers
  (cross-kernel-headers "x86_64-linux-gnu"
                        #:linux-headers base-linux-kernel-headers
                        #:xgcc (cross-gcc "x86_64-linux-gnu"
                                          #:xgcc linux-base-gcc
                                          #:xbinutils (cross-binutils "x86_64-linux-gnu"))
                        #:xbinutils (cross-binutils "x86_64-linux-gnu")))

(define-public openssl-glibc-2.31
  (package
    (inherit openssl)
    (name "openssl-glibc-2.31")
    (native-inputs
     `(("cross-toolchain" ,x86_64-linux-gnu-toolchain)
       ("cross-kernel-headers" ,x86_64-linux-gnu-kernel-headers)
       ("perl" ,perl)))
    (arguments
     (list
      #:tests? #f
      #:configure-flags #~'()
      #:phases
      #~(modify-phases %standard-phases
          (replace 'configure
            (lambda* (#:key outputs #:allow-other-keys)
              (let ((out (assoc-ref outputs "out")))
                (invoke "perl" "./Configure"
                        "linux-x86_64"
                        (string-append "--prefix=" out)
                        "--cross-compile-prefix=x86_64-linux-gnu-"
                        "shared"
                        "no-tests"))))
          ;; Inject kernel headers into the Makefile since OpenSSL's
          ;; Configure doesn't pass through CFLAGS or CPATH.
          ;; Override build to pass kernel headers via CPPFLAGS to make.
          (replace 'build
            (lambda* (#:key inputs parallel-build? #:allow-other-keys)
              (let* ((kernel-headers
                      (assoc-ref inputs "cross-kernel-headers"))
                     (jobs (if parallel-build?
                               (number->string (parallel-job-count))
                               "1")))
                (apply invoke "make" "-j" jobs
                       (if kernel-headers
                           (list (string-append
                                  "CPPFLAGS=-I" kernel-headers "/include"))
                           '())))))
          (delete 'check)
          ;; Skip RUNPATH validation — the cross-compiled .so files have
          ;; RUNPATH pointing to the cross-glibc, not the output directory.
          ;; This is expected; the final binary handles RUNPATH via patchelf.
          (delete 'validate-runpath))))))

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
       gcc-toolchain-14
       `(,gcc-toolchain-14 "static")
       x86_64-linux-gnu-toolchain
       clang
       pkg-config
       perl
       python-minimal
       python-lief
       nss-certs
       openssl-glibc-2.31
       patchelf
       sqlite
       rust-1.92
       `(,rust-1.92 "cargo")))
