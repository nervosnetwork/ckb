use crate::syscalls::{Source, ITEM_MISSING, LOAD_HEADER_SYSCALL_NUMBER, SUCCESS};
use ckb_core::cell::ResolvedOutPoint;
use ckb_core::header::Header;
use ckb_protocol::Header as FbsHeader;
use ckb_vm::{
    registers::{A0, A1, A2, A3, A4, A7},
    Error as VMError, Memory, Register, SupportMachine, Syscalls,
};
use flatbuffers::FlatBufferBuilder;
use std::cmp;

#[derive(Debug)]
pub struct LoadHeader<'a> {
    resolved_inputs: &'a [&'a ResolvedOutPoint],
    resolved_deps: &'a [&'a ResolvedOutPoint],
}

impl<'a> LoadHeader<'a> {
    pub fn new(
        resolved_inputs: &'a [&'a ResolvedOutPoint],
        resolved_deps: &'a [&'a ResolvedOutPoint],
    ) -> LoadHeader<'a> {
        LoadHeader {
            resolved_inputs,
            resolved_deps,
        }
    }

    fn fetch_header(&self, source: Source, index: usize) -> Option<&Header> {
        match source {
            Source::Input => self.resolved_inputs.get(index).and_then(|r| r.header()),
            Source::Output => None,
            Source::Dep => self.resolved_deps.get(index).and_then(|r| r.header()),
        }
    }
}

impl<'a, Mac: SupportMachine> Syscalls<Mac> for LoadHeader<'a> {
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != LOAD_HEADER_SYSCALL_NUMBER {
            return Ok(false);
        }
        machine.add_cycles(100)?;

        let addr = machine.registers()[A0].to_usize();
        let size_addr = machine.registers()[A1].to_usize();
        let size = machine
            .memory_mut()
            .load64(&Mac::REG::from_usize(size_addr))?
            .to_usize();

        let index = machine.registers()[A3].to_usize();
        let source = Source::parse_from_u64(machine.registers()[A4].to_u64())?;

        let header = self.fetch_header(source, index);
        if header.is_none() {
            machine.set_register(A0, Mac::REG::from_u8(ITEM_MISSING));
            return Ok(true);
        }
        let header = header.unwrap();

        let mut builder = FlatBufferBuilder::new();
        let offset = FbsHeader::build(&mut builder, header);
        builder.finish(offset, None);
        let data = builder.finished_data();

        let offset = machine.registers()[A2].to_usize();
        let full_size = data.len() - offset;
        let real_size = cmp::min(size, full_size);
        machine.memory_mut().store64(
            &Mac::REG::from_usize(size_addr),
            &Mac::REG::from_usize(full_size),
        )?;
        machine
            .memory_mut()
            .store_bytes(addr, &data[offset..offset + real_size])?;
        machine.set_register(A0, Mac::REG::from_u8(SUCCESS));
        machine.add_cycles(data.len() as u64 * 100)?;
        Ok(true)
    }
}
