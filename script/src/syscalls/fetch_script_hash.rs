use bigint::H256;
use core::transaction::{CellInput, CellOutput};
use syscalls::{
    Category, Source, FETCH_SCRIPT_HASH_SYSCALL_NUMBER, ITEM_MISSING, OVERRIDE_LEN, SUCCESS,
};
use vm::{CoreMachine, Error as VMError, Memory, Register, Syscalls, A0, A1, A2, A3, A4, A7};

#[derive(Debug)]
pub struct FetchScriptHash<'a> {
    outputs: &'a [&'a CellOutput],
    inputs: &'a [&'a CellInput],
    input_cells: &'a [&'a CellOutput],
}

impl<'a> FetchScriptHash<'a> {
    pub fn new(
        outputs: &'a [&'a CellOutput],
        inputs: &'a [&'a CellInput],
        input_cells: &'a [&'a CellOutput],
    ) -> FetchScriptHash<'a> {
        FetchScriptHash {
            outputs,
            inputs,
            input_cells,
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
            (Source::OUTPUT, Category::LOCK) => None,
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
        if machine.registers()[A7].to_u64() != FETCH_SCRIPT_HASH_SYSCALL_NUMBER {
            return Ok(false);
        }

        let addr = machine.registers()[A0].to_usize();
        let size_addr = machine.registers()[A1].to_usize();
        let size = machine.memory_mut().load64(size_addr)? as usize;

        let index = machine.registers()[A2].to_usize();
        let source = Source::parse_from_u64(machine.registers()[A3].to_u64())?;
        let category = Category::parse_from_u64(machine.registers()[A4].to_u64())?;

        match self.fetch_hash(source, category, index) {
            Some(hash) => {
                let hash: &[u8] = &hash;
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
