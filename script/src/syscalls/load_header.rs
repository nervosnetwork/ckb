use crate::syscalls::{
    utils::store_data, Source, SourceEntry, INDEX_OUT_OF_BOUND, ITEM_MISSING,
    LOAD_HEADER_SYSCALL_NUMBER, SUCCESS,
};
use crate::DataLoader;
use ckb_core::cell::CellMeta;
use ckb_core::header::Header;
use ckb_protocol::Header as FbsHeader;
use ckb_vm::{
    registers::{A0, A3, A4, A7},
    Error as VMError, Register, SupportMachine, Syscalls,
};
use flatbuffers::FlatBufferBuilder;
use numext_fixed_hash::H256;

#[derive(Debug)]
pub struct LoadHeader<'a, DL> {
    data_loader: &'a DL,
    // This can only be used for liner search
    header_deps: &'a [H256],
    resolved_inputs: &'a [CellMeta],
    resolved_cell_deps: &'a [CellMeta],
    group_inputs: &'a [usize],
}

impl<'a, DL: DataLoader + 'a> LoadHeader<'a, DL> {
    pub fn new(
        data_loader: &'a DL,
        header_deps: &'a [H256],
        resolved_inputs: &'a [CellMeta],
        resolved_cell_deps: &'a [CellMeta],
        group_inputs: &'a [usize],
    ) -> LoadHeader<'a, DL> {
        LoadHeader {
            data_loader,
            header_deps,
            resolved_inputs,
            resolved_cell_deps,
            group_inputs,
        }
    }

    fn load_header(&self, cell_meta: &CellMeta) -> Option<Header> {
        let block_hash = &cell_meta
            .transaction_info
            .as_ref()
            .expect("block_info of CellMeta should exists when load_header in syscall")
            .block_hash;
        if self.header_deps.iter().any(|hash| hash == block_hash) {
            self.data_loader.get_header(block_hash)
        } else {
            None
        }
    }

    fn fetch_header(&self, source: Source, index: usize) -> Result<Header, u8> {
        match source {
            Source::Transaction(SourceEntry::Input) => self
                .resolved_inputs
                .get(index)
                .ok_or(INDEX_OUT_OF_BOUND)
                .and_then(|cell_meta| self.load_header(cell_meta).ok_or(ITEM_MISSING)),
            Source::Transaction(SourceEntry::Output) => Err(INDEX_OUT_OF_BOUND),
            Source::Transaction(SourceEntry::CellDep) => self
                .resolved_cell_deps
                .get(index)
                .ok_or(INDEX_OUT_OF_BOUND)
                .and_then(|cell_meta| self.load_header(cell_meta).ok_or(ITEM_MISSING)),
            Source::Transaction(SourceEntry::HeaderDep) => self
                .header_deps
                .get(index)
                .ok_or(INDEX_OUT_OF_BOUND)
                .and_then(|block_hash| self.data_loader.get_header(block_hash).ok_or(ITEM_MISSING)),
            Source::Group(SourceEntry::Input) => self
                .group_inputs
                .get(index)
                .ok_or(INDEX_OUT_OF_BOUND)
                .and_then(|actual_index| {
                    self.resolved_inputs
                        .get(*actual_index)
                        .ok_or(INDEX_OUT_OF_BOUND)
                })
                .and_then(|cell_meta| self.load_header(cell_meta).ok_or(ITEM_MISSING)),
            Source::Group(SourceEntry::Output) => Err(INDEX_OUT_OF_BOUND),
            Source::Group(SourceEntry::CellDep) => Err(INDEX_OUT_OF_BOUND),
            Source::Group(SourceEntry::HeaderDep) => Err(INDEX_OUT_OF_BOUND),
        }
    }
}

impl<'a, DL: DataLoader + 'a, Mac: SupportMachine> Syscalls<Mac> for LoadHeader<'a, DL> {
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != LOAD_HEADER_SYSCALL_NUMBER {
            return Ok(false);
        }

        let index = machine.registers()[A3].to_u64();
        let source = Source::parse_from_u64(machine.registers()[A4].to_u64())?;

        let header = self.fetch_header(source, index as usize);
        if header.is_err() {
            machine.set_register(A0, Mac::REG::from_u8(header.unwrap_err()));
            return Ok(true);
        }
        let header = header.unwrap();

        let mut builder = FlatBufferBuilder::new();
        let offset = FbsHeader::build(&mut builder, &header);
        builder.finish(offset, None);
        let data = builder.finished_data();

        store_data(machine, &data)?;
        machine.set_register(A0, Mac::REG::from_u8(SUCCESS));
        machine.add_cycles(data.len() as u64 * 10)?;
        Ok(true)
    }
}
