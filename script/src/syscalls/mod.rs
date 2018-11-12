mod builder;
mod fetch_script_hash;
mod mmap_cell;
mod mmap_tx;

pub use self::builder::build_tx;
pub use self::fetch_script_hash::FetchScriptHash;
pub use self::mmap_cell::MmapCell;
pub use self::mmap_tx::MmapTx;

use vm::Error;

pub const SUCCESS: u8 = 0;
pub const OVERRIDE_LEN: u8 = 1;
pub const ITEM_MISSING: u8 = 2;

pub const MMAP_TX_SYSCALL_NUMBER: u64 = 2049;
pub const MMAP_CELL_SYSCALL_NUMBER: u64 = 2050;
pub const FETCH_SCRIPT_HASH_SYSCALL_NUMBER: u64 = 2051;

#[derive(Debug, PartialEq, Clone, Copy, Eq)]
pub enum Mode {
    ALL,
    PARTIAL,
}

impl Mode {
    pub fn parse_from_flag(flag: u64) -> Result<Mode, Error> {
        match flag {
            0 => Ok(Mode::ALL),
            1 => Ok(Mode::PARTIAL),
            _ => Err(Error::ParseError),
        }
    }
}

#[derive(Debug, PartialEq, Clone, Copy, Eq)]
pub enum Category {
    LOCK,
    CONTRACT,
}

impl Category {
    pub fn parse_from_u64(i: u64) -> Result<Category, Error> {
        match i {
            0 => Ok(Category::LOCK),
            1 => Ok(Category::CONTRACT),
            _ => Err(Error::ParseError),
        }
    }
}

#[derive(Debug, PartialEq, Clone, Copy, Eq)]
enum Source {
    INPUT,
    OUTPUT,
}

