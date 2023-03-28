use ckb_vm::{
    registers::{A0, A1, A2, A3, A4, A5, A7},
    CoreMachine, Memory, SupportMachine, Syscalls,
};
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

    let result = CurrentCycles::new().ecall(&mut machine);

    assert!(result.unwrap());
    assert_eq!(machine.registers()[A0], cycles);
}

#[test]
fn test_get_memory_limit() {
    let mut machine = SCRIPT_VERSION.init_core_machine_without_limit();

    machine.set_register(A0, 0);
    machine.set_register(A1, 0);
    machine.set_register(A2, 0);
    machine.set_register(A3, 0);
    machine.set_register(A4, 0);
    machine.set_register(A5, 0);
    machine.set_register(A7, GET_MEMORY_LIMIT);

    let result = GetMemoryLimit::new(8).ecall(&mut machine);

    assert!(result.unwrap());
    assert_eq!(machine.registers()[A0], 8);
}

#[test]
fn test_set_content() {
    let mut machine = SCRIPT_VERSION.init_core_machine_without_limit();
    machine.memory_mut().store64(&20000, &10).unwrap();
    machine
        .memory_mut()
        .store_bytes(30000, &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9])
        .unwrap();

    machine.set_register(A0, 30000);
    machine.set_register(A1, 20000);
    machine.set_register(A2, 0);
    machine.set_register(A3, 0);
    machine.set_register(A4, 0);
    machine.set_register(A5, 0);
    machine.set_register(A7, SET_CONTENT);

    let content_data = Arc::new(Mutex::new(vec![]));
    let result =
        SetContent::new(Arc::<Mutex<Vec<u8>>>::clone(&content_data), 5).ecall(&mut machine);

    assert!(result.unwrap());
    assert_eq!(machine.memory_mut().load64(&20000).unwrap(), 5);
}
