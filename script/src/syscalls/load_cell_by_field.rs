use crate::syscalls::{
    utils::store_data, CellField, Source, ITEM_MISSING, LOAD_CELL_BY_FIELD_SYSCALL_NUMBER, SUCCESS,
};
use byteorder::{LittleEndian, WriteBytesExt};
use ckb_core::transaction::CellOutput;
use ckb_protocol::Script as FbsScript;
use ckb_vm::{CoreMachine, Error as VMError, Memory, Register, Syscalls, A0, A3, A4, A5, A7};
use flatbuffers::FlatBufferBuilder;

#[derive(Debug)]
pub struct LoadCellByField<'a> {
    outputs: &'a [&'a CellOutput],
    input_cells: &'a [&'a CellOutput],
    current: &'a CellOutput,
    dep_cells: &'a [&'a CellOutput],
}

impl<'a> LoadCellByField<'a> {
    pub fn new(
        outputs: &'a [&'a CellOutput],
        input_cells: &'a [&'a CellOutput],
        current: &'a CellOutput,
        dep_cells: &'a [&'a CellOutput],
    ) -> LoadCellByField<'a> {
        LoadCellByField {
            outputs,
            input_cells,
            current,
            dep_cells,
        }
    }

    fn fetch_cell(&self, source: Source, index: usize) -> Option<&CellOutput> {
        match source {
            Source::Input => self.input_cells.get(index).cloned(),
            Source::Output => self.outputs.get(index).cloned(),
            Source::Current => Some(self.current),
            Source::Dep => self.dep_cells.get(index).cloned(),
        }
    }
}

impl<'a, R: Register, M: Memory> Syscalls<R, M> for LoadCellByField<'a> {
    fn initialize(&mut self, _machine: &mut CoreMachine<R, M>) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut CoreMachine<R, M>) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != LOAD_CELL_BY_FIELD_SYSCALL_NUMBER {
            return Ok(false);
        }

        let index = machine.registers()[A3].to_usize();
        let source = Source::parse_from_u64(machine.registers()[A4].to_u64())?;
        let field = CellField::parse_from_u64(machine.registers()[A5].to_u64())?;

        let cell = self.fetch_cell(source, index);
        if cell.is_none() {
            machine.registers_mut()[A0] = R::from_u8(ITEM_MISSING);
            return Ok(true);
        }
        let cell = cell.unwrap();

        let (return_code, data_length) = match field {
            CellField::Capacity => {
                let mut buffer = vec![];
                buffer.write_u64::<LittleEndian>(cell.capacity)?;
                store_data(machine, &buffer)?;
                (SUCCESS, buffer.len())
            }
            CellField::Data => {
                store_data(machine, &cell.data)?;
                (SUCCESS, cell.data.len())
            }
            CellField::DataHash => {
                let hash = cell.data_hash();
                let bytes = hash.as_bytes();
                store_data(machine, &bytes)?;
                (SUCCESS, bytes.len())
            }
            CellField::LockHash => {
                let bytes = cell.lock.as_bytes();
                store_data(machine, &bytes)?;
                (SUCCESS, bytes.len())
            }
            CellField::Type => match cell.type_ {
                Some(ref type_) => {
                    let mut builder = FlatBufferBuilder::new();
                    let offset = FbsScript::build(&mut builder, &type_);
                    builder.finish(offset, None);
                    let data = builder.finished_data();
                    store_data(machine, data)?;
                    (SUCCESS, data.len())
                }
                None => (ITEM_MISSING, 0),
            },
            CellField::TypeHash => match cell.type_ {
                Some(ref type_) => {
                    let hash = type_.type_hash();
                    let bytes = hash.as_bytes();
                    store_data(machine, &bytes)?;
                    (SUCCESS, bytes.len())
                }
                None => (ITEM_MISSING, 0),
            },
        };
        machine.registers_mut()[A0] = R::from_u8(return_code);
        machine.add_cycles((data_length as u64 + 1) * 10);
        Ok(true)
    }
}
