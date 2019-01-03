use ckb_vm::{CoreMachine, Error as VMError, Memory, Register, A0, A1, A2};
use std::cmp;

pub fn store_data<R: Register, M: Memory>(
    machine: &mut CoreMachine<R, M>,
    data: &[u8],
) -> Result<(), VMError> {
    let addr = machine.registers()[A0].to_usize();
    let size_addr = machine.registers()[A1].to_usize();
    let offset = machine.registers()[A2].to_usize();

    let size = machine.memory_mut().load64(size_addr)? as usize;
    let full_size = data.len() - offset;
    let real_size = cmp::min(size, full_size);
    machine.memory_mut().store64(size_addr, full_size as u64)?;
    machine
        .memory_mut()
        .store_bytes(addr, &data[offset..offset + real_size])?;
    Ok(())
}
