This binary comes from: <https://github.com/gpBlockchain/ckb-test-contracts/blob/main/rust/acceptance-contracts/contracts/spawn_demo/src/spawn_times.rs>. Since I couldn't build a binary in C that would cause the same bug, I just added the binary to the project.

```rs
#![no_std]
#![cfg_attr(not(test), no_main)]

#[cfg(test)]
extern crate alloc;

#[cfg(not(test))]
use ckb_std::default_alloc;
#[cfg(not(test))]
ckb_std::entry!(program_entry);
#[cfg(not(test))]
default_alloc!();

use core::result::Result;

use alloc::{vec};
use core::ffi::{CStr};

use ckb_std::{debug, syscalls};
use ckb_std::ckb_constants::Source;
use ckb_std::env::argv;
use ckb_std::syscalls::{current_cycles, get_memory_limit, set_content, spawn};


///
/// test case :
/// invoke int ckb_spawn( uint64_t memory_limit,
///                    size_t index,
///                    size_t source,
///                    size_t bounds,
///                    int argc, char* argv[],
///                    int8_t* exit_code,
///                    uint8_t* content,
///                    uint64_t* content_length);
///
///     for {
///         spawn(xxx)
///     }
///     case1 : for {
///                spawn(xxx)
///             }
///
///     result：
///         return ERROR : ExceededMaximumCycles
///
pub fn program_entry() -> i8 {
    // let argvs = argv();
    // debug!("argvs length:{:?}:{:?}",argvs.len(),argvs);

    if get_memory_limit() != 8 {
        return 0;
    }
    let mut exit_code: i8 = 0;
    let mut content: [u8; 10] = [1; 10];

    let content_length: u64 = content.len() as u64;
    let mut spawn_args = syscalls::SpawnArgs {
        memory_limit: 8,
        exit_code: &mut exit_code as *mut i8,
        content: content.as_mut_ptr(),
        content_length: &content_length as *const u64 as *mut u64,
    };
    // let cstr1 = CStr::from_bytes_with_nul(b"arg0\0").unwrap();
    //argv is empty
    let cstrs = vec![];

    spawn_args.memory_limit = 1;
    for i in 0..10000 {
        debug!("current idx:{:?}",i);
        let result = spawn(0, Source::CellDep, 0, cstrs.as_slice(), &spawn_args);
        assert_eq!(exit_code, 0);
        // debug!("result:{:?}",result);
        let cycles = current_cycles();
        debug!("cycle:{:?}",cycles);
    }
    return 0;
}
```
