use byteorder::{ByteOrder, LittleEndian};
use ckb_vm::{
    registers::{A0, A1, A2},
    Error as VMError, Memory, Register, SupportMachine,
};
use std::cmp;

pub fn store_data<Mac: SupportMachine>(machine: &mut Mac, data: &[u8]) -> Result<u64, VMError> {
    let addr = machine.registers()[A0].to_u64();
    let size_addr = machine.registers()[A1].clone();
    let data_len = data.len() as u64;
    let offset = cmp::min(data_len, machine.registers()[A2].to_u64());

    let size = machine.memory_mut().load64(&size_addr)?.to_u64();
    let full_size = data_len - offset;
    let real_size = cmp::min(size, full_size);
    machine
        .memory_mut()
        .store64(&size_addr, &Mac::REG::from_u64(full_size))?;
    machine
        .memory_mut()
        .store_bytes(addr, &data[offset as usize..(offset + real_size) as usize])?;
    Ok(real_size)
}

pub fn store_u64<Mac: SupportMachine>(machine: &mut Mac, v: u64) -> Result<u64, VMError> {
    let mut buffer = [0u8; std::mem::size_of::<u64>()];
    LittleEndian::write_u64(&mut buffer, v);
    store_data(machine, &buffer)
}
