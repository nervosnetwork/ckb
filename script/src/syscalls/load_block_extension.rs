use crate::{
    cost_model::transferred_byte_cycles,
    syscalls::{
        utils::store_data, Source, SourceEntry, INDEX_OUT_OF_BOUND, ITEM_MISSING,
        LOAD_BLOCK_EXTENSION, SUCCESS,
    },
    types::SgData,
};
use ckb_traits::ExtensionProvider;
use ckb_types::{
    core::cell::CellMeta,
    packed::{self, Byte32Vec},
};
use ckb_vm::{
    registers::{A0, A3, A4, A7},
    Error as VMError, Register, SupportMachine, Syscalls,
};

#[derive(Debug)]
pub struct LoadBlockExtension<DL> {
    sg_data: SgData<DL>,
}

impl<DL: ExtensionProvider + Clone> LoadBlockExtension<DL> {
    pub fn new(sg_data: &SgData<DL>) -> LoadBlockExtension<DL> {
        LoadBlockExtension {
            sg_data: sg_data.clone(),
        }
    }

    #[inline]
    fn header_deps(&self) -> Byte32Vec {
        self.sg_data.rtx.transaction.header_deps()
    }

    #[inline]
    fn resolved_inputs(&self) -> &Vec<CellMeta> {
        &self.sg_data.rtx.resolved_inputs
    }

    #[inline]
    fn resolved_cell_deps(&self) -> &Vec<CellMeta> {
        &self.sg_data.rtx.resolved_cell_deps
    }

    fn load_block_extension(&self, cell_meta: &CellMeta) -> Option<packed::Bytes> {
        let block_hash = &cell_meta
            .transaction_info
            .as_ref()
            .expect("block_info of CellMeta should exists when load_block_extension in syscall")
            .block_hash;
        if self
            .header_deps()
            .into_iter()
            .any(|hash| &hash == block_hash)
        {
            self.sg_data.data_loader().get_block_extension(block_hash)
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
                .and_then(|cell_meta| self.load_block_extension(cell_meta).ok_or(ITEM_MISSING)),
            Source::Transaction(SourceEntry::Output) => Err(INDEX_OUT_OF_BOUND),
            Source::Transaction(SourceEntry::CellDep) => self
                .resolved_cell_deps()
                .get(index)
                .ok_or(INDEX_OUT_OF_BOUND)
                .and_then(|cell_meta| self.load_block_extension(cell_meta).ok_or(ITEM_MISSING)),
            Source::Transaction(SourceEntry::HeaderDep) => self
                .header_deps()
                .get(index)
                .ok_or(INDEX_OUT_OF_BOUND)
                .and_then(|block_hash| {
                    self.sg_data
                        .data_loader()
                        .get_block_extension(&block_hash)
                        .ok_or(ITEM_MISSING)
                }),
            Source::Group(SourceEntry::Input) => self
                .sg_data
                .group_inputs()
                .get(index)
                .ok_or(INDEX_OUT_OF_BOUND)
                .and_then(|actual_index| {
                    self.resolved_inputs()
                        .get(*actual_index)
                        .ok_or(INDEX_OUT_OF_BOUND)
                })
                .and_then(|cell_meta| self.load_block_extension(cell_meta).ok_or(ITEM_MISSING)),
            Source::Group(SourceEntry::Output) => Err(INDEX_OUT_OF_BOUND),
            Source::Group(SourceEntry::CellDep) => Err(INDEX_OUT_OF_BOUND),
            Source::Group(SourceEntry::HeaderDep) => Err(INDEX_OUT_OF_BOUND),
        }
    }
}

impl<DL: ExtensionProvider + Send + Sync + Clone, Mac: SupportMachine> Syscalls<Mac>
    for LoadBlockExtension<DL>
{
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != LOAD_BLOCK_EXTENSION {
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
