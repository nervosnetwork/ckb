#![no_main]

use ckb_script::{InitialProgramLoadLimit, InitialProgramLoadReceipt};
use ckb_vm::{bytes::Bytes, elf::parse_elf};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let program = Bytes::copy_from_slice(data);
    for version in 0..=2 {
        let Ok(metadata) = parse_elf::<u64>(&program, version) else {
            continue;
        };
        let Some(receipt) = InitialProgramLoadReceipt::from_metadata(&metadata) else {
            continue;
        };
        assert_eq!(receipt.action_count(), metadata.actions.len());
        let exact = metadata
            .actions
            .iter()
            .try_fold(0u64, |total, action| total.checked_add(action.size))
            .expect("a successful receipt has a representable exact sum");
        assert_eq!(receipt.mapped_bytes(), exact);
        if exact > 0 {
            let limit = InitialProgramLoadLimit::new(exact)
                .expect("a positive exact mapped-byte sum is a valid limit");
            assert!(limit.admits(receipt));
        }
    }
});
