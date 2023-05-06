use crate::types::Indices;
use crate::{
    cost_model::transferred_byte_cycles,
    syscalls::{
        utils::store_data, Source, SourceEntry, INDEX_OUT_OF_BOUND, ITEM_MISSING, LOAD_EXTENSION,
        SUCCESS,
    },
};
use ckb_traits::ExtensionProvider;
use ckb_types::core::cell::ResolvedTransaction;
use ckb_types::{
    core::cell::CellMeta,
    packed::{self, Byte32Vec},
};
use ckb_vm::{
    registers::{A0, A3, A4, A7},
    Error as VMError, Register, SupportMachine, Syscalls,
};
use std::sync::Arc;

#[derive(Debug)]
pub struct LoadExtension<DL> {
    data_loader: DL,
    rtx: Arc<ResolvedTransaction>,
    group_inputs: Indices,
}

impl<DL: ExtensionProvider> LoadExtension<DL> {
    pub fn new(
        data_loader: DL,
        rtx: Arc<ResolvedTransaction>,
        group_inputs: Indices,
    ) -> LoadExtension<DL> {
        LoadExtension {
            data_loader,
            rtx,
            group_inputs,
        }
    }

    #[inline]
    fn header_deps(&self) -> Byte32Vec {
        self.rtx.transaction.header_deps()
    }

    #[inline]
    fn resolved_inputs(&self) -> &Vec<CellMeta> {
        &self.rtx.resolved_inputs
    }

    #[inline]
    fn resolved_cell_deps(&self) -> &Vec<CellMeta> {
        &self.rtx.resolved_cell_deps
    }

    fn load_extension(&self, cell_meta: &CellMeta) -> Option<packed::Bytes> {
        let block_hash = &cell_meta
            .transaction_info
            .as_ref()
            .expect("block_info of CellMeta should exists when load_extension in syscall")
            .block_hash;
        if self
            .header_deps()
            .into_iter()
            .any(|hash| &hash == block_hash)
        {
            self.data_loader.get_block_extension(block_hash)
        } else {
            None
        }
    }

    fn fetch_extension(&self, source: Source, index: usize) -> Result<packed::Bytes, u8> {
        match source {
            Source::Transaction(SourceEntry::Input) => self
                .resolved_inputs()
                .get(index)
                .ok_or(INDEX_OUT_OF_BOUND)
                .and_then(|cell_meta| self.load_extension(cell_meta).ok_or(ITEM_MISSING)),
            Source::Transaction(SourceEntry::Output) => Err(INDEX_OUT_OF_BOUND),
            Source::Transaction(SourceEntry::CellDep) => self
                .resolved_cell_deps()
                .get(index)
                .ok_or(INDEX_OUT_OF_BOUND)
                .and_then(|cell_meta| self.load_extension(cell_meta).ok_or(ITEM_MISSING)),
            Source::Transaction(SourceEntry::HeaderDep) => self
                .header_deps()
                .get(index)
                .ok_or(INDEX_OUT_OF_BOUND)
                .and_then(|block_hash| {
                    self.data_loader
                        .get_block_extension(&block_hash)
                        .ok_or(ITEM_MISSING)
                }),
            Source::Group(SourceEntry::Input) => self
                .group_inputs
                .get(index)
                .ok_or(INDEX_OUT_OF_BOUND)
                .and_then(|actual_index| {
                    self.resolved_inputs()
                        .get(*actual_index)
                        .ok_or(INDEX_OUT_OF_BOUND)
                })
                .and_then(|cell_meta| self.load_extension(cell_meta).ok_or(ITEM_MISSING)),
            Source::Group(SourceEntry::Output) => Err(INDEX_OUT_OF_BOUND),
            Source::Group(SourceEntry::CellDep) => Err(INDEX_OUT_OF_BOUND),
            Source::Group(SourceEntry::HeaderDep) => Err(INDEX_OUT_OF_BOUND),
        }
    }
}

impl<DL: ExtensionProvider + Send + Sync, Mac: SupportMachine> Syscalls<Mac> for LoadExtension<DL> {
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != LOAD_EXTENSION {
            return Ok(false);
        }

        let index = machine.registers()[A3].to_u64();
        let source = Source::parse_from_u64(machine.registers()[A4].to_u64())?;

        let extension = self.fetch_extension(source, index as usize);
        if let Err(err) = extension {
            machine.set_register(A0, Mac::REG::from_u8(err));
            return Ok(true);
        }
        let extension = extension.unwrap();
        let data = extension.raw_data();
        let wrote_size = store_data(machine, &data)?;

        machine.add_cycles_no_checking(transferred_byte_cycles(wrote_size))?;
        machine.set_register(A0, Mac::REG::from_u8(SUCCESS));
        Ok(true)
    }
}
