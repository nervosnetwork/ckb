mod builder;
mod debugger;
mod load_cell;
mod load_cell_by_field;
mod load_input_by_field;
mod load_tx;
mod utils;

pub use self::builder::build_tx;
pub use self::debugger::Debugger;
pub use self::load_cell::LoadCell;
pub use self::load_cell_by_field::LoadCellByField;
pub use self::load_input_by_field::LoadInputByField;
pub use self::load_tx::LoadTx;

use ckb_vm::Error;

pub const SUCCESS: u8 = 0;
pub const ITEM_MISSING: u8 = 2;

pub const LOAD_TX_SYSCALL_NUMBER: u64 = 2049;
pub const LOAD_CELL_SYSCALL_NUMBER: u64 = 2053;
pub const LOAD_CELL_BY_FIELD_SYSCALL_NUMBER: u64 = 2054;
pub const LOAD_INPUT_BY_FIELD_SYSCALL_NUMBER: u64 = 2055;
pub const DEBUG_PRINT_SYSCALL_NUMBER: u64 = 2177;

#[derive(Debug, PartialEq, Clone, Copy, Eq)]
enum CellField {
    Capacity,
    Data,
    LockHash,
    Contract,
    ContractHash,
}

impl CellField {
    fn parse_from_u64(i: u64) -> Result<CellField, Error> {
        match i {
            0 => Ok(CellField::Capacity),
            1 => Ok(CellField::Data),
            2 => Ok(CellField::LockHash),
            3 => Ok(CellField::Contract),
            4 => Ok(CellField::ContractHash),
            _ => Err(Error::ParseError),
        }
    }
}

#[derive(Debug, PartialEq, Clone, Copy, Eq)]
enum InputField {
    Unlock,
    OutPoint,
}

impl InputField {
    fn parse_from_u64(i: u64) -> Result<InputField, Error> {
        match i {
            0 => Ok(InputField::Unlock),
            1 => Ok(InputField::OutPoint),
            _ => Err(Error::ParseError),
        }
    }
}

#[derive(Debug, PartialEq, Clone, Copy, Eq)]
enum Source {
    Input,
    Output,
    Current,
}

