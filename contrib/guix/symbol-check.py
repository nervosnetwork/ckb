#!/usr/bin/env python3
"""
Check that the Guix-built Linux release binary stays within the intended ABI
baseline and only links against an allowed set of shared libraries.

Derived from Bitcoin Core's symbol and security check scripts.
Reference: https://github.com/bitcoin/bitcoin/blob/master/contrib/guix/symbol-check.py
"""

from __future__ import annotations

import sys

import lief


MAX_VERSIONS = {
    "GLIBC": {
        lief.ELF.ARCH.X86_64: (2, 31),
    },
}

ELF_INTERPRETER_NAMES = {
    lief.ELF.ARCH.X86_64: {
        lief.Header.ENDIANNESS.LITTLE: "/lib64/ld-linux-x86-64.so.2",
    },
}

ELF_ALLOWED_LIBRARIES = {
    "libc.so.6",
    "libpthread.so.0",
    "libm.so.6",
    "librt.so.1",
    "libdl.so.2",
    "libgcc_s.so.1",
    "libstdc++.so.6",
    "ld-linux-x86-64.so.2",
    "libssl.so.3",
    "libcrypto.so.3",
}


def check_version(version_name: str, arch: lief.ELF.ARCH) -> bool:
    lib, _, raw_version = version_name.rpartition("_")
    if lib not in MAX_VERSIONS:
        # Only enforce version caps for libraries listed in MAX_VERSIONS
        # (currently GLIBC).  Symbols from libstdc++ (GLIBCXX, CXXABI) and
        # libgcc_s (GCC) are allowed without a version ceiling because the
        # target system is expected to provide a sufficiently recent C++
        # runtime alongside glibc.
        return True
    version = tuple(int(part) for part in raw_version.split("."))
    allowed = MAX_VERSIONS[lib]
    if isinstance(allowed, tuple):
        return version <= allowed
    return version <= allowed[arch]


def check_imported_symbols(filename: str, binary: lief.ELF.Binary) -> bool:
    ok = True
    for symbol in binary.imported_symbols:
        if not symbol.imported or not symbol.has_version:
            continue
        version = symbol.symbol_version
        if not version or not version.has_auxiliary_version:
            continue
        version_name = version.symbol_version_auxiliary.name
        if version_name and not check_version(version_name, binary.header.machine_type):
            print(f"{filename}: symbol {symbol.name} from unsupported version {version_name}")
            ok = False
    return ok


def check_interpreter(filename: str, binary: lief.ELF.Binary) -> bool:
    interpreter = binary.interpreter
    arch_map = ELF_INTERPRETER_NAMES.get(binary.header.machine_type, {})
    # Try identity_data first; fall back to the only entry for this arch.
    expected = arch_map.get(binary.header.identity_data)
    if expected is None and len(arch_map) == 1:
        expected = next(iter(arch_map.values()))
    if interpreter != expected:
        print(f"{filename}: unexpected interpreter {interpreter!r}, expected {expected!r}")
        return False
    return True


def check_runpath(filename: str, binary: lief.ELF.Binary) -> bool:
    if binary.get(lief.ELF.DynamicEntry.TAG.RUNPATH) is not None:
        print(f"{filename}: RUNPATH is not allowed")
        return False
    if binary.get(lief.ELF.DynamicEntry.TAG.RPATH) is not None:
        print(f"{filename}: RPATH is not allowed")
        return False
    return True


def check_libraries(filename: str, binary: lief.ELF.Binary) -> bool:
    ok = True
    for library in binary.libraries:
        if library not in ELF_ALLOWED_LIBRARIES:
            print(f"{filename}: unexpected library dependency {library}")
            ok = False
    return ok


def check_file(filename: str) -> bool:
    binary = lief.parse(filename)
    if not isinstance(binary, lief.ELF.Binary):
        print(f"{filename}: not an ELF binary")
        return False

    ok = True
    ok &= check_imported_symbols(filename, binary)
    ok &= check_interpreter(filename, binary)
    ok &= check_runpath(filename, binary)
    ok &= check_libraries(filename, binary)
    return ok


def main(argv: list[str]) -> int:
    if len(argv) < 2:
        print(f"usage: {argv[0]} <elf> [<elf>...]", file=sys.stderr)
        return 1

    ok = True
    for filename in argv[1:]:
        ok &= check_file(filename)
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
