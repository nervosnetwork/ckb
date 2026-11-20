use crate::{
    cost_model::transferred_byte_cycles,
    syscalls::{
        CellField, INDEX_OUT_OF_BOUND, ITEM_MISSING, LOAD_CELL_BY_FIELD_SYSCALL_NUMBER,
        LOAD_CELL_SYSCALL_NUMBER, SUCCESS, Source, SourceEntry, utils::store_data,
    },
    types::{CellScriptHashes, SgData, TxInfo},
};
use byteorder::{LittleEndian, WriteBytesExt};
use ckb_traits::CellDataProvider;
use ckb_types::{
    core::{Capacity, cell::CellMeta},
    packed::CellOutput,
    prelude::*,
};
use ckb_vm::{
    Error as VMError, Register, SupportMachine, Syscalls,
    registers::{A0, A3, A4, A5, A7},
};

pub struct LoadCell<DL> {
    sg_data: SgData<DL>,
}

impl<DL: CellDataProvider + Clone> LoadCell<DL> {
    pub fn new(sg_data: &SgData<DL>) -> LoadCell<DL> {
        LoadCell {
            sg_data: sg_data.clone(),
        }
    }

    #[inline]
    fn tx_info(&self) -> &TxInfo<DL> {
        &self.sg_data.tx_info
    }

    #[inline]
    fn resolved_inputs(&self) -> &Vec<CellMeta> {
        &self.sg_data.rtx.resolved_inputs
    }

    #[inline]
    fn resolved_cell_deps(&self) -> &Vec<CellMeta> {
        &self.sg_data.rtx.resolved_cell_deps
    }

    fn fetch_cell(
        &self,
        source: Source,
        index: usize,
    ) -> Result<(&CellMeta, Option<&CellScriptHashes>), u8> {
        let cache = &self.tx_info().script_hash_cache;
        let fetch_input = |actual_index: usize| {
            self.resolved_inputs()
                .get(actual_index)
                .map(|cell| (cell, cache.inputs.get(actual_index)))
                .ok_or(INDEX_OUT_OF_BOUND)
        };
        let fetch_output = |actual_index: usize| {
            self.tx_info()
                .outputs
                .get(actual_index)
                .map(|cell| (cell, cache.outputs.get(actual_index)))
                .ok_or(INDEX_OUT_OF_BOUND)
        };
        match source {
            Source::Transaction(SourceEntry::Input) => fetch_input(index),
            Source::Transaction(SourceEntry::Output) => fetch_output(index),
            Source::Transaction(SourceEntry::CellDep) => self
                .resolved_cell_deps()
                .get(index)
                .map(|cell| (cell, cache.cell_deps.get(index)))
                .ok_or(INDEX_OUT_OF_BOUND),
            Source::Transaction(SourceEntry::HeaderDep) => Err(INDEX_OUT_OF_BOUND),
            Source::Group(SourceEntry::Input) => self
                .sg_data
                .group_inputs()
                .get(index)
                .ok_or(INDEX_OUT_OF_BOUND)
                .and_then(|actual_index| fetch_input(*actual_index)),
            Source::Group(SourceEntry::Output) => self
                .sg_data
                .group_outputs()
                .get(index)
                .ok_or(INDEX_OUT_OF_BOUND)
                .and_then(|actual_index| fetch_output(*actual_index)),
            Source::Group(SourceEntry::CellDep) => Err(INDEX_OUT_OF_BOUND),
            Source::Group(SourceEntry::HeaderDep) => Err(INDEX_OUT_OF_BOUND),
        }
    }

    fn load_full<Mac: SupportMachine>(
        &self,
        machine: &mut Mac,
        output: &CellOutput,
    ) -> Result<(u8, u64), VMError> {
        let data = output.as_slice();
        let wrote_size = store_data(machine, data)?;
        Ok((SUCCESS, wrote_size))
    }

    fn load_by_field<Mac: SupportMachine>(
        &self,
        machine: &mut Mac,
        cell: &CellMeta,
        script_hashes: Option<&CellScriptHashes>,
    ) -> Result<(u8, u64), VMError> {
        let field = CellField::parse_from_u64(machine.registers()[A5].to_u64())?;
        let output = &cell.cell_output;

        let result = match field {
            CellField::Capacity => {
                let capacity: Capacity = output.capacity().into();
                let mut buffer = vec![];
                buffer.write_u64::<LittleEndian>(capacity.as_u64())?;
                (SUCCESS, store_data(machine, &buffer)?)
            }
            CellField::DataHash => {
                if let Some(bytes) = self.tx_info().data_loader.load_cell_data_hash(cell) {
                    (SUCCESS, store_data(machine, &bytes.as_bytes())?)
                } else {
                    (ITEM_MISSING, 0)
                }
            }
            CellField::OccupiedCapacity => {
                let mut buffer = vec![];
                buffer.write_u64::<LittleEndian>(
                    cell.occupied_capacity()
                        .map_err(|_| {
                            VMError::Unexpected(format!(
                                "Unexpected occupied_capacity overflow {}",
                                cell.out_point,
                            ))
                        })?
                        .as_u64(),
                )?;
                (SUCCESS, store_data(machine, &buffer)?)
            }
            CellField::Lock => {
                let lock = output.lock();
                let data = lock.as_slice();
                (SUCCESS, store_data(machine, data)?)
            }
            CellField::LockHash => {
                // Reuse the memoized hash, so repeated requests won't hash the
                // (potentially large) script again on the host side.
                let hash = match script_hashes {
                    Some(script_hashes) => script_hashes.lock_hash(output),
                    None => output.calc_lock_hash(),
                };
                let bytes = hash.as_bytes();
                (SUCCESS, store_data(machine, &bytes)?)
            }
            CellField::Type => match output.type_().to_opt() {
                Some(type_) => {
                    let data = type_.as_slice();
                    (SUCCESS, store_data(machine, data)?)
                }
                None => (ITEM_MISSING, 0),
            },
            CellField::TypeHash => {
                let hash = match script_hashes {
                    Some(script_hashes) => script_hashes.type_hash(output),
                    None => output.type_().to_opt().map(|t| t.calc_script_hash()),
                };
                match hash {
                    Some(hash) => {
                        let bytes = hash.as_bytes();
                        (SUCCESS, store_data(machine, &bytes)?)
                    }
                    None => (ITEM_MISSING, 0),
                }
            }
        };
        Ok(result)
    }
}

impl<Mac: SupportMachine, DL: CellDataProvider + Send + Sync + Clone> Syscalls<Mac>
    for LoadCell<DL>
{
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        let load_by_field = match machine.registers()[A7].to_u64() {
            LOAD_CELL_SYSCALL_NUMBER => false,
            LOAD_CELL_BY_FIELD_SYSCALL_NUMBER => true,
            _ => return Ok(false),
        };

        let index = machine.registers()[A3].to_u64();
        let source = Source::parse_from_u64(machine.registers()[A4].to_u64())?;

        let (cell, script_hashes) = match self.fetch_cell(source, index as usize) {
            Ok(fetched) => fetched,
            Err(err) => {
                machine.set_register(A0, Mac::REG::from_u8(err));
                return Ok(true);
            }
        };
        let (return_code, len) = if load_by_field {
            self.load_by_field(machine, cell, script_hashes)?
        } else {
            self.load_full(machine, &cell.cell_output)?
        };

        machine.add_cycles_no_checking(transferred_byte_cycles(len))?;
        machine.set_register(A0, Mac::REG::from_u8(return_code));
        Ok(true)
    }
}