impl Source {
    fn parse_from_u64(i: u64) -> Result<Source, Error> {
        match i {
            0 => Ok(Source::Input),
            1 => Ok(Source::Output),
            2 => Ok(Source::Current),
            _ => Err(Error::ParseError),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use byteorder::{LittleEndian, WriteBytesExt};
    use ckb_core::script::Script;
    use ckb_core::transaction::{CellInput, CellOutput, OutPoint};
    use ckb_protocol::{CellOutput as FbsCellOutput, OutPoint as FbsOutPoint, Script as FbsScript};
    use ckb_vm::machine::DefaultCoreMachine;
    use ckb_vm::{CoreMachine, Memory, SparseMemory, Syscalls, A0, A1, A2, A3, A4, A5, A7};
    use flatbuffers::FlatBufferBuilder;
    use hash::sha3_256;
    use numext_fixed_hash::H256;
    use proptest::{
        collection::size_range, prelude::any, prelude::any_with, proptest, proptest_helper,
    };

    fn _test_load_tx_all(tx: &Vec<u8>) {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory>::default();
        let size_addr = 0;
        let addr = 100;

        machine.registers_mut()[A0] = addr; // addr
        machine.registers_mut()[A1] = size_addr; // size_addr
        machine.registers_mut()[A2] = 0; // offset
        machine.registers_mut()[A7] = LOAD_TX_SYSCALL_NUMBER; // syscall number

        assert!(machine
            .memory_mut()
            .store64(size_addr as usize, tx.len() as u64)
            .is_ok());

        let mut load_tx = LoadTx::new(tx);
        assert!(load_tx.ecall(&mut machine).is_ok());

        assert_eq!(machine.registers()[A0], SUCCESS as u64);
        for (i, addr) in (addr as usize..addr as usize + tx.len()).enumerate() {
            assert_eq!(machine.memory_mut().load8(addr), Ok(tx[i]));
        }
    }

    proptest! {
        #[test]
        fn test_load_tx_all(ref tx in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_tx_all(tx);
        }
    }

    fn _test_load_tx_length(tx: &Vec<u8>) {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory>::default();
        let size_addr = 0;
        let addr = 100;

        machine.registers_mut()[A0] = addr; // addr
        machine.registers_mut()[A1] = size_addr; // size_addr
        machine.registers_mut()[A2] = 0; // offset
        machine.registers_mut()[A7] = LOAD_TX_SYSCALL_NUMBER; // syscall number

        assert!(machine.memory_mut().store64(size_addr as usize, 0).is_ok());

        let mut load_tx = LoadTx::new(tx);
        assert!(load_tx.ecall(&mut machine).is_ok());

        assert_eq!(machine.registers()[A0], SUCCESS as u64);
        assert_eq!(
            machine.memory_mut().load64(size_addr as usize),
            Ok(tx.len() as u64)
        );

        machine.registers_mut()[A0] = addr; // addr
        machine.registers_mut()[A2] = 100; // offset

        assert!(machine.memory_mut().store64(size_addr as usize, 0).is_ok());

        assert!(load_tx.ecall(&mut machine).is_ok());

        assert_eq!(machine.registers()[A0], SUCCESS as u64);
        assert_eq!(
            machine.memory_mut().load64(size_addr as usize),
            Ok(tx.len() as u64 - 100)
        );
    }

    proptest! {
        #[test]
        fn test_load_tx_length(ref tx in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_tx_length(tx);
        }
    }

    fn _test_load_tx_partial(tx: &Vec<u8>) {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory>::default();
        let size_addr = 0;
        let addr = 100;
        let offset = 100usize;

        machine.registers_mut()[A0] = addr; // addr
        machine.registers_mut()[A1] = size_addr; // size_addr
        machine.registers_mut()[A2] = offset as u64; // offset
        machine.registers_mut()[A7] = LOAD_TX_SYSCALL_NUMBER; // syscall number

        assert!(machine
            .memory_mut()
            .store64(size_addr as usize, tx.len() as u64)
            .is_ok());

        let mut load_tx = LoadTx::new(tx);
        assert!(load_tx.ecall(&mut machine).is_ok());

        assert_eq!(machine.registers()[A0], SUCCESS as u64);
        assert_eq!(
            machine.memory_mut().load64(size_addr as usize),
            Ok((tx.len() - offset) as u64)
        );
        for (i, addr) in (addr as usize..addr as usize + tx.len() - offset).enumerate() {
            assert_eq!(machine.memory_mut().load8(addr), Ok(tx[i + offset]));
        }
    }

    proptest! {
        #[test]
        fn test_load_tx_partial(ref tx in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_tx_partial(tx);
        }
    }

    fn _test_load_cell_item_missing(data: Vec<u8>) {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory>::default();
        let size_addr = 0;
        let addr = 100;

        machine.registers_mut()[A0] = addr; // addr
        machine.registers_mut()[A1] = size_addr; // size_addr
        machine.registers_mut()[A2] = 0; // offset
        machine.registers_mut()[A3] = 1; //index
        machine.registers_mut()[A4] = 0; //source: 0 input
        machine.registers_mut()[A7] = LOAD_CELL_SYSCALL_NUMBER; // syscall number

        assert!(machine
            .memory_mut()
            .store64(size_addr as usize, data.len() as u64)
            .is_ok());

        let output = CellOutput::new(100, data.clone(), H256::zero(), None);
        let input_cell = CellOutput::new(
            100,
            data.iter().rev().cloned().collect(),
            H256::zero(),
            None,
        );
        let outputs = vec![&output];
        let input_cells = vec![&input_cell];
        let mut load_cell = LoadCell::new(&outputs, &input_cells, &input_cell);

        assert!(load_cell.ecall(&mut machine).is_ok());
        assert_eq!(machine.registers()[A0], ITEM_MISSING as u64);
    }

    proptest! {
        #[test]
        fn test_load_cell_item_missing(data in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_cell_item_missing(data);
        }
    }

    fn _test_load_cell_all(data: Vec<u8>) {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory>::default();
        let size_addr = 0;
        let addr = 100;

        machine.registers_mut()[A0] = addr; // addr
        machine.registers_mut()[A1] = size_addr; // size_addr
        machine.registers_mut()[A2] = 0; // offset
        machine.registers_mut()[A3] = 0; //index
        machine.registers_mut()[A4] = 0; //source: 0 input
        machine.registers_mut()[A7] = LOAD_CELL_SYSCALL_NUMBER; // syscall number

        let output = CellOutput::new(100, data.clone(), H256::zero(), None);
        let input_cell = CellOutput::new(
            100,
            data.iter().rev().cloned().collect(),
            H256::zero(),
            None,
        );
        let outputs = vec![&output];
        let input_cells = vec![&input_cell];
        let mut load_cell = LoadCell::new(&outputs, &input_cells, &input_cell);

        let mut builder = FlatBufferBuilder::new();
        let fbs_offset = FbsCellOutput::build(&mut builder, &input_cell);
        builder.finish(fbs_offset, None);
        let input_correct_data = builder.finished_data();

        let mut builder = FlatBufferBuilder::new();
        let fbs_offset = FbsCellOutput::build(&mut builder, &output);
        builder.finish(fbs_offset, None);
        let output_correct_data = builder.finished_data();

        // test input
        assert!(machine
            .memory_mut()
            .store64(size_addr as usize, input_correct_data.len() as u64)
            .is_ok());

        assert!(load_cell.ecall(&mut machine).is_ok());
        assert_eq!(machine.registers()[A0], SUCCESS as u64);

        assert_eq!(
            machine.memory_mut().load64(size_addr as usize),
            Ok(input_correct_data.len() as u64)
        );

        for (i, addr) in (addr as usize..addr as usize + input_correct_data.len()).enumerate() {
            assert_eq!(machine.memory_mut().load8(addr), Ok(input_correct_data[i]));
        }

        // clean memory
        assert!(machine.memory_mut().store_byte(0, 1100, 0).is_ok());

        // test output
        machine.registers_mut()[A0] = addr; // addr
        machine.registers_mut()[A1] = size_addr; // size_addr
        machine.registers_mut()[A4] = 1; //source: 1 output
        assert!(machine
            .memory_mut()
            .store64(size_addr as usize, output_correct_data.len() as u64 + 10)
            .is_ok());

        assert!(load_cell.ecall(&mut machine).is_ok());
        assert_eq!(machine.registers()[A0], SUCCESS as u64);

        assert_eq!(
            machine.memory_mut().load64(size_addr as usize),
            Ok(output_correct_data.len() as u64)
        );

        for (i, addr) in (addr as usize..addr as usize + output_correct_data.len()).enumerate() {
            assert_eq!(machine.memory_mut().load8(addr), Ok(output_correct_data[i]));
        }
    }

    proptest! {
        #[test]
        fn test_load_cell_all(tx in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_cell_all(tx);
        }
    }

    fn _test_load_cell_length(data: Vec<u8>) {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory>::default();
        let size_addr = 0;
        let addr = 100;

        machine.registers_mut()[A0] = addr; // addr
        machine.registers_mut()[A1] = size_addr; // size_addr
        machine.registers_mut()[A2] = 0; // offset
        machine.registers_mut()[A3] = 0; //index
        machine.registers_mut()[A4] = 0; //source: 0 input
        machine.registers_mut()[A7] = LOAD_CELL_SYSCALL_NUMBER; // syscall number

        let output = CellOutput::new(100, data.clone(), H256::zero(), None);
        let input_cell = CellOutput::new(
            100,
            data.iter().rev().cloned().collect(),
            H256::zero(),
            None,
        );
        let outputs = vec![&output];
        let input_cells = vec![&input_cell];
        let mut load_cell = LoadCell::new(&outputs, &input_cells, &input_cell);

        let mut builder = FlatBufferBuilder::new();
        let fbs_offset = FbsCellOutput::build(&mut builder, &input_cell);
        builder.finish(fbs_offset, None);
        let input_correct_data = builder.finished_data();

        assert!(machine.memory_mut().store64(size_addr as usize, 0).is_ok());

        assert!(load_cell.ecall(&mut machine).is_ok());
        assert_eq!(machine.registers()[A0], SUCCESS as u64);

        assert_eq!(
            machine.memory_mut().load64(size_addr as usize),
            Ok(input_correct_data.len() as u64)
        );
    }

    proptest! {
        #[test]
        fn test_load_cell_length(tx in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_cell_length(tx);
        }
    }

    fn _test_load_cell_partial(data: Vec<u8>) {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory>::default();
        let size_addr = 0;
        let addr = 100;
        let offset = 100usize;

        machine.registers_mut()[A0] = addr; // addr
        machine.registers_mut()[A1] = size_addr; // size_addr
        machine.registers_mut()[A2] = offset as u64; // offset
        machine.registers_mut()[A3] = 0; // index
        machine.registers_mut()[A4] = 0; // source: 0 input
        machine.registers_mut()[A7] = LOAD_CELL_SYSCALL_NUMBER; // syscall number

        let output = CellOutput::new(100, data.clone(), H256::zero(), None);
        let input_cell = CellOutput::new(
            100,
            data.iter().rev().cloned().collect(),
            H256::zero(),
            None,
        );
        let outputs = vec![&output];
        let input_cells = vec![&input_cell];
        let mut load_cell = LoadCell::new(&outputs, &input_cells, &input_cell);

        let mut builder = FlatBufferBuilder::new();
        let fbs_offset = FbsCellOutput::build(&mut builder, &input_cell);
        builder.finish(fbs_offset, None);
        let input_correct_data = builder.finished_data();

        assert!(machine
            .memory_mut()
            .store64(size_addr as usize, input_correct_data.len() as u64)
            .is_ok());

        assert!(load_cell.ecall(&mut machine).is_ok());
        assert_eq!(machine.registers()[A0], SUCCESS as u64);

        for (i, addr) in
            (addr as usize..addr as usize + input_correct_data.len() - offset).enumerate()
        {
            assert_eq!(
                machine.memory_mut().load8(addr),
                Ok(input_correct_data[i + offset])
            );
        }
    }

    proptest! {
        #[test]
        fn test_load_cell_partial(data in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_cell_partial(data);
        }
    }

    fn _test_load_current_cell(data: Vec<u8>) {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory>::default();
        let size_addr = 0;
        let addr = 100;

        machine.registers_mut()[A0] = addr; // addr
        machine.registers_mut()[A1] = size_addr; // size_addr
        machine.registers_mut()[A2] = 0; // offset
        machine.registers_mut()[A3] = 1000; //index
        machine.registers_mut()[A4] = 2; //source: 2 self
        machine.registers_mut()[A7] = LOAD_CELL_SYSCALL_NUMBER; // syscall number

        let input_cell = CellOutput::new(
            100,
            data.iter().rev().cloned().collect(),
            H256::zero(),
            None,
        );
        let outputs = vec![];
        let input_cells = vec![];
        let mut load_cell = LoadCell::new(&outputs, &input_cells, &input_cell);

        let mut builder = FlatBufferBuilder::new();
        let fbs_offset = FbsCellOutput::build(&mut builder, &input_cell);
        builder.finish(fbs_offset, None);
        let input_correct_data = builder.finished_data();

        // test input
        assert!(machine
            .memory_mut()
            .store64(size_addr as usize, input_correct_data.len() as u64 + 5)
            .is_ok());

        assert!(load_cell.ecall(&mut machine).is_ok());
        assert_eq!(machine.registers()[A0], SUCCESS as u64);

        assert_eq!(
            machine.memory_mut().load64(size_addr as usize),
            Ok(input_correct_data.len() as u64)
        );

        for (i, addr) in (addr as usize..addr as usize + input_correct_data.len()).enumerate() {
            assert_eq!(machine.memory_mut().load8(addr), Ok(input_correct_data[i]));
        }
    }

    proptest! {
        #[test]
        fn test_load_current_cell(tx in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_current_cell(tx);
        }
    }

    fn _test_load_cell_capacity(capacity: u64) {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory>::default();
        let size_addr = 0;
        let addr = 100;

        machine.registers_mut()[A0] = addr; // addr
        machine.registers_mut()[A1] = size_addr; // size_addr
        machine.registers_mut()[A2] = 0; // offset
        machine.registers_mut()[A3] = 0; //index
        machine.registers_mut()[A4] = 0; //source: 0 input
        machine.registers_mut()[A5] = 0; //field: 0 capacity
        machine.registers_mut()[A7] = LOAD_CELL_BY_FIELD_SYSCALL_NUMBER; // syscall number

        let input_cell = CellOutput::new(capacity, vec![], H256::zero(), None);
        let outputs = vec![];
        let input_cells = vec![&input_cell];
        let mut load_cell = LoadCellByField::new(&outputs, &input_cells, &input_cell);

        assert!(machine.memory_mut().store64(size_addr as usize, 16).is_ok());

        assert!(load_cell.ecall(&mut machine).is_ok());
        assert_eq!(machine.registers()[A0], SUCCESS as u64);

        assert_eq!(machine.memory_mut().load64(size_addr as usize), Ok(8));

        let mut buffer = vec![];
        buffer.write_u64::<LittleEndian>(capacity).unwrap();

        for (i, addr) in (addr as usize..addr as usize + buffer.len() as usize).enumerate() {
            assert_eq!(machine.memory_mut().load8(addr), Ok(buffer[i]));
        }
    }

    proptest! {
        #[test]
        fn test_load_cell_capacity(capacity in any::<u64>()) {
            _test_load_cell_capacity(capacity);
        }
    }

    fn _test_load_self_lock_hash(data: Vec<u8>) {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory>::default();
        let size_addr = 0;
        let addr = 100;

        machine.registers_mut()[A0] = addr; // addr
        machine.registers_mut()[A1] = size_addr; // size_addr
        machine.registers_mut()[A2] = 0; // offset
        machine.registers_mut()[A3] = 0; //index
        machine.registers_mut()[A4] = 2; //source: 2 self
        machine.registers_mut()[A5] = 2; //field: 2 lock hash
        machine.registers_mut()[A7] = LOAD_CELL_BY_FIELD_SYSCALL_NUMBER; // syscall number

        let sha3_data = sha3_256(data);
        let input_cell = CellOutput::new(100, vec![], H256::from_slice(&sha3_data).unwrap(), None);
        let outputs = vec![];
        let input_cells = vec![];
        let mut load_cell = LoadCellByField::new(&outputs, &input_cells, &input_cell);

        assert!(machine.memory_mut().store64(size_addr as usize, 64).is_ok());

        assert!(load_cell.ecall(&mut machine).is_ok());
        assert_eq!(machine.registers()[A0], SUCCESS as u64);

        assert_eq!(
            machine.memory_mut().load64(size_addr as usize),
            Ok(sha3_data.len() as u64)
        );

        for (i, addr) in (addr as usize..addr as usize + sha3_data.len() as usize).enumerate() {
            assert_eq!(machine.memory_mut().load8(addr), Ok(sha3_data[i]));
        }

        machine.registers_mut()[A0] = addr; // addr
        assert!(machine.memory_mut().store64(size_addr as usize, 0).is_ok());

        assert!(load_cell.ecall(&mut machine).is_ok());
        assert_eq!(machine.registers()[A0], SUCCESS as u64);

        assert_eq!(
            machine.memory_mut().load64(size_addr as usize),
            Ok(sha3_data.len() as u64)
        );
    }

    proptest! {
        #[test]
        fn test_load_self_lock_hash(data in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_self_lock_hash(data);
        }
    }

    #[test]
    fn test_load_missing_contract() {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory>::default();
        let size_addr = 0;
        let addr = 100;

        machine.registers_mut()[A0] = addr; // addr
        machine.registers_mut()[A1] = size_addr; // size_addr
        machine.registers_mut()[A2] = 0; // offset
        machine.registers_mut()[A3] = 0; //index
        machine.registers_mut()[A4] = 1; //source: 1 output
        machine.registers_mut()[A5] = 3; //field: 3 contract
        machine.registers_mut()[A7] = LOAD_CELL_BY_FIELD_SYSCALL_NUMBER; // syscall number

        let output_cell = CellOutput::new(100, vec![], H256::default(), None);
        let outputs = vec![&output_cell];
        let input_cells = vec![];
        let mut load_cell = LoadCellByField::new(&outputs, &input_cells, &output_cell);

        assert!(machine
            .memory_mut()
            .store64(size_addr as usize, 100)
            .is_ok());

        assert!(load_cell.ecall(&mut machine).is_ok());
        assert_eq!(machine.registers()[A0], ITEM_MISSING as u64);

        assert_eq!(machine.memory_mut().load64(size_addr as usize), Ok(100));

        for addr in addr as usize..addr as usize + 100 {
            assert_eq!(machine.memory_mut().load8(addr), Ok(0));
        }
    }

    fn _test_load_input_unlock_script(data: Vec<u8>) {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory>::default();
        let size_addr = 0;
        let addr = 100;

        machine.registers_mut()[A0] = addr; // addr
        machine.registers_mut()[A1] = size_addr; // size_addr
        machine.registers_mut()[A2] = 0; // offset
        machine.registers_mut()[A3] = 0; //index
        machine.registers_mut()[A4] = 0; //source: 0 input
        machine.registers_mut()[A5] = 0; //field: 0 unlock
        machine.registers_mut()[A7] = LOAD_INPUT_BY_FIELD_SYSCALL_NUMBER; // syscall number

        let unlock = Script::new(0, vec![], None, Some(data), vec![]);
        let mut builder = FlatBufferBuilder::new();
        let fbs_offset = FbsScript::build(&mut builder, &unlock);
        builder.finish(fbs_offset, None);
        let unlock_data = builder.finished_data();

        let input = CellInput::new(OutPoint::default(), unlock);
        let inputs = vec![&input];
        let mut load_input = LoadInputByField::new(&inputs, Some(&input));

        assert!(machine
            .memory_mut()
            .store64(size_addr as usize, unlock_data.len() as u64)
            .is_ok());

        assert!(load_input.ecall(&mut machine).is_ok());
        assert_eq!(machine.registers()[A0], SUCCESS as u64);

        assert_eq!(
            machine.memory_mut().load64(size_addr as usize),
            Ok(unlock_data.len() as u64)
        );

        for (i, addr) in (addr as usize..addr as usize + unlock_data.len() as usize).enumerate() {
            assert_eq!(machine.memory_mut().load8(addr), Ok(unlock_data[i]));
        }
    }

    proptest! {
        #[test]
        fn test_load_input_unlock_script(data in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_input_unlock_script(data);
        }
    }

    fn _test_load_missing_output_unlock_script(data: Vec<u8>) {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory>::default();
        let size_addr = 0;
        let addr = 100;

        machine.registers_mut()[A0] = addr; // addr
        machine.registers_mut()[A1] = size_addr; // size_addr
        machine.registers_mut()[A2] = 0; // offset
        machine.registers_mut()[A3] = 0; //index
        machine.registers_mut()[A4] = 1; //source: 1 output
        machine.registers_mut()[A5] = 0; //field: 0 unlock
        machine.registers_mut()[A7] = LOAD_INPUT_BY_FIELD_SYSCALL_NUMBER; // syscall number

        let unlock = Script::new(0, vec![], None, Some(data), vec![]);
        let mut builder = FlatBufferBuilder::new();
        let fbs_offset = FbsScript::build(&mut builder, &unlock);
        builder.finish(fbs_offset, None);
        let unlock_data = builder.finished_data();

        let input = CellInput::new(OutPoint::default(), unlock);
        let inputs = vec![&input];
        let mut load_input = LoadInputByField::new(&inputs, Some(&input));

        assert!(machine
            .memory_mut()
            .store64(size_addr as usize, unlock_data.len() as u64 + 10)
            .is_ok());

        assert!(load_input.ecall(&mut machine).is_ok());
        assert_eq!(machine.registers()[A0], ITEM_MISSING as u64);

        assert_eq!(
            machine.memory_mut().load64(size_addr as usize),
            Ok(unlock_data.len() as u64 + 10)
        );

        for addr in addr as usize..addr as usize + unlock_data.len() as usize {
            assert_eq!(machine.memory_mut().load8(addr), Ok(0));
        }
    }

    proptest! {
        #[test]
        fn test_load_missing_output_unlock_script(data in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_missing_output_unlock_script(data);
        }
    }

    fn _test_load_self_input_outpoint(data: Vec<u8>) {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory>::default();
        let size_addr = 0;
        let addr = 100;

        machine.registers_mut()[A0] = addr; // addr
        machine.registers_mut()[A1] = size_addr; // size_addr
        machine.registers_mut()[A2] = 0; // offset
        machine.registers_mut()[A3] = 0; //index
        machine.registers_mut()[A4] = 2; //source: 2 self
        machine.registers_mut()[A5] = 1; //field: 1 outpoint
        machine.registers_mut()[A7] = LOAD_INPUT_BY_FIELD_SYSCALL_NUMBER; // syscall number

        let unlock = Script::new(0, vec![], None, Some(vec![]), vec![]);
        let sha3_data = sha3_256(data);
        let outpoint = OutPoint::new(H256::from_slice(&sha3_data).unwrap(), 3);
        let mut builder = FlatBufferBuilder::new();
        let fbs_offset = FbsOutPoint::build(&mut builder, &outpoint);
        builder.finish(fbs_offset, None);
        let outpoint_data = builder.finished_data();

        let input = CellInput::new(outpoint, unlock);
        let inputs = vec![];
        let mut load_input = LoadInputByField::new(&inputs, Some(&input));

        assert!(machine
            .memory_mut()
            .store64(size_addr as usize, outpoint_data.len() as u64 + 5)
            .is_ok());

        assert!(load_input.ecall(&mut machine).is_ok());
        assert_eq!(machine.registers()[A0], SUCCESS as u64);

        assert_eq!(
            machine.memory_mut().load64(size_addr as usize),
            Ok(outpoint_data.len() as u64)
        );

        for (i, addr) in (addr as usize..addr as usize + outpoint_data.len() as usize).enumerate() {
            assert_eq!(machine.memory_mut().load8(addr), Ok(outpoint_data[i]));
        }
    }

    proptest! {
        #[test]
        fn test_load_self_input_outpoint(data in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_self_input_outpoint(data);
        }
    }

    #[test]
    fn test_load_missing_self_output_outpoint() {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory>::default();
        let size_addr = 0;
        let addr = 100;

        machine.registers_mut()[A0] = addr; // addr
        machine.registers_mut()[A1] = size_addr; // size_addr
        machine.registers_mut()[A2] = 0; // offset
        machine.registers_mut()[A3] = 0; //index
        machine.registers_mut()[A4] = 2; //source: 2 self
        machine.registers_mut()[A5] = 1; //field: 1 outpoint
        machine.registers_mut()[A7] = LOAD_INPUT_BY_FIELD_SYSCALL_NUMBER; // syscall number

        let inputs = vec![];
        let mut load_input = LoadInputByField::new(&inputs, None);

        assert!(machine.memory_mut().store64(size_addr as usize, 5).is_ok());

        assert!(load_input.ecall(&mut machine).is_ok());
        assert_eq!(machine.registers()[A0], ITEM_MISSING as u64);

        assert_eq!(machine.memory_mut().load64(size_addr as usize), Ok(5));

        for addr in addr as usize..addr as usize + 5 {
            assert_eq!(machine.memory_mut().load8(addr), Ok(0));
        }
    }
}
