use core::transaction::CellOutput;
use std::cmp;
use std::rc::Rc;
use syscalls::{Mode, MMAP_CELL_SYSCALL_NUMBER, OVERRIDE_LEN, SUCCESS};
use vm::memory::PROT_READ;
use vm::{CoreMachine, Error as VMError, Memory, Register, Syscalls, A0, A1, A2, A3, A4, A5, A7};

#[derive(Debug, PartialEq, Clone, Copy, Eq)]
enum Source {
    INPUT,
    OUTPUT,
}

impl Source {
    fn parse_from_u64(i: u64) -> Result<Source, VMError> {
        match i {
            0 => Ok(Source::INPUT),
            1 => Ok(Source::OUTPUT),
            _ => Err(VMError::ParseError),
        }
    }
}

#[derive(Debug)]
pub struct MmapCell<'a> {
    outputs: &'a [&'a CellOutput],
    input_cells: &'a [&'a CellOutput],
}

impl<'a> MmapCell<'a> {
    pub fn new(outputs: &'a [&'a CellOutput], input_cells: &'a [&'a CellOutput]) -> MmapCell<'a> {
        MmapCell {
            outputs,
            input_cells,
        }
    }

    fn read_data(&self, source: Source, index: usize) -> Option<&'a [u8]> {
        match source {
            Source::INPUT => self.input_cells.get(index).map(|output| &output.data[..]),
            Source::OUTPUT => self.outputs.get(index).map(|output| &output.data[..]),
        }
    }
}

impl<'a, R: Register, M: Memory> Syscalls<R, M> for MmapCell<'a> {
    fn initialize(&mut self, _machine: &mut CoreMachine<R, M>) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut CoreMachine<R, M>) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != MMAP_CELL_SYSCALL_NUMBER {
            return Ok(false);
        }

        let addr = machine.registers()[A0].to_usize();
        let size_addr = machine.registers()[A1].to_usize();
        let mode = Mode::parse_from_flag(machine.registers()[A2].to_u64())?;

        let index = machine.registers()[A4].to_usize();
        let source = Source::parse_from_u64(machine.registers()[A5].to_u64())?;
        let size = machine.memory_mut().load64(size_addr)? as usize;

        let data = self
            .read_data(source, index)
            .ok_or_else(|| VMError::ParseError)?;

        let (size, offset) = match mode {
            Mode::ALL => {
                if size < data.len() {
                    machine.memory_mut().store64(size_addr, data.len() as u64)?;
                    machine.registers_mut()[A0] = R::from_u8(OVERRIDE_LEN);
                } else {
                    machine.registers_mut()[A0] = R::from_u8(SUCCESS);
                }
                (data.len(), 0)
            }
            Mode::PARTIAL => {
                let offset = machine.registers()[A3].to_usize();
                let real_size = cmp::min(size, data.len() - offset);
                machine.memory_mut().store64(size_addr, real_size as u64)?;
                machine.registers_mut()[A0] = R::from_u8(SUCCESS);
                (real_size, offset)
            }
        };

        machine.memory_mut().mmap(
            addr,
            size,
            PROT_READ,
            Some(Rc::new(data.to_vec().into_boxed_slice())),
            offset,
        )?;
        Ok(true)
    }
}