impl Source {
    fn parse_from_u64(i: u64) -> Result<Source, Error> {
        match i {
            0 => Ok(Source::INPUT),
            1 => Ok(Source::OUTPUT),
            _ => Err(Error::ParseError),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bigint::H256;
    use core::script::Script;
    use core::transaction::{CellInput, CellOutput, OutPoint};
    use proptest::collection::size_range;
    use proptest::prelude::any_with;
    use vm::machine::DefaultCoreMachine;
    use vm::{
        CoreMachine, Error as VMError, Memory, SparseMemory, Syscalls, A0, A1, A2, A3, A4, A5, A7,
    };

    fn _test_mmap_tx_all(tx: &Vec<u8>) {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory>::default();
        let size_addr = 0;
        let addr = 100;

        machine.registers_mut()[A0] = addr; // addr
        machine.registers_mut()[A1] = size_addr; // size_addr
        machine.registers_mut()[A2] = 0; // mode: all
        machine.registers_mut()[A7] = MMAP_TX_SYSCALL_NUMBER; // syscall number

        assert!(
            machine
                .memory_mut()
                .store64(size_addr as usize, tx.len() as u64)
                .is_ok()
        );

        let mut mmap_tx = MmapTx::new(tx);
        assert!(mmap_tx.ecall(&mut machine).is_ok());

        assert_eq!(machine.registers()[A0], SUCCESS as u64);
        for (i, addr) in (addr as usize..addr as usize + tx.len()).enumerate() {
            assert_eq!(machine.memory_mut().load8(addr), Ok(tx[i]))
        }

        // clean memory
        assert!(machine.memory_mut().munmap(0, 1100).is_ok());

        // test all mode execute with wrong len
        // reset register A0
        machine.registers_mut()[A0] = addr; // addr
        let len = tx.len() as u64 - 100;

        // write len - 100
        assert!(
            machine
                .memory_mut()
                .store64(size_addr as usize, len)
                .is_ok()
        );

        assert!(mmap_tx.ecall(&mut machine).is_ok());
        assert_eq!(machine.registers()[A0], OVERRIDE_LEN as u64);
        assert_eq!(
            machine.memory_mut().load64(size_addr as usize),
            Ok(tx.len() as u64)
        );
        for (i, addr) in (addr as usize..addr as usize + tx.len()).enumerate() {
            assert_eq!(machine.memory_mut().load8(addr), Ok(tx[i]))
        }
    }

    proptest! {
        #[test]
        fn test_mmap_tx_all(ref tx in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_mmap_tx_all(tx);
        }
    }

    fn _test_mmap_tx_partial(tx: &Vec<u8>) {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory>::default();
        let size_addr = 0;
        let addr = 100;
        let offset = 100usize;

        machine.registers_mut()[A0] = addr; // addr
        machine.registers_mut()[A1] = size_addr; // size_addr
        machine.registers_mut()[A2] = 1; // mode: partial
        machine.registers_mut()[A3] = offset as u64; // offset
        machine.registers_mut()[A7] = MMAP_TX_SYSCALL_NUMBER; // syscall number

        assert!(
            machine
                .memory_mut()
                .store64(size_addr as usize, tx.len() as u64)
                .is_ok()
        );

        let mut mmap_tx = MmapTx::new(tx);
        assert!(mmap_tx.ecall(&mut machine).is_ok());

        assert_eq!(machine.registers()[A0], SUCCESS as u64);
        assert_eq!(
            machine.memory_mut().load64(size_addr as usize),
            Ok((tx.len() - offset) as u64)
        );
        for (i, addr) in (addr as usize..addr as usize + tx.len() - offset).enumerate() {
            assert_eq!(machine.memory_mut().load8(addr), Ok(tx[i + offset]))
        }
    }

    proptest! {
        #[test]
        fn test_mmap_tx_partial(ref tx in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_mmap_tx_partial(tx);
        }
    }

    fn _test_mmap_cell_out_of_bound(data: Vec<u8>) {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory>::default();
        let size_addr = 0;
        let addr = 100;

        machine.registers_mut()[A0] = addr; // addr
        machine.registers_mut()[A1] = size_addr; // size_addr
        machine.registers_mut()[A2] = 0; // mode: all
        machine.registers_mut()[A4] = 1; //index
        machine.registers_mut()[A5] = 0; //source: 0 input
        machine.registers_mut()[A7] = MMAP_CELL_SYSCALL_NUMBER; // syscall number

        assert!(
            machine
                .memory_mut()
                .store64(size_addr as usize, data.len() as u64)
                .is_ok()
        );

        let output = CellOutput::new(100, data.clone(), H256::zero(), None);
        let input_cell = CellOutput::new(
            100,
            data.iter().rev().cloned().collect(),
            H256::zero(),
            None,
        );
        let outputs = vec![&output];
        let input_cells = vec![&input_cell];
        let mut mmap_cell = MmapCell::new(&outputs, &input_cells);

        assert_eq!(mmap_cell.ecall(&mut machine), Err(VMError::ParseError)); // index out of bounds
    }

    proptest! {
        #[test]
        fn test_mmap_cell_out_of_bound(data in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_mmap_cell_out_of_bound(data);
        }
    }

    fn _test_mmap_cell_all(data: Vec<u8>) {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory>::default();
        let size_addr = 0;
        let addr = 100;

        machine.registers_mut()[A0] = addr; // addr
        machine.registers_mut()[A1] = size_addr; // size_addr
        machine.registers_mut()[A2] = 0; // mode: all
        machine.registers_mut()[A4] = 0; //index
        machine.registers_mut()[A5] = 0; //source: 0 input
        machine.registers_mut()[A7] = MMAP_CELL_SYSCALL_NUMBER; // syscall number

        assert!(
            machine
                .memory_mut()
                .store64(size_addr as usize, data.len() as u64)
                .is_ok()
        );

        let output = CellOutput::new(100, data.clone(), H256::zero(), None);
        let input_cell = CellOutput::new(
            100,
            data.iter().rev().cloned().collect(),
            H256::zero(),
            None,
        );
        let outputs = vec![&output];
        let input_cells = vec![&input_cell];
        let mut mmap_cell = MmapCell::new(&outputs, &input_cells);

        // test input
        assert!(mmap_cell.ecall(&mut machine).is_ok());
        assert_eq!(machine.registers()[A0], SUCCESS as u64);

        for (i, addr) in (addr as usize..addr as usize + data.len()).enumerate() {
            assert_eq!(machine.memory_mut().load8(addr), Ok(input_cell.data[i]))
        }

        // clean memory
        assert!(machine.memory_mut().munmap(0, 1100).is_ok());

        // test output
        machine.registers_mut()[A0] = addr; // addr
        machine.registers_mut()[A5] = 1; //source: 1 output
        assert!(mmap_cell.ecall(&mut machine).is_ok());
        assert_eq!(machine.registers()[A0], SUCCESS as u64);

        for (i, addr) in (addr as usize..addr as usize + data.len()).enumerate() {
            assert_eq!(machine.memory_mut().load8(addr), Ok(output.data[i]))
        }

        // clean memory
        // test all mode execute with wrong len
        // write len - 100
        assert!(machine.memory_mut().munmap(0, 1100).is_ok());
        machine.registers_mut()[A0] = addr; // addr
        let len = data.len() as u64 - 100;
        assert!(
            machine
                .memory_mut()
                .store64(size_addr as usize, len)
                .is_ok()
        );
        assert!(mmap_cell.ecall(&mut machine).is_ok());
        assert_eq!(
            machine.memory_mut().load64(size_addr as usize),
            Ok((data.len()) as u64)
        );
        assert_eq!(machine.registers()[A0], OVERRIDE_LEN as u64);
    }

    proptest! {
        #[test]
        fn test_mmap_cell_all(tx in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_mmap_cell_all(tx);
        }
    }

    fn _test_mmap_cell_partial(data: Vec<u8>) {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory>::default();
        let size_addr = 0;
        let addr = 100;
        let offset = 100usize;

        machine.registers_mut()[A0] = addr; // addr
        machine.registers_mut()[A1] = size_addr; // size_addr
        machine.registers_mut()[A2] = 1; // mode: partial
        machine.registers_mut()[A3] = offset as u64; // offset
        machine.registers_mut()[A4] = 0; // index
        machine.registers_mut()[A5] = 0; // source: 0 input
        machine.registers_mut()[A7] = MMAP_CELL_SYSCALL_NUMBER; // syscall number

        assert!(
            machine
                .memory_mut()
                .store64(size_addr as usize, data.len() as u64)
                .is_ok()
        );

        let output = CellOutput::new(100, data.clone(), H256::zero(), None);
        let input_cell = CellOutput::new(
            100,
            data.iter().rev().cloned().collect(),
            H256::zero(),
            None,
        );
        let outputs = vec![&output];
        let input_cells = vec![&input_cell];
        let mut mmap_cell = MmapCell::new(&outputs, &input_cells);

        assert!(mmap_cell.ecall(&mut machine).is_ok());
        assert_eq!(machine.registers()[A0], SUCCESS as u64);

        for (i, addr) in (addr as usize..addr as usize + data.len() - offset).enumerate() {
            assert_eq!(
                machine.memory_mut().load8(addr),
                Ok(input_cell.data[i + offset])
            )
        }
    }

    proptest! {
        #[test]
        fn test_mmap_cell_partial(data in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_mmap_cell_partial(data);
        }
    }

    fn _test_fetch_script_hash_input_lock(data: Vec<u8>) {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory>::default();
        let size_addr = 0;
        let addr = 100;

        machine.registers_mut()[A0] = addr; // addr
        machine.registers_mut()[A1] = size_addr; // size_addr
        machine.registers_mut()[A2] = 0; // index
        machine.registers_mut()[A3] = 0; // source: 0 input
        machine.registers_mut()[A4] = 0; // category: 0 lock
        machine.registers_mut()[A7] = FETCH_SCRIPT_HASH_SYSCALL_NUMBER; // syscall number

        assert!(machine.memory_mut().store64(size_addr as usize, 32).is_ok());

        let script = Script::new(0, Vec::new(), None, Some(data), Vec::new());
        let input = CellInput::new(OutPoint::default(), script.clone());
        let inputs = vec![&input];
        let input_cells = Vec::new();
        let outputs = Vec::new();

        let mut fetch_script_hash = FetchScriptHash::new(&outputs, &inputs, &input_cells);

        assert!(fetch_script_hash.ecall(&mut machine).is_ok());
        assert_eq!(machine.registers()[A0], SUCCESS as u64);

        let hash = &script.type_hash();
        for (i, addr) in (addr as usize..addr as usize + hash.len()).enumerate() {
            assert_eq!(machine.memory_mut().load8(addr), Ok(hash[i]))
        }
    }

    proptest! {
        #[test]
        fn test_fetch_script_hash_input_lock(data in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_fetch_script_hash_input_lock(data);
        }
    }

    fn _test_fetch_script_hash_input_contract(data: Vec<u8>) {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory>::default();
        let size_addr = 0;
        let addr = 100;

        machine.registers_mut()[A0] = addr; // addr
        machine.registers_mut()[A1] = size_addr; // size_addr
        machine.registers_mut()[A2] = 0; // index
        machine.registers_mut()[A3] = 0; // source: 0 input
        machine.registers_mut()[A4] = 1; // category: 1 contract
        machine.registers_mut()[A7] = FETCH_SCRIPT_HASH_SYSCALL_NUMBER; // syscall number

        assert!(machine.memory_mut().store64(size_addr as usize, 32).is_ok());

        let script = Script::new(0, Vec::new(), None, Some(data), Vec::new());
        let output = CellOutput::new(0, Vec::new(), H256::from(0), Some(script.clone()));
        let inputs = Vec::new();
        let input_cells = vec![&output];
        let outputs = Vec::new();

        let mut fetch_script_hash = FetchScriptHash::new(&outputs, &inputs, &input_cells);

        assert!(fetch_script_hash.ecall(&mut machine).is_ok());
        assert_eq!(machine.registers()[A0], SUCCESS as u64);

        let hash = &script.type_hash();
        for (i, addr) in (addr as usize..addr as usize + hash.len()).enumerate() {
            assert_eq!(machine.memory_mut().load8(addr), Ok(hash[i]))
        }
    }

    proptest! {
        #[test]
        fn test_fetch_script_hash_input_contract(data in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_fetch_script_hash_input_contract(data);
        }
    }

    fn _test_fetch_script_hash_output_contract(data: Vec<u8>) {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory>::default();
        let size_addr = 0;
        let addr = 100;

        machine.registers_mut()[A0] = addr; // addr
        machine.registers_mut()[A1] = size_addr; // size_addr
        machine.registers_mut()[A2] = 0; // index
        machine.registers_mut()[A3] = 1; // source: 1 output
        machine.registers_mut()[A4] = 1; // category: 1 contract
        machine.registers_mut()[A7] = FETCH_SCRIPT_HASH_SYSCALL_NUMBER; // syscall number

        assert!(machine.memory_mut().store64(size_addr as usize, 32).is_ok());

        let script = Script::new(0, Vec::new(), None, Some(data), Vec::new());
        let output = CellOutput::new(0, Vec::new(), H256::from(0), Some(script.clone()));
        let inputs = Vec::new();
        let input_cells = Vec::new();
        let outputs = vec![&output];

        let mut fetch_script_hash = FetchScriptHash::new(&outputs, &inputs, &input_cells);

        assert!(fetch_script_hash.ecall(&mut machine).is_ok());
        assert_eq!(machine.registers()[A0], SUCCESS as u64);

        let hash = &script.type_hash();
        for (i, addr) in (addr as usize..addr as usize + hash.len()).enumerate() {
            assert_eq!(machine.memory_mut().load8(addr), Ok(hash[i]))
        }
    }

    proptest! {
        #[test]
        fn test_fetch_script_hash_output_contract(data in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_fetch_script_hash_output_contract(data);
        }
    }

    fn _test_fetch_script_hash_not_enough_space(data: Vec<u8>) {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory>::default();
        let size_addr = 0;
        let addr = 100;

        machine.registers_mut()[A0] = addr; // addr
        machine.registers_mut()[A1] = size_addr; // size_addr
        machine.registers_mut()[A2] = 0; // index
        machine.registers_mut()[A3] = 1; // source: 1 output
        machine.registers_mut()[A4] = 1; // category: 1 contract
        machine.registers_mut()[A7] = FETCH_SCRIPT_HASH_SYSCALL_NUMBER; // syscall number

        assert!(machine.memory_mut().store64(size_addr as usize, 16).is_ok());

        let script = Script::new(0, Vec::new(), None, Some(data), Vec::new());
        let output = CellOutput::new(0, Vec::new(), H256::from(0), Some(script.clone()));
        let inputs = Vec::new();
        let input_cells = Vec::new();
        let outputs = vec![&output];

        let mut fetch_script_hash = FetchScriptHash::new(&outputs, &inputs, &input_cells);

        assert!(fetch_script_hash.ecall(&mut machine).is_ok());
        assert_eq!(machine.registers()[A0], OVERRIDE_LEN as u64);

        assert_eq!(machine.memory_mut().load64(size_addr as usize), Ok(32));
    }

    proptest! {
        #[test]
        fn test_fetch_script_hash_not_enough_space(data in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_fetch_script_hash_not_enough_space(data);
        }
    }

    #[test]
    fn test_fetch_script_hash_missing_item() {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory>::default();
        let size_addr = 0;
        let addr = 100;

        machine.registers_mut()[A0] = addr; // addr
        machine.registers_mut()[A1] = size_addr; // size_addr
        machine.registers_mut()[A2] = 0; // index
        machine.registers_mut()[A3] = 1; // source: 1 output
        machine.registers_mut()[A4] = 1; // category: 1 contract
        machine.registers_mut()[A7] = FETCH_SCRIPT_HASH_SYSCALL_NUMBER; // syscall number

        assert!(machine.memory_mut().store64(size_addr as usize, 16).is_ok());

        let inputs = Vec::new();
        let input_cells = Vec::new();
        let outputs = Vec::new();

        let mut fetch_script_hash = FetchScriptHash::new(&outputs, &inputs, &input_cells);

        assert!(fetch_script_hash.ecall(&mut machine).is_ok());
        assert_eq!(machine.registers()[A0], ITEM_MISSING as u64);
    }
}
