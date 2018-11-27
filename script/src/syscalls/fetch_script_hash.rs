use ckb_core::transaction::{CellInput, CellOutput};
use ckb_vm::{CoreMachine, Error as VMError, Memory, Register, Syscalls, A0, A1, A2, A3, A4, A7};
use numext_fixed_hash::H256;
use syscalls::{
    Category, Source, FETCH_CURRENT_SCRIPT_HASH_SYSCALL_NUMBER, FETCH_SCRIPT_HASH_SYSCALL_NUMBER,
    ITEM_MISSING, OVERRIDE_LEN, SUCCESS,
};

#[derive(Debug)]
pub struct FetchScriptHash<'a> {
    outputs: &'a [&'a CellOutput],
    inputs: &'a [&'a CellInput],
    input_cells: &'a [&'a CellOutput],
    current_script_hash: H256,
}

impl<'a> FetchScriptHash<'a> {
    pub fn new(
        outputs: &'a [&'a CellOutput],
        inputs: &'a [&'a CellInput],
        input_cells: &'a [&'a CellOutput],
        current_script_hash: H256,
    ) -> FetchScriptHash<'a> {
        FetchScriptHash {
            outputs,
            inputs,
            input_cells,
            current_script_hash,
        }
    }

    fn fetch_hash(&self, source: Source, category: Category, index: usize) -> Option<H256> {
        match (source, category) {
            (Source::INPUT, Category::LOCK) => {
                self.inputs.get(index).map(|input| input.unlock.type_hash())
            }
            (Source::INPUT, Category::CONTRACT) => {
                self.input_cells.get(index).and_then(|input_cell| {
                    input_cell
                        .contract
                        .as_ref()
                        .map(|contract| contract.type_hash())
                })
            }
            (Source::OUTPUT, Category::LOCK) => {
                self.outputs.get(index).map(|output| output.lock.clone())
            }
            (Source::OUTPUT, Category::CONTRACT) => self.outputs.get(index).and_then(|output| {
                output
                    .contract
                    .as_ref()
                    .map(|contract| contract.type_hash())
            }),
        }
    }
}

impl<'a, R: Register, M: Memory> Syscalls<R, M> for FetchScriptHash<'a> {
    fn initialize(&mut self, _machine: &mut CoreMachine<R, M>) -> Result<(), VMError> {
        Ok(())
    }

    fn ecall(&mut self, machine: &mut CoreMachine<R, M>) -> Result<bool, VMError> {
        let code = machine.registers()[A7].to_u64();
        if code != FETCH_SCRIPT_HASH_SYSCALL_NUMBER
            && code != FETCH_CURRENT_SCRIPT_HASH_SYSCALL_NUMBER
        {
            return Ok(false);
        }

        let addr = machine.registers()[A0].to_usize();
        let size_addr = machine.registers()[A1].to_usize();
        let size = machine.memory_mut().load64(size_addr)? as usize;

        let hash = if code == FETCH_SCRIPT_HASH_SYSCALL_NUMBER {
            let index = machine.registers()[A2].to_usize();
            let source = Source::parse_from_u64(machine.registers()[A3].to_u64())?;
            let category = Category::parse_from_u64(machine.registers()[A4].to_u64())?;
            self.fetch_hash(source, category, index)
        } else {
            Some(self.current_script_hash.clone())
        };

        match hash {
            Some(hash) => {
                let hash: &[u8] = hash.as_bytes();
                machine.memory_mut().store64(size_addr, hash.len() as u64)?;
                if size >= hash.len() {
                    machine.memory_mut().store_bytes(addr, hash)?;
                    machine.registers_mut()[A0] = R::from_u8(SUCCESS);
                } else {
                    machine.registers_mut()[A0] = R::from_u8(OVERRIDE_LEN);
                }
            }
            None => {
                machine.registers_mut()[A0] = R::from_u8(ITEM_MISSING);
            }
        };
        Ok(true)
    }
}
