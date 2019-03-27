use crate::syscalls::{utils::store_data, ITEM_MISSING, LOAD_EMBED_SYSCALL_NUMBER, SUCCESS};
use ckb_vm::{CoreMachine, Error as VMError, Memory, Register, Syscalls, A0, A3, A7};

#[derive(Debug)]
pub struct LoadEmbed<'a> {
    embeds: &'a [&'a Vec<u8>],
}

impl<'a> LoadEmbed<'a> {
    pub fn new(embeds: &'a [&'a Vec<u8>]) -> Self {
        LoadEmbed { embeds }
    }
}

impl<'a, R: Register, M: Memory> Syscalls<R, M> for LoadEmbed<'a> {
    fn initialize(&mut self, _machine: &mut CoreMachine<R, M>) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut CoreMachine<R, M>) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != LOAD_EMBED_SYSCALL_NUMBER {
            return Ok(false);
        }
        machine.add_cycles(100);

        let index = machine.registers()[A3].to_usize();
        let length = match self.embeds.get(index) {
            Some(data) => {
                store_data(machine, &data)?;
                machine.registers_mut()[A0] = R::from_u8(SUCCESS);
                data.len()
            }
            None => {
                machine.registers_mut()[A0] = R::from_u8(ITEM_MISSING);
                0
            }
        };

        machine.add_cycles(length as u64 * 10);
        Ok(true)
    }
}
