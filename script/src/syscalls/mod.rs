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
    Capacity = 0,
    Data = 1,
    DataHash = 2,
    Lock = 6,
    LockHash = 3,
    Type = 4,
    TypeHash = 5,
}

impl CellField {
    fn parse_from_u64(i: u64) -> Result<CellField, Error> {
        match i {
            0 => Ok(CellField::Capacity),
            1 => Ok(CellField::Data),
            2 => Ok(CellField::DataHash),
            3 => Ok(CellField::LockHash),
            4 => Ok(CellField::Type),
            5 => Ok(CellField::TypeHash),
            6 => Ok(CellField::Lock),
            _ => Err(Error::ParseError),
        }
    }
}

#[derive(Debug, PartialEq, Clone, Copy, Eq)]
enum InputField {
    Args = 0,
    OutPoint = 1,
}

impl InputField {
    fn parse_from_u64(i: u64) -> Result<InputField, Error> {
        match i {
            0 => Ok(InputField::Args),
            1 => Ok(InputField::OutPoint),
            _ => Err(Error::ParseError),
        }
    }
}

#[derive(Debug, PartialEq, Clone, Copy, Eq)]
enum Source {
    Current = 0,
    Input = 1,
    Output = 2,
    Dep = 3,
}

impl Source {
    fn parse_from_u64(i: u64) -> Result<Source, Error> {
        match i {
            0 => Ok(Source::Current),
            1 => Ok(Source::Input),
            2 => Ok(Source::Output),
            3 => Ok(Source::Dep),
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
    use ckb_protocol::{
        Bytes as FbsBytes, CellInputBuilder, CellOutput as FbsCellOutput, OutPoint as FbsOutPoint,
    };
    use ckb_vm::machine::DefaultCoreMachine;
    use ckb_vm::{CoreMachine, Memory, SparseMemory, Syscalls, A0, A1, A2, A3, A4, A5, A7};
    use flatbuffers::FlatBufferBuilder;
    use hash::blake2b_256;
    use numext_fixed_hash::H256;
    use proptest::{collection::size_range, prelude::*};

    fn _test_load_tx_all(tx: &[u8]) -> Result<(), TestCaseError> {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory<u64>>::default();
        let size_addr: u64 = 0;
        let addr: u64 = 100;

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, size_addr); // size_addr
        machine.set_register(A2, 0); // offset
        machine.set_register(A7, LOAD_TX_SYSCALL_NUMBER); // syscall number

        prop_assert!(machine
            .memory_mut()
            .store64(&size_addr, &(tx.len() as u64))
            .is_ok());

        let mut load_tx = LoadTx::new(tx);
        prop_assert!(load_tx.ecall(&mut machine).is_ok());

        prop_assert_eq!(machine.registers()[A0], u64::from(SUCCESS));
        for (i, addr) in (addr..addr + tx.len() as u64).enumerate() {
            prop_assert_eq!(machine.memory_mut().load8(&addr), Ok(u64::from(tx[i])));
        }
        Ok(())
    }

    proptest! {
        #[test]
        fn test_load_tx_all(ref tx in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_tx_all(tx)?;
        }
    }

    fn _test_load_tx_length(tx: &[u8]) -> Result<(), TestCaseError> {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory<u64>>::default();
        let size_addr: u64 = 0;
        let addr: u64 = 100;

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, size_addr); // size_addr
        machine.set_register(A2, 0); // offset
        machine.set_register(A7, LOAD_TX_SYSCALL_NUMBER); // syscall number

        prop_assert!(machine.memory_mut().store64(&size_addr, &0).is_ok());

        let mut load_tx = LoadTx::new(tx);
        prop_assert!(load_tx.ecall(&mut machine).is_ok());

        prop_assert_eq!(machine.registers()[A0], u64::from(SUCCESS));
        prop_assert_eq!(machine.memory_mut().load64(&size_addr), Ok(tx.len() as u64));

        machine.set_register(A0, addr); // addr
        machine.set_register(A2, 100); // offset

        prop_assert!(machine.memory_mut().store64(&size_addr, &0).is_ok());

        prop_assert!(load_tx.ecall(&mut machine).is_ok());

        prop_assert_eq!(machine.registers()[A0], u64::from(SUCCESS));
        prop_assert_eq!(
            machine.memory_mut().load64(&size_addr),
            Ok(tx.len() as u64 - 100)
        );
        Ok(())
    }

