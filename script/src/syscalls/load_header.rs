use crate::{
    cost_model::transferred_byte_cycles,
    syscalls::{
        utils::{store_data, store_u64},
        HeaderField, Source, SourceEntry, INDEX_OUT_OF_BOUND, ITEM_MISSING,
        LOAD_HEADER_BY_FIELD_SYSCALL_NUMBER, LOAD_HEADER_SYSCALL_NUMBER, SUCCESS,
    },
    types::SgData,
};
use ckb_traits::HeaderProvider;
use ckb_types::{
    core::{cell::CellMeta, HeaderView},
    packed::Byte32Vec,
    prelude::*,
};
use ckb_vm::{
    registers::{A0, A3, A4, A5, A7},
    Error as VMError, Register, SupportMachine, Syscalls,
};
#[derive(Debug)]
pub struct LoadHeader<DL> {
    sg_data: SgData<DL>,
}

impl<DL: HeaderProvider + Clone> LoadHeader<DL> {
    pub fn new(sg_data: &SgData<DL>) -> LoadHeader<DL> {
        LoadHeader {
            sg_data: sg_data.clone(),
        }
    }

    // This can only be used for liner search
    // header_deps: Byte32Vec,
    // resolved_inputs: &'a [CellMeta],
    // resolved_cell_deps: &'a [CellMeta],
    #[inline]
    fn group_inputs(&self) -> &[usize] {
        self.sg_data.group_inputs()
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

    fn load_header(&self, cell_meta: &CellMeta) -> Option<HeaderView> {
        let block_hash = &cell_meta
            .transaction_info
            .as_ref()
            .expect("block_info of CellMeta should exists when load_header in syscall")
            .block_hash;
        if self
            .header_deps()
            .into_iter()
            .any(|hash| &hash == block_hash)
        {
            self.sg_data.tx_info.data_loader.get_header(block_hash)
        } else {
            None
        }
    }

    fn fetch_header(&self, source: Source, index: usize) -> Result<HeaderView, u8> {
        match source {
            Source::Transaction(SourceEntry::Input) => self
                .resolved_inputs()
                .get(index)
                .ok_or(INDEX_OUT_OF_BOUND)
                .and_then(|cell_meta| self.load_header(cell_meta).ok_or(ITEM_MISSING)),
            Source::Transaction(SourceEntry::Output) => Err(INDEX_OUT_OF_BOUND),
            Source::Transaction(SourceEntry::CellDep) => self
                .resolved_cell_deps()
                .get(index)
                .ok_or(INDEX_OUT_OF_BOUND)
                .and_then(|cell_meta| self.load_header(cell_meta).ok_or(ITEM_MISSING)),
            Source::Transaction(SourceEntry::HeaderDep) => self
                .header_deps()
                .get(index)
                .ok_or(INDEX_OUT_OF_BOUND)
                .and_then(|block_hash| {
                    self.sg_data
                        .tx_info
                        .data_loader
                        .get_header(&block_hash)
                        .ok_or(ITEM_MISSING)
                }),
            Source::Group(SourceEntry::Input) => self
                .group_inputs()
                .get(index)
                .ok_or(INDEX_OUT_OF_BOUND)
                .and_then(|actual_index| {
                    self.resolved_inputs()
                        .get(*actual_index)
                        .ok_or(INDEX_OUT_OF_BOUND)
                })
                .and_then(|cell_meta| self.load_header(cell_meta).ok_or(ITEM_MISSING)),
            Source::Group(SourceEntry::Output) => Err(INDEX_OUT_OF_BOUND),
            Source::Group(SourceEntry::CellDep) => Err(INDEX_OUT_OF_BOUND),
            Source::Group(SourceEntry::HeaderDep) => Err(INDEX_OUT_OF_BOUND),
        }
    }

    fn load_full<Mac: SupportMachine>(
        &self,
        machine: &mut Mac,
        header: &HeaderView,
    ) -> Result<(u8, u64), VMError> {
        let data = header.data().as_bytes();
        let wrote_size = store_data(machine, &data)?;
        Ok((SUCCESS, wrote_size))
    }

    fn load_by_field<Mac: SupportMachine>(
        &self,
        machine: &mut Mac,
        header: &HeaderView,
    ) -> Result<(u8, u64), VMError> {
        let field = HeaderField::parse_from_u64(machine.registers()[A5].to_u64())?;
        let epoch = header.epoch();

        let result = match field {
            HeaderField::EpochNumber => epoch.number(),
            HeaderField::EpochStartBlockNumber => {
                header.number().checked_sub(epoch.index()).ok_or_else(|| {
                    VMError::Unexpected(format!(
                        "Unexpected header epoch number index overflow {epoch}"
                    ))
                })?
            }
            HeaderField::EpochLength => epoch.length(),
        };

        Ok((SUCCESS, store_u64(machine, result)?))
    }
}

impl<DL: HeaderProvider + Send + Sync + Clone, Mac: SupportMachine> Syscalls<Mac>
    for LoadHeader<DL>
{
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        let load_by_field = match machine.registers()[A7].to_u64() {
            LOAD_HEADER_SYSCALL_NUMBER => false,
            LOAD_HEADER_BY_FIELD_SYSCALL_NUMBER => true,
            _ => return Ok(false),
        };

        let index = machine.registers()[A3].to_u64();
        let source = Source::parse_from_u64(machine.registers()[A4].to_u64())?;

        let header = self.fetch_header(source, index as usize);
        if let Err(err) = header {
            machine.set_register(A0, Mac::REG::from_u8(err));
            return Ok(true);
        }
        let header = header.unwrap();
        let (return_code, len) = if load_by_field {
            self.load_by_field(machine, &header)?
        } else {
            self.load_full(machine, &header)?
        };

        machine.add_cycles_no_checking(transferred_byte_cycles(len))?;
        machine.set_register(A0, Mac::REG::from_u8(return_code));
        Ok(true)
    }
}
