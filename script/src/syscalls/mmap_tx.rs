use std::cmp;
use std::rc::Rc;
use syscalls::{Mode, MMAP_TX_SYSCALL_NUMBER, OVERRIDE_LEN, SUCCESS};
use vm::memory::PROT_READ;
use vm::{CoreMachine, Error as VMError, Memory, Register, Syscalls, A0, A1, A2, A3, A7};

pub struct MmapTx<'a> {
    tx: &'a [u8],
}

impl<'a> MmapTx<'a> {
    pub fn new(tx: &'a [u8]) -> MmapTx<'a> {
        MmapTx { tx }
    }
}

impl<'a, R: Register, M: Memory> Syscalls<R, M> for MmapTx<'a> {
    fn initialize(&mut self, _machine: &mut CoreMachine<R, M>) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut CoreMachine<R, M>) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != MMAP_TX_SYSCALL_NUMBER {
            return Ok(false);
        }

        let addr = machine.registers()[A0].to_usize();
        let size_addr = machine.registers()[A1].to_usize();
        let mode = Mode::parse_from_flag(machine.registers()[A2].to_u64())?;
        let size = machine.memory_mut().load64(size_addr)? as usize;

        let data = self.tx;

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
