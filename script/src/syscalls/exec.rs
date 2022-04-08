use crate::cost_model::transferred_byte_cycles;
use crate::syscalls::{
    Place, Source, SourceEntry, EXEC, INDEX_OUT_OF_BOUND, SLICE_OUT_OF_BOUND, WRONG_FORMAT,
};
use ckb_traits::CellDataProvider;
use ckb_types::core::cell::CellMeta;
use ckb_types::packed::{Bytes as PackedBytes, BytesVec};
use ckb_vm::Memory;
use ckb_vm::{
    registers::{A0, A1, A2, A3, A4, A5, A7},
    Bytes, Error as VMError, Register, SupportMachine, Syscalls,
};
use ckb_vm::{DEFAULT_STACK_SIZE, RISCV_MAX_MEMORY};

#[derive(Debug)]
pub struct Exec<'a, DL> {
    data_loader: &'a DL,
    outputs: &'a [CellMeta],
    resolved_inputs: &'a [CellMeta],
    resolved_cell_deps: &'a [CellMeta],
    group_inputs: &'a [usize],
    group_outputs: &'a [usize],
    witnesses: BytesVec,
}

impl<'a, DL: CellDataProvider + 'a> Exec<'a, DL> {
    pub fn new(
        data_loader: &'a DL,
        outputs: &'a [CellMeta],
        resolved_inputs: &'a [CellMeta],
        resolved_cell_deps: &'a [CellMeta],
        group_inputs: &'a [usize],
        group_outputs: &'a [usize],
        witnesses: BytesVec,
    ) -> Exec<'a, DL> {
        Exec {
            data_loader,
            outputs,
            resolved_inputs,
            resolved_cell_deps,
            group_inputs,
            group_outputs,
            witnesses,
        }
    }

    fn fetch_cell(&self, source: Source, index: usize) -> Result<&'a CellMeta, u8> {
        let cell_opt = match source {
            Source::Transaction(SourceEntry::Input) => self.resolved_inputs.get(index),
            Source::Transaction(SourceEntry::Output) => self.outputs.get(index),
            Source::Transaction(SourceEntry::CellDep) => self.resolved_cell_deps.get(index),
            Source::Group(SourceEntry::Input) => self
                .group_inputs
                .get(index)
                .and_then(|actual_index| self.resolved_inputs.get(*actual_index)),
            Source::Group(SourceEntry::Output) => self
                .group_outputs
                .get(index)
                .and_then(|actual_index| self.outputs.get(*actual_index)),
            Source::Transaction(SourceEntry::HeaderDep)
            | Source::Group(SourceEntry::CellDep)
            | Source::Group(SourceEntry::HeaderDep) => {
                return Err(INDEX_OUT_OF_BOUND);
            }
        };

        cell_opt.ok_or(INDEX_OUT_OF_BOUND)
    }

    fn fetch_witness(&self, source: Source, index: usize) -> Result<PackedBytes, u8> {
        let witness_opt = match source {
            Source::Group(SourceEntry::Input) => self
                .group_inputs
                .get(index)
                .and_then(|actual_index| self.witnesses.get(*actual_index)),
            Source::Group(SourceEntry::Output) => self
                .group_outputs
                .get(index)
                .and_then(|actual_index| self.witnesses.get(*actual_index)),
            Source::Transaction(SourceEntry::Input) => self.witnesses.get(index),
            Source::Transaction(SourceEntry::Output) => self.witnesses.get(index),
            _ => {
                return Err(INDEX_OUT_OF_BOUND);
            }
        };

        witness_opt.ok_or(INDEX_OUT_OF_BOUND)
    }
}

fn load_c_string<Mac: SupportMachine>(machine: &mut Mac, addr: u64) -> Result<Bytes, VMError> {
    let mut buffer = Vec::new();
    let mut addr = addr;

    loop {
        let byte = machine
            .memory_mut()
            .load8(&Mac::REG::from_u64(addr))?
            .to_u8();
        if byte == 0 {
            break;
        }
        buffer.push(byte);
        addr += 1;
    }

    Ok(Bytes::from(buffer))
}

impl<'a, Mac: SupportMachine, DL: CellDataProvider> Syscalls<Mac> for Exec<'a, DL> {
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != EXEC {
            return Ok(false);
        }

        let index = machine.registers()[A0].to_u64();
        let source = Source::parse_from_u64(machine.registers()[A1].to_u64())?;
        let place = Place::parse_from_u64(machine.registers()[A2].to_u64())?;
        let bounds = machine.registers()[A3].to_u64();
        let offset = (bounds >> 32) as usize;
        let length = bounds as u32 as usize;

        let data = match place {
            Place::CellData => {
                let cell = self.fetch_cell(source, index as usize);
                if let Err(err) = cell {
                    machine.set_register(A0, Mac::REG::from_u8(err));
                    return Ok(true);
                }
                let cell = cell.unwrap();
                self.data_loader.load_cell_data(cell).ok_or_else(|| {
                    VMError::Unexpected(format!(
                        "Unexpected load_cell_data failed {}",
                        cell.out_point,
                    ))
                })?
            }
            Place::Witness => {
                let witness = self.fetch_witness(source, index as usize);
                if let Err(err) = witness {
                    machine.set_register(A0, Mac::REG::from_u8(err));
                    return Ok(true);
                }
                let witness = witness.unwrap();
                witness.raw_data()
            }
        };
        let data_size = data.len();
        if offset >= data_size {
            machine.set_register(A0, Mac::REG::from_u8(SLICE_OUT_OF_BOUND));
            return Ok(true);
        };
        let data = if length == 0 {
            data.slice(offset..data_size)
        } else {
            let end = offset.checked_add(length).ok_or(VMError::MemOutOfBound)?;
            if end > data_size {
                machine.set_register(A0, Mac::REG::from_u8(SLICE_OUT_OF_BOUND));
                return Ok(true);
            }
            data.slice(offset..end)
        };
        let argc = machine.registers()[A4].to_u64();
        let mut addr = machine.registers()[A5].to_u64();
        let mut argv = Vec::new();
        for _ in 0..argc {
            let target_addr = machine
                .memory_mut()
                .load64(&Mac::REG::from_u64(addr))?
                .to_u64();

            let cstr = load_c_string(machine, target_addr)?;
            argv.push(cstr);
            addr += 8;
        }

        let cycles = machine.cycles();
        let max_cycles = machine.max_cycles();
        machine.reset(max_cycles);
        machine.set_cycles(cycles);

        match machine.load_elf(&data, true) {
            Ok(size) => {
                machine.add_cycles_no_checking(transferred_byte_cycles(size))?;
            }
            Err(_) => {
                machine.set_register(A0, Mac::REG::from_u8(WRONG_FORMAT));
                return Ok(true);
            }
        }

        match machine.initialize_stack(
            &argv,
            (RISCV_MAX_MEMORY - DEFAULT_STACK_SIZE) as u64,
            DEFAULT_STACK_SIZE as u64,
        ) {
            Ok(size) => {
                machine.add_cycles_no_checking(transferred_byte_cycles(size))?;
            }
            Err(_) => {
                machine.set_register(A0, Mac::REG::from_u8(WRONG_FORMAT));
                return Ok(true);
            }
        }
        Ok(true)
    }
}