    proptest! {
        #[test]
        fn test_load_tx_length(ref tx in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_tx_length(tx)?;
        }
    }

    fn _test_load_tx_partial(tx: &[u8]) -> Result<(), TestCaseError> {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory<u64>>::default();
        let size_addr: u64 = 0;
        let addr: u64 = 100;
        let offset: u64 = 100;

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, size_addr); // size_addr
        machine.set_register(A2, offset); // offset
        machine.set_register(A7, LOAD_TX_SYSCALL_NUMBER); // syscall number

        prop_assert!(machine
            .memory_mut()
            .store64(&size_addr, &(tx.len() as u64))
            .is_ok());

        let mut load_tx = LoadTx::new(tx);
        prop_assert!(load_tx.ecall(&mut machine).is_ok());

        prop_assert_eq!(machine.registers()[A0], u64::from(SUCCESS));
        prop_assert_eq!(
            machine.memory_mut().load64(&size_addr),
            Ok(tx.len() as u64 - offset)
        );
        for (i, addr) in (addr..addr + tx.len() as u64 - offset).enumerate() {
            prop_assert_eq!(
                machine.memory_mut().load8(&addr),
                Ok(u64::from(tx[i + offset as usize]))
            );
        }
        Ok(())
    }

    proptest! {
        #[test]
        fn test_load_tx_partial(ref tx in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_tx_partial(tx)?;
        }
    }

    fn _test_load_cell_item_missing(data: &[u8]) -> Result<(), TestCaseError> {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory<u64>>::default();
        let size_addr: u64 = 0;
        let addr: u64 = 100;

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, size_addr); // size_addr
        machine.set_register(A2, 0); // offset
        machine.set_register(A3, 1); //index
        machine.set_register(A4, Source::Input as u64); //source: 1 input
        machine.set_register(A7, LOAD_CELL_SYSCALL_NUMBER); // syscall number

        prop_assert!(machine
            .memory_mut()
            .store64(&size_addr, &(data.len() as u64))
            .is_ok());

        let output = CellOutput::new(100, data.to_vec(), Script::default(), None);
        let input_cell = CellOutput::new(
            100,
            data.iter().rev().cloned().collect(),
            Script::default(),
            None,
        );
        let outputs = vec![&output];
        let input_cells = vec![&input_cell];
        let dep_cells = vec![];
        let mut load_cell = LoadCell::new(&outputs, &input_cells, &input_cell, &dep_cells);

        prop_assert!(load_cell.ecall(&mut machine).is_ok());
        prop_assert_eq!(machine.registers()[A0], u64::from(ITEM_MISSING));
        Ok(())
    }

    proptest! {
        #[test]
        fn test_load_cell_item_missing(ref data in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_cell_item_missing(data)?;
        }
    }

    fn _test_load_cell_all(data: &[u8]) -> Result<(), TestCaseError> {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory<u64>>::default();
        let size_addr: u64 = 0;
        let addr: u64 = 100;

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, size_addr); // size_addr
        machine.set_register(A2, 0); // offset
        machine.set_register(A3, 0); //index
        machine.set_register(A4, Source::Input as u64); //source: 1 input
        machine.set_register(A7, LOAD_CELL_SYSCALL_NUMBER); // syscall number

        let output = CellOutput::new(100, data.to_vec(), Script::default(), None);
        let input_cell = CellOutput::new(
            100,
            data.iter().rev().cloned().collect(),
            Script::default(),
            None,
        );
        let outputs = vec![&output];
        let input_cells = vec![&input_cell];
        let dep_cells = vec![];
        let mut load_cell = LoadCell::new(&outputs, &input_cells, &input_cell, &dep_cells);

        let mut builder = FlatBufferBuilder::new();
        let fbs_offset = FbsCellOutput::build(&mut builder, &input_cell);
        builder.finish(fbs_offset, None);
        let input_correct_data = builder.finished_data();

        let mut builder = FlatBufferBuilder::new();
        let fbs_offset = FbsCellOutput::build(&mut builder, &output);
        builder.finish(fbs_offset, None);
        let output_correct_data = builder.finished_data();

        // test input
        prop_assert!(machine
            .memory_mut()
            .store64(&size_addr, &(input_correct_data.len() as u64))
            .is_ok());

        prop_assert!(load_cell.ecall(&mut machine).is_ok());
        prop_assert_eq!(machine.registers()[A0], u64::from(SUCCESS));

        prop_assert_eq!(
            machine.memory_mut().load64(&size_addr),
            Ok(input_correct_data.len() as u64)
        );

        for (i, addr) in (addr..addr + input_correct_data.len() as u64).enumerate() {
            prop_assert_eq!(
                machine.memory_mut().load8(&addr),
                Ok(u64::from(input_correct_data[i]))
            );
        }

        // clean memory
        prop_assert!(machine.memory_mut().store_byte(0, 1100, 0).is_ok());

        // test output
        machine.set_register(A0, addr); // addr
        machine.set_register(A1, size_addr); // size_addr
        machine.set_register(A4, Source::Output as u64); //source: 2 output
        prop_assert!(machine
            .memory_mut()
            .store64(&size_addr, &(output_correct_data.len() as u64 + 10))
            .is_ok());

        prop_assert!(load_cell.ecall(&mut machine).is_ok());
        prop_assert_eq!(machine.registers()[A0], u64::from(SUCCESS));

        prop_assert_eq!(
            machine.memory_mut().load64(&size_addr),
            Ok(output_correct_data.len() as u64)
        );

        for (i, addr) in (addr..addr + output_correct_data.len() as u64).enumerate() {
            prop_assert_eq!(
                machine.memory_mut().load8(&addr),
                Ok(u64::from(output_correct_data[i]))
            );
        }
        Ok(())
    }

    proptest! {
        #[test]
        fn test_load_cell_all(ref tx in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_cell_all(tx)?;
        }
    }

    fn _test_load_cell_length(data: &[u8]) -> Result<(), TestCaseError> {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory<u64>>::default();
        let size_addr: u64 = 0;
        let addr: u64 = 100;

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, size_addr); // size_addr
        machine.set_register(A2, 0); // offset
        machine.set_register(A3, 0); //index
        machine.set_register(A4, Source::Input as u64); //source: 1 input
        machine.set_register(A7, LOAD_CELL_SYSCALL_NUMBER); // syscall number

        let output = CellOutput::new(100, data.to_vec(), Script::default(), None);
        let input_cell = CellOutput::new(
            100,
            data.iter().rev().cloned().collect(),
            Script::default(),
            None,
        );
        let outputs = vec![&output];
        let input_cells = vec![&input_cell];
        let dep_cells = vec![];
        let mut load_cell = LoadCell::new(&outputs, &input_cells, &input_cell, &dep_cells);

        let mut builder = FlatBufferBuilder::new();
        let fbs_offset = FbsCellOutput::build(&mut builder, &input_cell);
        builder.finish(fbs_offset, None);
        let input_correct_data = builder.finished_data();

        prop_assert!(machine.memory_mut().store64(&size_addr, &0).is_ok());

        prop_assert!(load_cell.ecall(&mut machine).is_ok());
        prop_assert_eq!(machine.registers()[A0], u64::from(SUCCESS));

        prop_assert_eq!(
            machine.memory_mut().load64(&size_addr),
            Ok(input_correct_data.len() as u64)
        );
        Ok(())
    }

    proptest! {
        #[test]
        fn test_load_cell_length(ref tx in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_cell_length(tx)?;
        }
    }

    fn _test_load_cell_partial(data: &[u8]) -> Result<(), TestCaseError> {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory<u64>>::default();
        let size_addr: u64 = 0;
        let addr: u64 = 100;
        let offset: u64 = 100;

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, size_addr); // size_addr
        machine.set_register(A2, offset); // offset
        machine.set_register(A3, 0); // index
        machine.set_register(A4, Source::Input as u64); // source: 1 input
        machine.set_register(A7, LOAD_CELL_SYSCALL_NUMBER); // syscall number

        let output = CellOutput::new(100, data.to_vec(), Script::default(), None);
        let input_cell = CellOutput::new(
            100,
            data.iter().rev().cloned().collect(),
            Script::default(),
            None,
        );
        let outputs = vec![&output];
        let input_cells = vec![&input_cell];
        let dep_cells = vec![];
        let mut load_cell = LoadCell::new(&outputs, &input_cells, &input_cell, &dep_cells);

        let mut builder = FlatBufferBuilder::new();
        let fbs_offset = FbsCellOutput::build(&mut builder, &input_cell);
        builder.finish(fbs_offset, None);
        let input_correct_data = builder.finished_data();

        prop_assert!(machine
            .memory_mut()
            .store64(&size_addr, &(input_correct_data.len() as u64))
            .is_ok());

        prop_assert!(load_cell.ecall(&mut machine).is_ok());
        prop_assert_eq!(machine.registers()[A0], u64::from(SUCCESS));

        for (i, addr) in (addr..addr + input_correct_data.len() as u64 - offset).enumerate() {
            prop_assert_eq!(
                machine.memory_mut().load8(&addr),
                Ok(u64::from(input_correct_data[i + offset as usize]))
            );
        }
        Ok(())
    }

    proptest! {
        #[test]
        fn test_load_cell_partial(ref data in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_cell_partial(data)?;
        }
    }

    fn _test_load_current_cell(data: &[u8]) -> Result<(), TestCaseError> {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory<u64>>::default();
        let size_addr: u64 = 0;
        let addr: u64 = 100;

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, size_addr); // size_addr
        machine.set_register(A2, 0); // offset
        machine.set_register(A3, 1000); //index
        machine.set_register(A4, Source::Current as u64); //source: 0 current
        machine.set_register(A7, LOAD_CELL_SYSCALL_NUMBER); // syscall number

        let input_cell = CellOutput::new(
            100,
            data.iter().rev().cloned().collect(),
            Script::default(),
            None,
        );
        let outputs = vec![];
        let input_cells = vec![];
        let dep_cells = vec![];
        let mut load_cell = LoadCell::new(&outputs, &input_cells, &input_cell, &dep_cells);

        let mut builder = FlatBufferBuilder::new();
        let fbs_offset = FbsCellOutput::build(&mut builder, &input_cell);
        builder.finish(fbs_offset, None);
        let input_correct_data = builder.finished_data();

        // test input
        prop_assert!(machine
            .memory_mut()
            .store64(&size_addr, &(input_correct_data.len() as u64 + 5))
            .is_ok());

        prop_assert!(load_cell.ecall(&mut machine).is_ok());
        prop_assert_eq!(machine.registers()[A0], u64::from(SUCCESS));

        prop_assert_eq!(
            machine.memory_mut().load64(&size_addr),
            Ok(input_correct_data.len() as u64)
        );

        for (i, addr) in (addr..addr + input_correct_data.len() as u64).enumerate() {
            prop_assert_eq!(
                machine.memory_mut().load8(&addr),
                Ok(u64::from(input_correct_data[i]))
            );
        }
        Ok(())
    }

    proptest! {
        #[test]
        fn test_load_current_cell(ref tx in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_current_cell(tx)?;
        }
    }

    fn _test_load_cell_capacity(capacity: u64) -> Result<(), TestCaseError> {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory<u64>>::default();
        let size_addr: u64 = 0;
        let addr: u64 = 100;

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, size_addr); // size_addr
        machine.set_register(A2, 0); // offset
        machine.set_register(A3, 0); //index
        machine.set_register(A4, Source::Input as u64); //source: 1 input
        machine.set_register(A5, CellField::Capacity as u64); //field: 0 capacity
        machine.set_register(A7, LOAD_CELL_BY_FIELD_SYSCALL_NUMBER); // syscall number

        let input_cell = CellOutput::new(capacity, vec![], Script::default(), None);
        let outputs = vec![];
        let input_cells = vec![&input_cell];
        let dep_cells = vec![];
        let mut load_cell = LoadCellByField::new(&outputs, &input_cells, &input_cell, &dep_cells);

        prop_assert!(machine.memory_mut().store64(&size_addr, &16).is_ok());

        prop_assert!(load_cell.ecall(&mut machine).is_ok());
        prop_assert_eq!(machine.registers()[A0], u64::from(SUCCESS));

        prop_assert_eq!(machine.memory_mut().load64(&size_addr), Ok(8));

        let mut buffer = vec![];
        buffer.write_u64::<LittleEndian>(capacity).unwrap();

        for (i, addr) in (addr..addr + buffer.len() as u64).enumerate() {
            prop_assert_eq!(machine.memory_mut().load8(&addr), Ok(u64::from(buffer[i])));
        }
        Ok(())
    }

    proptest! {
        #[test]
        fn test_load_cell_capacity(capacity in any::<u64>()) {
            _test_load_cell_capacity(capacity)?;
        }
    }

    fn _test_load_self_lock_hash(data: &[u8]) -> Result<(), TestCaseError> {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory<u64>>::default();
        let size_addr: u64 = 0;
        let addr: u64 = 100;

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, size_addr); // size_addr
        machine.set_register(A2, 0); // offset
        machine.set_register(A3, 0); //index
        machine.set_register(A4, Source::Current as u64); //source: 0 current
        machine.set_register(A5, CellField::LockHash as u64); //field: 3 lock hash
        machine.set_register(A7, LOAD_CELL_BY_FIELD_SYSCALL_NUMBER); // syscall number

        let script = Script::new(0, vec![data.to_vec()], H256::zero());
        let h = script.hash();
        let hash = h.as_bytes();
        let input_cell = CellOutput::new(100, vec![], script, None);
        let outputs = vec![];
        let input_cells = vec![];
        let dep_cells = vec![];
        let mut load_cell = LoadCellByField::new(&outputs, &input_cells, &input_cell, &dep_cells);

        prop_assert!(machine.memory_mut().store64(&size_addr, &64).is_ok());

        prop_assert!(load_cell.ecall(&mut machine).is_ok());
        prop_assert_eq!(machine.registers()[A0], u64::from(SUCCESS));

        prop_assert_eq!(
            machine.memory_mut().load64(&size_addr),
            Ok(hash.len() as u64)
        );

        for (i, addr) in (addr..addr + hash.len() as u64).enumerate() {
            prop_assert_eq!(machine.memory_mut().load8(&addr), Ok(u64::from(hash[i])));
        }

        machine.set_register(A0, addr); // addr
        prop_assert!(machine.memory_mut().store64(&size_addr, &0).is_ok());

        prop_assert!(load_cell.ecall(&mut machine).is_ok());
        prop_assert_eq!(machine.registers()[A0], u64::from(SUCCESS));

        prop_assert_eq!(
            machine.memory_mut().load64(&size_addr),
            Ok(hash.len() as u64)
        );
        Ok(())
    }

    proptest! {
        #[test]
        fn test_load_self_lock_hash(ref data in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_self_lock_hash(data)?;
        }
    }

    #[test]
    fn test_load_missing_contract() {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory<u64>>::default();
        let size_addr: u64 = 0;
        let addr: u64 = 100;

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, size_addr); // size_addr
        machine.set_register(A2, 0); // offset
        machine.set_register(A3, 0); //index
        machine.set_register(A4, Source::Output as u64); //source: 2 output
        machine.set_register(A5, CellField::Type as u64); //field: 4 type
        machine.set_register(A7, LOAD_CELL_BY_FIELD_SYSCALL_NUMBER); // syscall number

        let output_cell = CellOutput::new(100, vec![], Script::default(), None);
        let outputs = vec![&output_cell];
        let input_cells = vec![];
        let dep_cells = vec![];
        let mut load_cell = LoadCellByField::new(&outputs, &input_cells, &output_cell, &dep_cells);

        assert!(machine.memory_mut().store64(&size_addr, &100).is_ok());

        assert!(load_cell.ecall(&mut machine).is_ok());
        assert_eq!(machine.registers()[A0], u64::from(ITEM_MISSING));

        assert_eq!(machine.memory_mut().load64(&size_addr), Ok(100));

        for addr in addr..addr + 100 {
            assert_eq!(machine.memory_mut().load8(&addr), Ok(0));
        }
    }

    fn _test_load_input_unlock_args(data: Vec<u8>) -> Result<(), TestCaseError> {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory<u64>>::default();
        let size_addr: u64 = 0;
        let addr: u64 = 100;

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, size_addr); // size_addr
        machine.set_register(A2, 0); // offset
        machine.set_register(A3, 0); //index
        machine.set_register(A4, Source::Input as u64); //source: 1 input
        machine.set_register(A5, InputField::Args as u64); //field: 0 args
        machine.set_register(A7, LOAD_INPUT_BY_FIELD_SYSCALL_NUMBER); // syscall number

        let args = vec![data.to_vec()];

        let mut builder = FlatBufferBuilder::new();
        let vec = args
            .iter()
            .map(|argument| FbsBytes::build(&mut builder, argument))
            .collect::<Vec<_>>();
        let fbs_args = builder.create_vector(&vec);
        let mut input_builder = CellInputBuilder::new(&mut builder);
        input_builder.add_args(fbs_args);
        let offset = input_builder.finish();
        builder.finish(offset, None);
        let args_data = builder.finished_data();

        let input = CellInput::new(OutPoint::default(), args);
        let inputs = vec![&input];
        let mut load_input = LoadInputByField::new(&inputs, Some(&input));

        prop_assert!(machine
            .memory_mut()
            .store64(&size_addr, &(args_data.len() as u64))
            .is_ok());

        prop_assert!(load_input.ecall(&mut machine).is_ok());
        prop_assert_eq!(machine.registers()[A0], u64::from(SUCCESS));

        prop_assert_eq!(
            machine.memory_mut().load64(&size_addr),
            Ok(args_data.len() as u64)
        );

        for (i, addr) in (addr..addr + args_data.len() as u64).enumerate() {
            prop_assert_eq!(
                machine.memory_mut().load8(&addr),
                Ok(u64::from(args_data[i]))
            );
        }
        Ok(())
    }

    proptest! {
        #[test]
        fn test_load_input_unlock_args(data in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_input_unlock_args(data)?;
        }
    }

    fn _test_load_missing_output_unlock_args(data: Vec<u8>) -> Result<(), TestCaseError> {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory<u64>>::default();
        let size_addr: u64 = 0;
        let addr: u64 = 100;

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, size_addr); // size_addr
        machine.set_register(A2, 0); // offset
        machine.set_register(A3, 0); //index
        machine.set_register(A4, Source::Output as u64); //source: 2 output
        machine.set_register(A5, InputField::Args as u64); //field: 0 unlock
        machine.set_register(A7, LOAD_INPUT_BY_FIELD_SYSCALL_NUMBER); // syscall number

        let args = vec![data.to_vec()];

        let mut builder = FlatBufferBuilder::new();
        let vec = args
            .iter()
            .map(|argument| FbsBytes::build(&mut builder, argument))
            .collect::<Vec<_>>();
        let fbs_args = builder.create_vector(&vec);
        let mut input_builder = CellInputBuilder::new(&mut builder);
        input_builder.add_args(fbs_args);
        let offset = input_builder.finish();
        builder.finish(offset, None);
        let args_data = builder.finished_data();

        let input = CellInput::new(OutPoint::default(), args);
        let inputs = vec![&input];
        let mut load_input = LoadInputByField::new(&inputs, Some(&input));

        prop_assert!(machine
            .memory_mut()
            .store64(&size_addr, &(args_data.len() as u64 + 10))
            .is_ok());

        prop_assert!(load_input.ecall(&mut machine).is_ok());
        prop_assert_eq!(machine.registers()[A0], u64::from(ITEM_MISSING));

        prop_assert_eq!(
            machine.memory_mut().load64(&size_addr),
            Ok(args_data.len() as u64 + 10)
        );

        for addr in addr..addr + args_data.len() as u64 {
            prop_assert_eq!(machine.memory_mut().load8(&addr), Ok(0));
        }
        Ok(())
    }

    proptest! {
        #[test]
        fn test_load_missing_output_unlock_args(data in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_missing_output_unlock_args(data)?;
        }
    }

    fn _test_load_self_input_out_point(data: &[u8]) -> Result<(), TestCaseError> {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory<u64>>::default();
        let size_addr: u64 = 0;
        let addr: u64 = 100;

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, size_addr); // size_addr
        machine.set_register(A2, 0); // offset
        machine.set_register(A3, 0); //index
        machine.set_register(A4, Source::Current as u64); //source: 0 current
        machine.set_register(A5, InputField::OutPoint as u64); //field: 1 out_point
        machine.set_register(A7, LOAD_INPUT_BY_FIELD_SYSCALL_NUMBER); // syscall number

        let blake2b_data = blake2b_256(data);
        let out_point = OutPoint::new(H256::from_slice(&blake2b_data).unwrap(), 3);
        let mut builder = FlatBufferBuilder::new();
        let fbs_offset = FbsOutPoint::build(&mut builder, &out_point);
        builder.finish(fbs_offset, None);
        let out_point_data = builder.finished_data();

        let input = CellInput::new(out_point, vec![]);
        let inputs = vec![];
        let mut load_input = LoadInputByField::new(&inputs, Some(&input));

        prop_assert!(machine
            .memory_mut()
            .store64(&size_addr, &(out_point_data.len() as u64 + 5))
            .is_ok());

        prop_assert!(load_input.ecall(&mut machine).is_ok());
        prop_assert_eq!(machine.registers()[A0], u64::from(SUCCESS));

        prop_assert_eq!(
            machine.memory_mut().load64(&size_addr),
            Ok(out_point_data.len() as u64)
        );

        for (i, addr) in (addr..addr + out_point_data.len() as u64).enumerate() {
            prop_assert_eq!(
                machine.memory_mut().load8(&addr),
                Ok(u64::from(out_point_data[i]))
            );
        }
        Ok(())
    }

    proptest! {
        #[test]
        fn test_load_self_input_out_point(ref data in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_self_input_out_point(data)?;
        }
    }

    #[test]
    fn test_load_missing_self_output_out_point() {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory<u64>>::default();
        let size_addr: u64 = 0;
        let addr: u64 = 100;

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, size_addr); // size_addr
        machine.set_register(A2, 0); // offset
        machine.set_register(A3, 0); //index
        machine.set_register(A4, Source::Current as u64); //source: 0 current
        machine.set_register(A5, InputField::OutPoint as u64); //field: 1 out_point
        machine.set_register(A7, LOAD_INPUT_BY_FIELD_SYSCALL_NUMBER); // syscall number

        let inputs = vec![];
        let mut load_input = LoadInputByField::new(&inputs, None);

        assert!(machine.memory_mut().store64(&size_addr, &5).is_ok());

        assert!(load_input.ecall(&mut machine).is_ok());
        assert_eq!(machine.registers()[A0], u64::from(ITEM_MISSING));

        assert_eq!(machine.memory_mut().load64(&size_addr), Ok(5));

        for addr in addr..addr + 5 {
            assert_eq!(machine.memory_mut().load8(&addr), Ok(0));
        }
    }

    fn _test_load_dep_cell_data(data: &[u8]) -> Result<(), TestCaseError> {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory<u64>>::default();
        let size_addr: u64 = 0;
        let addr: u64 = 100;

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, size_addr); // size_addr
        machine.set_register(A2, 0); // offset
        machine.set_register(A3, 0); //index
        machine.set_register(A4, Source::Dep as u64); //source: 3 dep
        machine.set_register(A5, CellField::Data as u64); //field: 1 data
        machine.set_register(A7, LOAD_CELL_BY_FIELD_SYSCALL_NUMBER); // syscall number

        let input_cell = CellOutput::new(1000, vec![], Script::default(), None);
        let dep_cell = CellOutput::new(1000, data.to_vec(), Script::default(), None);
        let outputs = vec![];
        let input_cells = vec![&input_cell];
        let dep_cells = vec![&dep_cell];
        let mut load_cell = LoadCellByField::new(&outputs, &input_cells, &input_cell, &dep_cells);

        prop_assert!(machine
            .memory_mut()
            .store64(&size_addr, &(data.len() as u64 + 20))
            .is_ok());

        prop_assert!(load_cell.ecall(&mut machine).is_ok());
        prop_assert_eq!(machine.registers()[A0], u64::from(SUCCESS));

        prop_assert_eq!(
            machine.memory_mut().load64(&size_addr),
            Ok(data.len() as u64)
        );

        for (i, addr) in (addr..addr + data.len() as u64).enumerate() {
            prop_assert_eq!(machine.memory_mut().load8(&addr), Ok(u64::from(data[i])));
        }
        Ok(())
    }

    proptest! {
        #[test]
        fn test_load_dep_cell_data(ref data in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_dep_cell_data(data)?;
        }
    }

    fn _test_load_dep_cell_data_hash(data: &[u8]) -> Result<(), TestCaseError> {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory<u64>>::default();
        let size_addr: u64 = 0;
        let addr: u64 = 100;

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, size_addr); // size_addr
        machine.set_register(A2, 0); // offset
        machine.set_register(A3, 0); //index
        machine.set_register(A4, Source::Dep as u64); //source: 3 dep
        machine.set_register(A5, CellField::DataHash as u64); //field: 2 data hash
        machine.set_register(A7, LOAD_CELL_BY_FIELD_SYSCALL_NUMBER); // syscall number

        let input_cell = CellOutput::new(1000, vec![], Script::default(), None);
        let dep_cell = CellOutput::new(1000, data.to_vec(), Script::default(), None);
        let outputs = vec![];
        let input_cells = vec![&input_cell];
        let dep_cells = vec![&dep_cell];
        let mut load_cell = LoadCellByField::new(&outputs, &input_cells, &input_cell, &dep_cells);

        let data_hash = blake2b_256(&data);

        prop_assert!(machine
            .memory_mut()
            .store64(&size_addr, &(data_hash.len() as u64 + 20))
            .is_ok());

        prop_assert!(load_cell.ecall(&mut machine).is_ok());
        prop_assert_eq!(machine.registers()[A0], u64::from(SUCCESS));

        prop_assert_eq!(
            machine.memory_mut().load64(&size_addr),
            Ok(data_hash.len() as u64)
        );

        for (i, addr) in (addr..addr + data_hash.len() as u64).enumerate() {
            prop_assert_eq!(
                machine.memory_mut().load8(&addr),
                Ok(u64::from(data_hash[i]))
            );
        }
        Ok(())
    }

    proptest! {
        #[test]
        fn test_load_dep_cell_data_hash(ref data in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_dep_cell_data_hash(data)?;
        }
    }
}
