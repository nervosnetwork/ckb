use crate::syscalls::{
    utils::store_data, Source, INDEX_OUT_OF_BOUND, ITEM_MISSING, LOAD_HEADER_SYSCALL_NUMBER,
    SUCCESS,
};
use ckb_core::cell::ResolvedOutPoint;
use ckb_core::header::Header;
use ckb_protocol::Header as FbsHeader;
use ckb_vm::{
    registers::{A0, A3, A4, A7},
    Error as VMError, Register, SupportMachine, Syscalls,
};
use flatbuffers::FlatBufferBuilder;

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

    fn fetch_header(&self, source: Source, index: usize) -> Result<&Header, u8> {
        match source {
            Source::Input => self
                .resolved_inputs
                .get(index)
                .ok_or(INDEX_OUT_OF_BOUND)
                .and_then(|r| r.header().ok_or(ITEM_MISSING)),
            Source::Output => Err(ITEM_MISSING),
            Source::Dep => self
                .resolved_deps
                .get(index)
                .ok_or(INDEX_OUT_OF_BOUND)
                .and_then(|r| r.header().ok_or(ITEM_MISSING)),
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

        let index = machine.registers()[A3].to_usize();
        let source = Source::parse_from_u64(machine.registers()[A4].to_u64())?;

        let header = self.fetch_header(source, index);
        if header.is_err() {
            machine.set_register(A0, Mac::REG::from_u8(header.unwrap_err()));
            return Ok(true);
        }
        let header = header.unwrap();

        let mut builder = FlatBufferBuilder::new();
        let offset = FbsHeader::build(&mut builder, header);
        builder.finish(offset, None);
        let data = builder.finished_data();

        store_data(machine, &data)?;
        machine.set_register(A0, Mac::REG::from_u8(SUCCESS));
        machine.add_cycles(data.len() as u64 * 10)?;
        Ok(true)
    }
}
