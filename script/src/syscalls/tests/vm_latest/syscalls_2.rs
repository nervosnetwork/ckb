use crate::{syscalls::tests::utils::*, types::VmContext};
use ckb_types::{
    bytes::Bytes,
    core::{
        HeaderBuilder, TransactionBuilder, TransactionInfo,
        cell::{CellMeta, ResolvedTransaction},
    },
    packed::{CellOutput, OutPoint},
    prelude::*,
};
use ckb_vm::{
    CoreMachine, Memory, SupportMachine, Syscalls,
    registers::{A0, A1, A2, A3, A4, A5, A7},
};
use proptest::{collection::size_range, prelude::*};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::SCRIPT_VERSION;
use crate::syscalls::*;

#[test]
fn test_vm_version() {
    let mut machine = SCRIPT_VERSION.init_core_machine_without_limit();
    let vm_version = u64::from(SCRIPT_VERSION.vm_version());

    machine.set_register(A0, 0);
    machine.set_register(A1, 0);
    machine.set_register(A2, 0);
    machine.set_register(A3, 0);
    machine.set_register(A4, 0);
    machine.set_register(A5, 0);
    machine.set_register(A7, VM_VERSION);

    let result = VMVersion::new().ecall(&mut machine);

    assert!(result.unwrap());
    assert_eq!(machine.registers()[A0], vm_version);
}

#[test]
fn test_current_cycles() {
    let mut machine = SCRIPT_VERSION.init_core_machine_without_limit();
    let cycles = 100;

    machine.set_register(A0, 0);
    machine.set_register(A1, 0);
    machine.set_register(A2, 0);
    machine.set_register(A3, 0);
    machine.set_register(A4, 0);
    machine.set_register(A5, 0);
    machine.set_register(A7, CURRENT_CYCLES);

    machine.set_cycles(cycles);

    let rtx = Arc::new(ResolvedTransaction {
        transaction: TransactionBuilder::default().build(),
        resolved_cell_deps: vec![],
        resolved_inputs: vec![],
        resolved_dep_groups: vec![],
    });

    let sg_data = build_sg_data(rtx, vec![], vec![]);

    let vm_context = VmContext::new(&sg_data, &Arc::new(Mutex::new(Vec::new())));

    let result = CurrentCycles::new(&vm_context).ecall(&mut machine);

    assert!(result.unwrap());
    assert_eq!(machine.registers()[A0], cycles);
}

fn _test_load_extension(
    data: &[u8],
    index: u64,
    source: u64,
    ret: Result<(), u8>,
) -> Result<(), TestCaseError> {
    let mut machine = SCRIPT_VERSION.init_core_machine_without_limit();
    let size_addr: u64 = 0;
    let addr: u64 = 100;

    machine.set_register(A0, addr); // addr
    machine.set_register(A1, size_addr); // size_addr
    machine.set_register(A2, 0); // offset
    machine.set_register(A3, index); //index
    machine.set_register(A4, source); //source: 4
    machine.set_register(A7, LOAD_BLOCK_EXTENSION); // syscall number

    let data = Bytes::copy_from_slice(data);

    let header = HeaderBuilder::default().build();
    let cell = CellMeta {
        out_point: OutPoint::default(),
        transaction_info: Some(TransactionInfo {
            block_number: header.number(),
            block_hash: header.hash(),
            block_epoch: header.epoch(),
            index: 1,
        }),
        cell_output: CellOutput::new_builder().capacity(100.pack()).build(),
        data_bytes: 0,
        mem_cell_data: None,
        mem_cell_data_hash: None,
    };

    let mut extensions = HashMap::default();
    extensions.insert(header.hash(), data.pack());
    let data_loader = MockDataLoader {
        extensions,
        ..Default::default()
    };

    let rtx = Arc::new(ResolvedTransaction {
        transaction: TransactionBuilder::default()
            .header_dep(header.hash())
            .build(),
        resolved_cell_deps: vec![cell.clone()],
        resolved_inputs: vec![cell],
        resolved_dep_groups: vec![],
    });

    let sg_data = build_sg_data_with_loader(rtx, data_loader, vec![0], vec![]);

    let mut load_block_extension = LoadBlockExtension::new(&sg_data);

    prop_assert!(
        machine
            .memory_mut()
            .store64(&size_addr, &(data.len() as u64 + 20))
            .is_ok()
    );

    prop_assert!(load_block_extension.ecall(&mut machine).is_ok());

    if let Err(code) = ret {
        prop_assert_eq!(machine.registers()[A0], u64::from(code));
    } else {
        prop_assert_eq!(machine.registers()[A0], u64::from(SUCCESS));

        prop_assert_eq!(
            machine.memory_mut().load64(&size_addr),
            Ok(data.len() as u64)
        );

        for (i, addr) in (addr..addr + data.len() as u64).enumerate() {
            prop_assert_eq!(machine.memory_mut().load8(&addr), Ok(u64::from(data[i])));
        }
    }
    Ok(())
}

proptest! {
    #[test]
    fn test_load_extension(ref data in any_with::<Vec<u8>>(size_range(96).lift()), ) {
        for source in [
            Source::Transaction(SourceEntry::Input),
            Source::Transaction(SourceEntry::CellDep),
            Source::Transaction(SourceEntry::HeaderDep),
            Source::Group(SourceEntry::Input),
        ] {
            _test_load_extension(data, 0, u64::from(source), Ok(()))?;
        }
    }

    #[test]
    fn test_load_extension_out_of_slice(i in (1..1000u64)) {
        let data = vec![0; 10];
        for source in [
            Source::Transaction(SourceEntry::Input),
            Source::Transaction(SourceEntry::CellDep),
            Source::Transaction(SourceEntry::HeaderDep),
            Source::Group(SourceEntry::Input),
        ] {
            _test_load_extension(&data, i, u64::from(source), Err(INDEX_OUT_OF_BOUND))?;
        }

        for source in [
            Source::Transaction(SourceEntry::Output),
            Source::Group(SourceEntry::Output),
            Source::Group(SourceEntry::CellDep),
            Source::Group(SourceEntry::HeaderDep),
        ] {
            _test_load_extension(&data, 0, u64::from(source), Err(INDEX_OUT_OF_BOUND))?;
        }
    }
}
