// file is loaded as a module multiple times，this behavior is intentional,
// in order to reuse the test cases
#![allow(clippy::duplicate_mod)]

pub(crate) mod utils;

mod vm_version_0;
#[path = "vm_latest/mod.rs"]
mod vm_version_1;

#[allow(clippy::assertions_on_constants)]
#[test]
fn test_max_argv_length() {
    assert!(crate::syscalls::MAX_ARGV_LENGTH < u64::MAX);
}
