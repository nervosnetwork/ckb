use crate::{
    cost_model::transferred_byte_cycles,
    syscalls::{
        utils::store_data, CellField, Source, SourceEntry, INDEX_OUT_OF_BOUND, ITEM_MISSING,
        LOAD_CELL_BY_FIELD_SYSCALL_NUMBER, LOAD_CELL_SYSCALL_NUMBER, SUCCESS,
    },
};
use byteorder::{LittleEndian, WriteBytesExt};
use ckb_traits::CellDataProvider;
use ckb_types::core::cell::ResolvedTransaction;
use ckb_types::{
    core::{cell::CellMeta, Capacity},
    packed::CellOutput,
    prelude::*,
};
use ckb_vm::{
    registers::{A0, A3, A4, A5, A7},
    Error as VMError, Register, SupportMachine, Syscalls,
};
use std::rc::Rc;

pub struct LoadCell<DL> {
    data_loader: DL,
    outputs: Rc<Vec<CellMeta>>,
    rtx: Rc<ResolvedTransaction>,
    group_inputs: Rc<Vec<usize>>,
    group_outputs: Rc<Vec<usize>>,
}

impl<DL: CellDataProvider> LoadCell<DL> {
    pub fn new(
        data_loader: DL,
        rtx: Rc<ResolvedTransaction>,
        outputs: Rc<Vec<CellMeta>>,
        group_inputs: Rc<Vec<usize>>,
        group_outputs: Rc<Vec<usize>>,
    ) -> LoadCell<DL> {
        LoadCell {
            data_loader,
            outputs,
            group_inputs,
            group_outputs,
            rtx,
        }
    }

    #[inline]
    fn resolved_inputs(&self) -> &Vec<CellMeta> {
        &self.rtx.resolved_inputs
    }

    #[inline]
    fn resolved_cell_deps(&self) -> &Vec<CellMeta> {
        &self.rtx.resolved_cell_deps
    }

    fn fetch_cell(&self, source: Source, index: usize) -> Result<&CellMeta, u8> {
        match source {
            Source::Transaction(SourceEntry::Input) => {
                self.resolved_inputs().get(index).ok_or(INDEX_OUT_OF_BOUND)
            }
            Source::Transaction(SourceEntry::Output) => {
                self.outputs.get(index).ok_or(INDEX_OUT_OF_BOUND)
            }
            Source::Transaction(SourceEntry::CellDep) => self
                .resolved_cell_deps()
                .get(index)
                .ok_or(INDEX_OUT_OF_BOUND),
            Source::Transaction(SourceEntry::HeaderDep) => Err(INDEX_OUT_OF_BOUND),
            Source::Group(SourceEntry::Input) => self
                .group_inputs
                .get(index)
                .ok_or(INDEX_OUT_OF_BOUND)
                .and_then(|actual_index| {
                    self.resolved_inputs()
                        .get(*actual_index)
                        .ok_or(INDEX_OUT_OF_BOUND)
                }),
            Source::Group(SourceEntry::Output) => self
                .group_outputs
                .get(index)
                .ok_or(INDEX_OUT_OF_BOUND)
                .and_then(|actual_index| self.outputs.get(*actual_index).ok_or(INDEX_OUT_OF_BOUND)),
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
    ) -> Result<(u8, u64), VMError> {
        let field = CellField::parse_from_u64(machine.registers()[A5].to_u64())?;
        let output = &cell.cell_output;

        let result = match field {
            CellField::Capacity => {
                let capacity: Capacity = output.capacity().unpack();
                let mut buffer = vec![];
                buffer.write_u64::<LittleEndian>(capacity.as_u64())?;
                (SUCCESS, store_data(machine, &buffer)?)
            }
            CellField::DataHash => {
                if let Some(bytes) = self.data_loader.load_cell_data_hash(cell) {
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
                let hash = output.calc_lock_hash();
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
            CellField::TypeHash => match output.type_().to_opt() {
                Some(type_) => {
                    let hash = type_.calc_script_hash();
                    let bytes = hash.as_bytes();
                    (SUCCESS, store_data(machine, &bytes)?)
                }
                None => (ITEM_MISSING, 0),
            },
        };
        Ok(result)
    }
}

impl<Mac: SupportMachine, DL: CellDataProvider> Syscalls<Mac> for LoadCell<DL> {
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

        let cell = self.fetch_cell(source, index as usize);
        if let Err(err) = cell {
            machine.set_register(A0, Mac::REG::from_u8(err));
            return Ok(true);
        }
        let cell = cell.unwrap();
        let (return_code, len) = if load_by_field {
            self.load_by_field(machine, cell)?
        } else {
            self.load_full(machine, &cell.cell_output)?
        };

        machine.add_cycles_no_checking(transferred_byte_cycles(len as u64))?;
        machine.set_register(A0, Mac::REG::from_u8(return_code));
        Ok(true)
    }
}
