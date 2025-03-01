use crate::syscalls::DEBUG_PAUSE;
use ckb_vm::{Error as VMError, Register, SupportMachine, Syscalls, registers::A7};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Debug)]
pub struct Pause {
    skip: Arc<AtomicBool>,
}

impl Pause {
    pub fn new(skip: Arc<AtomicBool>) -> Self {
        Self { skip }
    }
}

impl<Mac: SupportMachine> Syscalls<Mac> for Pause {
    fn initialize(&mut self, _machine: &mut Mac) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut Mac) -> Result<bool, VMError> {
        if machine.registers()[A7].to_u64() != DEBUG_PAUSE {
            return Ok(false);
        }
        if self.skip.load(Ordering::SeqCst) {
            return Ok(true);
        }
        // Note(yukang): this syscall is used for tests and debugging,
        // old verify and tests logic use VMInternalError::CyclesExceeded as a flag to Suspend,
        // in new verify VMInternalError::CycleExceeded is used to indicate cycles exceeded error only,
        // VMInternalError::Pause is used to indicate the script execution should be paused.
        // To keep compatibility with old tests, we should change to use VMInternalError::Pause
        // and use VMInternalError::CyclesExceeded | VMInternalError::Pause as a flag to Suspend in tests code.
        Err(VMError::Pause)
    }
}
