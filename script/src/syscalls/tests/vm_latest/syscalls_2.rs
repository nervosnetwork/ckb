use ckb_vm::{
    registers::{A0, A1, A2, A3, A4, A5, A7},
    CoreMachine, SupportMachine, Syscalls,
};

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

    assert_eq!(result.unwrap(), true);
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

    assert_eq!(result.unwrap(), true);
    assert_eq!(machine.registers()[A0], cycles);
}
