use crate::common::{CurrentCell, LazyLoadCellOutput};
use crate::syscalls::{
    utils::store_data, CellField, Source, ITEM_MISSING, LOAD_CELL_BY_FIELD_SYSCALL_NUMBER, SUCCESS,
};
use byteorder::{LittleEndian, WriteBytesExt};
use ckb_core::cell::CellMeta;
use ckb_protocol::Script as FbsScript;
use ckb_vm::{Error as VMError, Register, SupportMachine, Syscalls, A0, A3, A4, A5, A7};
use flatbuffers::FlatBufferBuilder;
use std::sync::Arc;

#[derive(Debug)]
pub struct LoadCellByField<'a, CS> {
    store: Arc<CS>,
    outputs: &'a [CellMeta],
    input_cells: &'a [&'a CellMeta],
    current: CurrentCell,
    dep_cells: &'a [&'a CellMeta],
}

impl<'a, CS: LazyLoadCellOutput> LoadCellByField<'a, CS> {
    pub fn new(
        store: Arc<CS>,
        outputs: &'a [CellMeta],
        input_cells: &'a [&'a CellMeta],
        current: CurrentCell,
        dep_cells: &'a [&'a CellMeta],
    ) -> LoadCellByField<'a, CS> {
        LoadCellByField {
            store,
            outputs,
            input_cells,
            current,
            dep_cells,
        }
    }

    fn fetch_cell(&self, source: Source, index: usize) -> Option<&'a CellMeta> {
        match source {
            Source::Input => self.input_cells.get(index).cloned(),
            Source::Output => self.outputs.get(index),
            Source::Current => match self.current {
                CurrentCell::Input(index) => self.input_cells.get(index).cloned(),
                CurrentCell::Output(index) => self.outputs.get(index),
            },
            Source::Dep => self.dep_cells.get(index).cloned(),
        }
    }
}

impl<'a, Mac: SupportMachine, CS: LazyLoadCellOutput> Syscalls<Mac> for LoadCellByField<'a, CS> {
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != LOAD_CELL_BY_FIELD_SYSCALL_NUMBER {
            return Ok(false);
        }
        machine.add_cycles(10)?;

        let index = machine.registers()[A3].to_usize();
        let source = Source::parse_from_u64(machine.registers()[A4].to_u64())?;
        let field = CellField::parse_from_u64(machine.registers()[A5].to_u64())?;

        let cell = self.fetch_cell(source, index);
        if cell.is_none() {
            machine.set_register(A0, Mac::REG::from_u8(ITEM_MISSING));
            return Ok(true);
        }

        let cell = cell.unwrap();

        let (return_code, data_length) = match field {
            CellField::Capacity => {
                let mut buffer = vec![];
                buffer.write_u64::<LittleEndian>(cell.capacity().as_u64())?;
                store_data(machine, &buffer)?;
                (SUCCESS, buffer.len())
            }
            CellField::Data => {
                let output = self.store.lazy_load_cell_output(cell);
                store_data(machine, &output.data)?;
                (SUCCESS, output.data.len())
            }
            CellField::DataHash => {
                let hash = match cell.data_hash() {
                    Some(hash) => hash.to_owned(),
                    None => {
                        let output = self.store.lazy_load_cell_output(cell);
                        output.data_hash()
                    }
                };
                let bytes = hash.as_bytes();
                store_data(machine, &bytes)?;
                (SUCCESS, bytes.len())
            }
            CellField::Lock => {
                let output = self.store.lazy_load_cell_output(cell);
                let mut builder = FlatBufferBuilder::new();
                let offset = FbsScript::build(&mut builder, &output.lock);
                builder.finish(offset, None);
                let data = builder.finished_data();
                store_data(machine, data)?;
                (SUCCESS, data.len())
            }
            CellField::LockHash => {
                let output = self.store.lazy_load_cell_output(cell);
                let hash = output.lock.hash();
                let bytes = hash.as_bytes();
                store_data(machine, &bytes)?;
                (SUCCESS, bytes.len())
            }
            CellField::Type => {
                let output = self.store.lazy_load_cell_output(cell);
                match output.type_ {
                    Some(ref type_) => {
                        let mut builder = FlatBufferBuilder::new();
                        let offset = FbsScript::build(&mut builder, &type_);
                        builder.finish(offset, None);
                        let data = builder.finished_data();
                        store_data(machine, data)?;
                        (SUCCESS, data.len())
                    }
                    None => (ITEM_MISSING, 0),
                }
            }
            CellField::TypeHash => {
                let output = self.store.lazy_load_cell_output(cell);
                match output.type_ {
                    Some(ref type_) => {
                        let hash = type_.hash();
                        let bytes = hash.as_bytes();
                        store_data(machine, &bytes)?;
                        (SUCCESS, bytes.len())
                    }
                    None => (ITEM_MISSING, 0),
                }
            }
        };
        machine.set_register(A0, Mac::REG::from_u8(return_code));
        machine.add_cycles(data_length as u64 * 10)?;
        Ok(true)
    }
}
