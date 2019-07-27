use ckb_vm::{
    registers::{A0, A1, A2},
    Error as VMError, Memory, Register, SupportMachine,
};
use std::cmp;

pub fn store_data<Mac: SupportMachine>(machine: &mut Mac, data: &[u8]) -> Result<(), VMError> {
    let addr = machine.registers()[A0].to_u64();
    let size_addr = machine.registers()[A1].clone();
    let offset = machine.registers()[A2].to_u64();

    let size = machine.memory_mut().load64(&size_addr)?.to_u64();
    let full_size = data.len() as u64 - offset;
    let real_size = cmp::min(size, full_size);
    machine
        .memory_mut()
        .store64(&size_addr, &Mac::REG::from_u64(full_size))?;
    machine
        .memory_mut()
        .store_bytes(addr, &data[offset as usize..(offset + real_size) as usize])?;
    Ok(())
}
