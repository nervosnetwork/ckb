mod debugger;
mod load_cell;
mod load_cell_data;
mod load_header;
mod load_input;
mod load_script_hash;
mod load_tx_hash;
mod load_witness;
mod utils;

pub use self::debugger::Debugger;
pub use self::load_cell::LoadCell;
pub use self::load_cell_data::LoadCellData;
pub use self::load_header::LoadHeader;
pub use self::load_input::LoadInput;
pub use self::load_script_hash::LoadScriptHash;
pub use self::load_tx_hash::LoadTxHash;
pub use self::load_witness::LoadWitness;

use ckb_vm::Error;

pub const SUCCESS: u8 = 0;
// INDEX_OUT_OF_BOUND is returned when requesting the 4th output in a transaction
// with only 3 outputs; while ITEM_MISSING is returned when requesting (for example)
// the type field on an output without type script, or requesting the cell data
// for a dep OutPoint which only references a block header.
pub const INDEX_OUT_OF_BOUND: u8 = 1;
pub const ITEM_MISSING: u8 = 2;
pub const SLICE_OUT_OF_BOUND: u8 = 3;

pub const LOAD_TX_HASH_SYSCALL_NUMBER: u64 = 2061;
pub const LOAD_SCRIPT_HASH_SYSCALL_NUMBER: u64 = 2062;
pub const LOAD_CELL_SYSCALL_NUMBER: u64 = 2071;
pub const LOAD_HEADER_SYSCALL_NUMBER: u64 = 2072;
pub const LOAD_INPUT_SYSCALL_NUMBER: u64 = 2073;
pub const LOAD_WITNESS_SYSCALL_NUMBER: u64 = 2074;
pub const LOAD_CELL_BY_FIELD_SYSCALL_NUMBER: u64 = 2081;
pub const LOAD_INPUT_BY_FIELD_SYSCALL_NUMBER: u64 = 2083;
pub const LOAD_CELL_DATA_SYSCALL_NUMBER: u64 = 2091;
pub const DEBUG_PRINT_SYSCALL_NUMBER: u64 = 2177;

#[derive(Debug, PartialEq, Clone, Copy, Eq)]
enum CellField {
    Capacity = 0,
    DataHash = 1,
    Lock = 2,
    LockHash = 3,
    Type = 4,
    TypeHash = 5,
    OccupiedCapacity = 6,
}

impl CellField {
    fn parse_from_u64(i: u64) -> Result<CellField, Error> {
        match i {
            0 => Ok(CellField::Capacity),
            1 => Ok(CellField::DataHash),
            2 => Ok(CellField::Lock),
            3 => Ok(CellField::LockHash),
            4 => Ok(CellField::Type),
            5 => Ok(CellField::TypeHash),
            6 => Ok(CellField::OccupiedCapacity),
            _ => Err(Error::ParseError),
        }
    }
}

#[derive(Debug, PartialEq, Clone, Copy, Eq)]
enum InputField {
    OutPoint = 0,
    Since = 1,
}

impl InputField {
    fn parse_from_u64(i: u64) -> Result<InputField, Error> {
        match i {
            0 => Ok(InputField::OutPoint),
            1 => Ok(InputField::Since),
            _ => Err(Error::ParseError),
        }
    }
}

#[derive(Debug, PartialEq, Clone, Copy, Eq)]
enum SourceEntry {
    Input,
    Output,
    Dep,
}

impl From<SourceEntry> for u64 {
    fn from(s: SourceEntry) -> u64 {
        match s {
            SourceEntry::Input => 1,
            SourceEntry::Output => 2,
            SourceEntry::Dep => 3,
        }
    }
}

impl SourceEntry {
    fn parse_from_u64(i: u64) -> Result<SourceEntry, Error> {
        match i {
            1 => Ok(SourceEntry::Input),
            2 => Ok(SourceEntry::Output),
            3 => Ok(SourceEntry::Dep),
            _ => Err(Error::ParseError),
        }
    }
}

const SOURCE_GROUP_FLAG: u64 = 0x0100_0000_0000_0000;
const SOURCE_GROUP_MASK: u64 = 0xFF00_0000_0000_0000;
const SOURCE_ENTRY_MASK: u64 = 0x00FF_FFFF_FFFF_FFFF;

#[derive(Debug, PartialEq, Clone, Copy, Eq)]
enum Source {
    Transaction(SourceEntry),
    Group(SourceEntry),
}

impl From<Source> for u64 {
    fn from(s: Source) -> u64 {
        match s {
            Source::Transaction(e) => u64::from(e),
            Source::Group(e) => SOURCE_GROUP_FLAG | u64::from(e),
        }
    }
}

impl Source {
    fn parse_from_u64(i: u64) -> Result<Source, Error> {
        let entry = SourceEntry::parse_from_u64(i & SOURCE_ENTRY_MASK)?;
        if i & SOURCE_GROUP_MASK == SOURCE_GROUP_FLAG {
            Ok(Source::Group(entry))
        } else {
            Ok(Source::Transaction(entry))
        }
    }
}

#[derive(Copy, Clone, Debug)]
enum LoadDataType {
    Data = 0,
    Code = 1,
}

impl LoadDataType {
    pub fn parse_from_u64(i: u64) -> Result<LoadDataType, Error> {
        match i {
            0 => Ok(LoadDataType::Data),
            1 => Ok(LoadDataType::Code),
            _ => Err(Error::ParseError),
        }
    }

    pub fn memory_flags(self) -> u8 {
        use ckb_vm::memory::{FLAG_EXECUTABLE, FLAG_FREEZED, FLAG_WRITABLE};
        match self {
            LoadDataType::Data => FLAG_WRITABLE,
            LoadDataType::Code => FLAG_EXECUTABLE | FLAG_FREEZED,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use byteorder::{LittleEndian, WriteBytesExt};
    use ckb_core::cell::{CellMeta, ResolvedDep, ResolvedInput};
    use ckb_core::header::HeaderBuilder;
    use ckb_core::script::{Script, ScriptHashType};
    use ckb_core::transaction::{CellOutput, CellOutputBuilder, OutPoint};
    use ckb_core::{capacity_bytes, Bytes, Capacity};
    use ckb_db::RocksDB;
    use ckb_hash::blake2b_256;
    use ckb_protocol::{CellOutput as FbsCellOutput, Header as FbsHeader, Witness as FbsWitness};
    use ckb_store::{data_loader_wrapper::DataLoaderWrapper, ChainDB, COLUMNS};
    use ckb_vm::machine::DefaultCoreMachine;
    use ckb_vm::{
        memory::{FLAG_EXECUTABLE, FLAG_FREEZED},
        registers::{A0, A1, A2, A3, A4, A5, A6, A7},
        CoreMachine, Memory, SparseMemory, Syscalls, WXorXMemory, RISCV_PAGESIZE,
    };
    use flatbuffers::FlatBufferBuilder;
    use numext_fixed_hash::H256;
    use proptest::{collection::size_range, prelude::*};

    fn new_store() -> ChainDB {
        ChainDB::new(RocksDB::open_tmp(COLUMNS))
    }

    fn build_cell_meta(output: CellOutput, data: Bytes) -> CellMeta {
        CellMeta {
            out_point: OutPoint {
                tx_hash: Default::default(),
                index: 0,
            },
            block_info: None,
            cellbase: false,
            cell_output: output,
            data_bytes: data.len() as u64,
            mem_cell_data: Some(data),
        }
    }

    fn build_resolved_input(output: CellOutput, data: Bytes) -> ResolvedInput {
        ResolvedInput::new(build_cell_meta(output, data))
    }
    fn build_resolved_dep(output: CellOutput, data: Bytes) -> ResolvedDep {
        ResolvedDep::cell_only(build_cell_meta(output, data))
    }

    fn _test_load_cell_not_exist(data: &[u8]) -> Result<(), TestCaseError> {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory<u64>>::default();
        let size_addr: u64 = 0;
        let addr: u64 = 100;

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, size_addr); // size_addr
        machine.set_register(A2, 0); // offset
        machine.set_register(A3, 1); //index
        machine.set_register(A4, u64::from(Source::Transaction(SourceEntry::Input))); //source: 1 input
        machine.set_register(A7, LOAD_CELL_SYSCALL_NUMBER); // syscall number

        prop_assert!(machine
            .memory_mut()
            .store64(&size_addr, &(data.len() as u64))
            .is_ok());

        let output_cell_data = Bytes::from(data);
        let output = build_cell_meta(
            CellOutputBuilder::from_data(&output_cell_data)
                .capacity(capacity_bytes!(100))
                .build(),
            output_cell_data,
        );
        let input_cell_data: Bytes = data.iter().rev().cloned().collect();
        let input_cell = build_resolved_input(
            CellOutputBuilder::from_data(&input_cell_data)
                .capacity(capacity_bytes!(100))
                .build(),
            input_cell_data,
        );
        let outputs = vec![output];
        let resolved_inputs = vec![input_cell];
        let resolved_deps = vec![];
        let group_inputs = vec![];
        let group_outputs = vec![];
        let mut load_cell = LoadCell::new(
            &outputs,
            &resolved_inputs,
            &resolved_deps,
            &group_inputs,
            &group_outputs,
        );

        prop_assert!(load_cell.ecall(&mut machine).is_ok());
        prop_assert_eq!(machine.registers()[A0], u64::from(INDEX_OUT_OF_BOUND));
        Ok(())
    }

    proptest! {
        #[test]
        fn test_load_cell_not_exist(ref data in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_cell_not_exist(data)?;
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
        machine.set_register(A4, u64::from(Source::Transaction(SourceEntry::Input))); //source: 1 input
        machine.set_register(A7, LOAD_CELL_SYSCALL_NUMBER); // syscall number

        let output_cell_data = Bytes::from(data);
        let output = build_cell_meta(
            CellOutputBuilder::from_data(&output_cell_data)
                .capacity(capacity_bytes!(100))
                .build(),
            output_cell_data,
        );
        let input_cell_data: Bytes = data.iter().rev().cloned().collect();
        let input_cell = build_resolved_input(
            CellOutputBuilder::from_data(&input_cell_data)
                .capacity(capacity_bytes!(100))
                .build(),
            input_cell_data,
        );
        let outputs = vec![output.clone()];
        let resolved_inputs = vec![input_cell.clone()];
        let resolved_deps = vec![];
        let group_inputs = vec![];
        let group_outputs = vec![];
        let mut load_cell = LoadCell::new(
            &outputs,
            &resolved_inputs,
            &resolved_deps,
            &group_inputs,
            &group_outputs,
        );

        let mut builder = FlatBufferBuilder::new();
        let fbs_offset = FbsCellOutput::build(&mut builder, &input_cell.cell().cell_output);
        builder.finish(fbs_offset, None);
        let input_correct_data = builder.finished_data();

        let mut builder = FlatBufferBuilder::new();
        let fbs_offset = FbsCellOutput::build(&mut builder, &output.cell_output);
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
        machine.set_register(A4, u64::from(Source::Transaction(SourceEntry::Output))); //source: 2 output
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
        machine.set_register(A4, u64::from(Source::Transaction(SourceEntry::Input))); //source: 1 input
        machine.set_register(A7, LOAD_CELL_SYSCALL_NUMBER); // syscall number

        let output_cell_data = Bytes::from(data);
        let output = build_cell_meta(
            CellOutputBuilder::from_data(&output_cell_data)
                .capacity(capacity_bytes!(100))
                .build(),
            output_cell_data,
        );
        let input_cell_data: Bytes = data.iter().rev().cloned().collect();
        let input_cell = build_resolved_input(
            CellOutputBuilder::from_data(&input_cell_data)
                .capacity(capacity_bytes!(100))
                .build(),
            input_cell_data,
        );
        let outputs = vec![output];
        let resolved_inputs = vec![input_cell.clone()];
        let resolved_deps = vec![];
        let group_inputs = vec![];
        let group_outputs = vec![];
        let mut load_cell = LoadCell::new(
            &outputs,
            &resolved_inputs,
            &resolved_deps,
            &group_inputs,
            &group_outputs,
        );

        let mut builder = FlatBufferBuilder::new();
        let fbs_offset = FbsCellOutput::build(&mut builder, &input_cell.cell().cell_output);
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
        machine.set_register(A4, u64::from(Source::Transaction(SourceEntry::Input))); // source: 1 input
        machine.set_register(A7, LOAD_CELL_SYSCALL_NUMBER); // syscall number

        let output_cell_data = Bytes::from(data);
        let output = build_cell_meta(
            CellOutputBuilder::from_data(&output_cell_data)
                .capacity(capacity_bytes!(100))
                .build(),
            output_cell_data,
        );

        let input_cell_data: Bytes = data.iter().rev().cloned().collect();
        let input_cell = build_resolved_input(
            CellOutputBuilder::from_data(&input_cell_data)
                .capacity(capacity_bytes!(100))
                .build(),
            input_cell_data,
        );
        let outputs = vec![output];
        let resolved_inputs = vec![input_cell.clone()];
        let resolved_deps = vec![];
        let group_inputs = vec![];
        let group_outputs = vec![];
        let mut load_cell = LoadCell::new(
            &outputs,
            &resolved_inputs,
            &resolved_deps,
            &group_inputs,
            &group_outputs,
        );

        let mut builder = FlatBufferBuilder::new();
        let fbs_offset = FbsCellOutput::build(&mut builder, &input_cell.cell().cell_output);
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

    fn _test_load_cell_capacity(capacity: Capacity) -> Result<(), TestCaseError> {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory<u64>>::default();
        let size_addr: u64 = 0;
        let addr: u64 = 100;

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, size_addr); // size_addr
        machine.set_register(A2, 0); // offset
        machine.set_register(A3, 0); //index
        machine.set_register(A4, u64::from(Source::Transaction(SourceEntry::Input))); //source: 1 input
        machine.set_register(A5, CellField::Capacity as u64); //field: 0 capacity
        machine.set_register(A7, LOAD_CELL_BY_FIELD_SYSCALL_NUMBER); // syscall number

        let input_cell = build_resolved_input(
            CellOutputBuilder::default().capacity(capacity).build(),
            Bytes::new(),
        );
        let outputs = vec![];
        let resolved_inputs = vec![input_cell.clone()];
        let resolved_deps = vec![];
        let group_inputs = vec![];
        let group_outputs = vec![];
        let mut load_cell = LoadCell::new(
            &outputs,
            &resolved_inputs,
            &resolved_deps,
            &group_inputs,
            &group_outputs,
        );

        prop_assert!(machine.memory_mut().store64(&size_addr, &16).is_ok());

        prop_assert!(load_cell.ecall(&mut machine).is_ok());
        prop_assert_eq!(machine.registers()[A0], u64::from(SUCCESS));

        prop_assert_eq!(machine.memory_mut().load64(&size_addr), Ok(8));

        let mut buffer = vec![];
        buffer.write_u64::<LittleEndian>(capacity.as_u64()).unwrap();

        for (i, addr) in (addr..addr + buffer.len() as u64).enumerate() {
            prop_assert_eq!(machine.memory_mut().load8(&addr), Ok(u64::from(buffer[i])));
        }
        Ok(())
    }

    proptest! {
        #[test]
        fn test_load_cell_capacity(capacity in any::<u64>()) {
            _test_load_cell_capacity(Capacity::shannons(capacity))?;
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
        machine.set_register(A4, u64::from(Source::Transaction(SourceEntry::Output))); //source: 2 output
        machine.set_register(A5, CellField::Type as u64); //field: 4 type
        machine.set_register(A7, LOAD_CELL_BY_FIELD_SYSCALL_NUMBER); // syscall number

        let output_cell = build_cell_meta(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(100))
                .build(),
            Bytes::new(),
        );
        let outputs = vec![output_cell];
        let resolved_inputs = vec![];
        let resolved_deps = vec![];
        let group_inputs = vec![];
        let group_outputs = vec![];
        let mut load_cell = LoadCell::new(
            &outputs,
            &resolved_inputs,
            &resolved_deps,
            &group_inputs,
            &group_outputs,
        );

        assert!(machine.memory_mut().store64(&size_addr, &100).is_ok());

        assert!(load_cell.ecall(&mut machine).is_ok());
        assert_eq!(machine.registers()[A0], u64::from(ITEM_MISSING));

        assert_eq!(machine.memory_mut().load64(&size_addr), Ok(100));

        for addr in addr..addr + 100 {
            assert_eq!(machine.memory_mut().load8(&addr), Ok(0));
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
        machine.set_register(A4, u64::from(Source::Transaction(SourceEntry::Dep))); //source: 3 dep
        machine.set_register(A5, CellField::DataHash as u64); //field: 2 data hash
        machine.set_register(A7, LOAD_CELL_BY_FIELD_SYSCALL_NUMBER); // syscall number

        let input_cell = build_resolved_input(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(1000))
                .build(),
            Bytes::new(),
        );
        let dep_cell_data = Bytes::from(data);
        let dep_cell = build_resolved_dep(
            CellOutputBuilder::from_data(&dep_cell_data)
                .capacity(capacity_bytes!(1000))
                .build(),
            dep_cell_data,
        );
        let outputs = vec![];
        let resolved_inputs = vec![input_cell.clone()];
        let resolved_deps = vec![dep_cell];
        let group_inputs = vec![];
        let group_outputs = vec![];
        let mut load_cell = LoadCell::new(
            &outputs,
            &resolved_inputs,
            &resolved_deps,
            &group_inputs,
            &group_outputs,
        );

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

    fn _test_load_header(data: &[u8]) -> Result<(), TestCaseError> {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory<u64>>::default();
        let size_addr: u64 = 0;
        let addr: u64 = 100;

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, size_addr); // size_addr
        machine.set_register(A2, 0); // offset
        machine.set_register(A3, 0); //index
        machine.set_register(A4, u64::from(Source::Transaction(SourceEntry::Dep))); //source: 3 dep
        machine.set_register(A7, LOAD_HEADER_SYSCALL_NUMBER); // syscall number

        let data_hash = blake2b_256(&data);
        let header = HeaderBuilder::default()
            .transactions_root(data_hash.into())
            .build();

        let mut builder = FlatBufferBuilder::new();
        let fbs_offset = FbsHeader::build(&mut builder, &header);
        builder.finish(fbs_offset, None);
        let header_correct_data = builder.finished_data();

        let dep_cell = ResolvedDep::header_only(header);
        let resolved_inputs = vec![];
        let resolved_deps = vec![dep_cell];
        let group_inputs = vec![];
        let mut load_cell = LoadHeader::new(&resolved_inputs, &resolved_deps, &group_inputs);

        prop_assert!(machine
            .memory_mut()
            .store64(&size_addr, &(header_correct_data.len() as u64 + 20))
            .is_ok());

        prop_assert!(load_cell.ecall(&mut machine).is_ok());
        prop_assert_eq!(machine.registers()[A0], u64::from(SUCCESS));

        prop_assert_eq!(
            machine.memory_mut().load64(&size_addr),
            Ok(header_correct_data.len() as u64)
        );

        for (i, addr) in (addr..addr + header_correct_data.len() as u64).enumerate() {
            prop_assert_eq!(
                machine.memory_mut().load8(&addr),
                Ok(u64::from(header_correct_data[i]))
            );
        }
        Ok(())
    }

    proptest! {
        #[test]
        fn test_load_header(ref data in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_header(data)?;
        }
    }

    fn _test_load_tx_hash(data: &[u8]) -> Result<(), TestCaseError> {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory<u64>>::default();
        let size_addr: u64 = 0;
        let addr: u64 = 100;

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, size_addr); // size_addr
        machine.set_register(A2, 0); // offset
        machine.set_register(A7, LOAD_TX_HASH_SYSCALL_NUMBER); // syscall number

        let hash = blake2b_256(&data);
        let mut load_tx_hash = LoadTxHash::new(&hash);

        prop_assert!(machine
            .memory_mut()
            .store64(&size_addr, &(hash.len() as u64 + 20))
            .is_ok());

        prop_assert!(load_tx_hash.ecall(&mut machine).is_ok());
        prop_assert_eq!(machine.registers()[A0], u64::from(SUCCESS));

        prop_assert_eq!(
            machine.memory_mut().load64(&size_addr),
            Ok(hash.len() as u64)
        );

        for (i, addr) in (addr..addr + hash.len() as u64).enumerate() {
            prop_assert_eq!(machine.memory_mut().load8(&addr), Ok(u64::from(hash[i])));
        }
        Ok(())
    }

    proptest! {
        #[test]
        fn test_load_tx_hash(ref data in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_tx_hash(data)?;
        }
    }

    fn _test_load_current_script_hash(data: &[u8]) -> Result<(), TestCaseError> {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory<u64>>::default();
        let size_addr: u64 = 0;
        let addr: u64 = 100;

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, size_addr); // size_addr
        machine.set_register(A2, 0); // offset
        machine.set_register(A7, LOAD_SCRIPT_HASH_SYSCALL_NUMBER); // syscall number

        let script = Script::new(vec![Bytes::from(data)], H256::zero(), ScriptHashType::Data);
        let h = script.hash();
        let hash = h.as_bytes();
        let mut load_script_hash = LoadScriptHash::new(hash);

        prop_assert!(machine.memory_mut().store64(&size_addr, &64).is_ok());

        prop_assert!(load_script_hash.ecall(&mut machine).is_ok());
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

        prop_assert!(load_script_hash.ecall(&mut machine).is_ok());
        prop_assert_eq!(machine.registers()[A0], u64::from(SUCCESS));

        prop_assert_eq!(
            machine.memory_mut().load64(&size_addr),
            Ok(hash.len() as u64)
        );
        Ok(())
    }

    proptest! {
        #[test]
        fn test_load_current_script_hash(ref data in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_current_script_hash(data)?;
        }
    }

    fn _test_load_input_lock_script_hash(data: &[u8]) -> Result<(), TestCaseError> {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory<u64>>::default();
        let size_addr: u64 = 0;
        let addr: u64 = 100;

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, size_addr); // size_addr
        machine.set_register(A2, 0); // offset
        machine.set_register(A3, 0); //index
        machine.set_register(A4, u64::from(Source::Transaction(SourceEntry::Input))); //source: 1 input
        machine.set_register(A5, CellField::LockHash as u64); //field: 2 lock hash
        machine.set_register(A7, LOAD_CELL_BY_FIELD_SYSCALL_NUMBER); // syscall number

        let script = Script::new(vec![Bytes::from(data)], H256::zero(), ScriptHashType::Data);
        let h = script.hash();
        let hash = h.as_bytes();

        let input_cell = build_resolved_input(
            CellOutputBuilder::default()
                .capacity(capacity_bytes!(1000))
                .lock(script)
                .build(),
            Bytes::new(),
        );
        let outputs = vec![];
        let resolved_inputs = vec![input_cell.clone()];
        let resolved_deps = vec![];
        let group_inputs = vec![];
        let group_outputs = vec![];
        let mut load_cell = LoadCell::new(
            &outputs,
            &resolved_inputs,
            &resolved_deps,
            &group_inputs,
            &group_outputs,
        );

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
        Ok(())
    }

    proptest! {
        #[test]
        fn test_load_input_lock_script_hash(ref data in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_input_lock_script_hash(data)?;
        }
    }

    fn _test_load_witness(data: &[u8]) -> Result<(), TestCaseError> {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory<u64>>::default();
        let size_addr: u64 = 0;
        let addr: u64 = 100;

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, size_addr); // size_addr
        machine.set_register(A2, 0); // offset
        machine.set_register(A3, 0); //index
        machine.set_register(A4, u64::from(Source::Transaction(SourceEntry::Input))); //source
        machine.set_register(A7, LOAD_WITNESS_SYSCALL_NUMBER); // syscall number

        let witness = vec![data.into()];

        let mut builder = FlatBufferBuilder::new();
        let fbs_offset = FbsWitness::build(&mut builder, &witness);
        builder.finish(fbs_offset, None);
        let witness_correct_data = builder.finished_data();

        let witnesses = vec![witness];
        let group_inputs = vec![];
        let mut load_witness = LoadWitness::new(&witnesses, &group_inputs);

        prop_assert!(machine
            .memory_mut()
            .store64(&size_addr, &(witness_correct_data.len() as u64 + 20))
            .is_ok());

        prop_assert!(load_witness.ecall(&mut machine).is_ok());
        prop_assert_eq!(machine.registers()[A0], u64::from(SUCCESS));

        prop_assert_eq!(
            machine.memory_mut().load64(&size_addr),
            Ok(witness_correct_data.len() as u64)
        );

        for (i, addr) in (addr..addr + witness_correct_data.len() as u64).enumerate() {
            prop_assert_eq!(
                machine.memory_mut().load8(&addr),
                Ok(u64::from(witness_correct_data[i]))
            );
        }
        Ok(())
    }

    proptest! {
        #[test]
        fn test_load_witness(ref data in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_witness(data)?;
        }
    }

    fn _test_load_group_witness(data: &[u8]) -> Result<(), TestCaseError> {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory<u64>>::default();
        let size_addr: u64 = 0;
        let addr: u64 = 100;

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, size_addr); // size_addr
        machine.set_register(A2, 0); // offset
        machine.set_register(A3, 0); //index
        machine.set_register(A4, u64::from(Source::Group(SourceEntry::Input))); //source
        machine.set_register(A7, LOAD_WITNESS_SYSCALL_NUMBER); // syscall number

        let witness = vec![data.into()];

        let mut builder = FlatBufferBuilder::new();
        let fbs_offset = FbsWitness::build(&mut builder, &witness);
        builder.finish(fbs_offset, None);
        let witness_correct_data = builder.finished_data();

        let dummy_witness = vec![];
        let witnesses = vec![dummy_witness, witness];
        let group_inputs = vec![1];
        let mut load_witness = LoadWitness::new(&witnesses, &group_inputs);

        prop_assert!(machine
            .memory_mut()
            .store64(&size_addr, &(witness_correct_data.len() as u64 + 20))
            .is_ok());

        prop_assert!(load_witness.ecall(&mut machine).is_ok());
        prop_assert_eq!(machine.registers()[A0], u64::from(SUCCESS));

        prop_assert_eq!(
            machine.memory_mut().load64(&size_addr),
            Ok(witness_correct_data.len() as u64)
        );

        for (i, addr) in (addr..addr + witness_correct_data.len() as u64).enumerate() {
            prop_assert_eq!(
                machine.memory_mut().load8(&addr),
                Ok(u64::from(witness_correct_data[i]))
            );
        }
        Ok(())
    }

    proptest! {
        #[test]
        fn test_load_group_witness(ref data in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_group_witness(data)?;
        }
    }

    fn _test_load_cell_data(
        load_data_type: LoadDataType,
        data: &[u8],
    ) -> Result<(), TestCaseError> {
        let mut machine = DefaultCoreMachine::<u64, WXorXMemory<u64, SparseMemory<u64>>>::default();
        let addr = 4096;
        let addr_size = 4096;

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, addr_size); // size
        machine.set_register(A2, 0); // content offset
        machine.set_register(A3, data.len() as u64); // content size
        machine.set_register(A4, 0); //index
        machine.set_register(A5, u64::from(Source::Transaction(SourceEntry::Dep))); //source
        machine.set_register(A6, load_data_type as u64); // memory type
        machine.set_register(A7, LOAD_CELL_DATA_SYSCALL_NUMBER); // syscall number

        let dep_cell_data = Bytes::from(data);
        let dep_cell = build_resolved_dep(
            CellOutputBuilder::from_data(&dep_cell_data)
                .capacity(capacity_bytes!(10000))
                .build(),
            dep_cell_data,
        );

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let outputs = vec![];
        let resolved_inputs = vec![];
        let resolved_deps = vec![dep_cell];
        let group_inputs = vec![];
        let group_outputs = vec![];
        let mut load_code = LoadCellData::new(
            &data_loader,
            &outputs,
            &resolved_inputs,
            &resolved_deps,
            &group_inputs,
            &group_outputs,
        );

        prop_assert!(machine.memory_mut().store_byte(addr, addr_size, 1).is_ok());

        prop_assert!(load_code.ecall(&mut machine).is_ok());
        prop_assert_eq!(machine.registers()[A0], u64::from(SUCCESS));

        prop_assert_eq!(
            machine
                .memory_mut()
                .fetch_flag(addr / RISCV_PAGESIZE as u64),
            Ok(load_data_type.memory_flags())
        );
        for (i, addr) in (addr..addr + data.len() as u64).enumerate() {
            prop_assert_eq!(machine.memory_mut().load8(&addr), Ok(u64::from(data[i])));
        }
        if (data.len() as u64) < addr_size {
            for i in (data.len() as u64)..addr_size {
                prop_assert_eq!(machine.memory_mut().load8(&(addr + i)), Ok(0));
            }
        }
        Ok(())
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 10, .. ProptestConfig::default()
        })]
        #[test]
        fn test_load_code(ref data in any_with::<Vec<u8>>(size_range(4096).lift())) {
            _test_load_cell_data(LoadDataType::Code, data)?;
        }

        #[test]
        fn test_load_data(ref data in any_with::<Vec<u8>>(size_range(4096).lift())) {
            _test_load_cell_data(LoadDataType::Data, data)?;
        }
    }

    fn _test_load_cell_data_on_freezed_memory(
        load_data_type: LoadDataType,
        data: &[u8],
    ) -> Result<(), TestCaseError> {
        let mut machine = DefaultCoreMachine::<u64, WXorXMemory<u64, SparseMemory<u64>>>::default();
        let addr = 4096;
        let addr_size = 4096;

        prop_assert!(machine
            .memory_mut()
            .init_pages(addr, addr_size, FLAG_EXECUTABLE | FLAG_FREEZED, None, 0)
            .is_ok());

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, addr_size); // size
        machine.set_register(A2, 0); // content offset
        machine.set_register(A3, data.len() as u64); // content size
        machine.set_register(A4, 0); //index
        machine.set_register(A5, u64::from(Source::Transaction(SourceEntry::Dep))); //source
        machine.set_register(A6, load_data_type as u64); // memory type
        machine.set_register(A7, LOAD_CELL_DATA_SYSCALL_NUMBER); // syscall number

        let dep_cell_data = Bytes::from(data);
        let dep_cell = build_resolved_dep(
            CellOutputBuilder::from_data(&dep_cell_data)
                .capacity(capacity_bytes!(10000))
                .build(),
            dep_cell_data,
        );

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let outputs = vec![];
        let resolved_inputs = vec![];
        let resolved_deps = vec![dep_cell];
        let group_inputs = vec![];
        let group_outputs = vec![];
        let mut load_code = LoadCellData::new(
            &data_loader,
            &outputs,
            &resolved_inputs,
            &resolved_deps,
            &group_inputs,
            &group_outputs,
        );

        prop_assert!(load_code.ecall(&mut machine).is_err());

        for i in addr..addr + addr_size {
            assert_eq!(machine.memory_mut().load8(&i), Ok(0));
        }
        Ok(())
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 10, .. ProptestConfig::default()
        })]
        #[test]
        fn test_load_code_on_freezed_memory(ref data in any_with::<Vec<u8>>(size_range(4096).lift())) {
            _test_load_cell_data_on_freezed_memory(LoadDataType::Code, data)?;
        }

        #[test]
        fn test_load_data_on_freezed_memory(ref data in any_with::<Vec<u8>>(size_range(4096).lift())) {
            _test_load_cell_data_on_freezed_memory(LoadDataType::Data, data)?;
        }
    }

    #[test]
    fn test_load_code_unaligned_error() {
        let mut machine = DefaultCoreMachine::<u64, WXorXMemory<u64, SparseMemory<u64>>>::default();
        let addr = 4097;
        let addr_size = 4096;
        let data = [2; 32];

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, addr_size); // size
        machine.set_register(A2, 0); // content offset
        machine.set_register(A3, data.len() as u64); // content size
        machine.set_register(A4, 0); //index
        machine.set_register(A5, u64::from(Source::Transaction(SourceEntry::Dep))); //source
        machine.set_register(A6, LoadDataType::Code as u64); // memory type
        machine.set_register(A7, LOAD_CELL_DATA_SYSCALL_NUMBER); // syscall number
        let dep_cell_data = Bytes::from(&data[..]);
        let dep_cell = build_resolved_dep(
            CellOutputBuilder::from_data(&dep_cell_data)
                .capacity(capacity_bytes!(10000))
                .build(),
            dep_cell_data,
        );

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let outputs = vec![];
        let resolved_inputs = vec![];
        let resolved_deps = vec![dep_cell];
        let group_inputs = vec![];
        let group_outputs = vec![];
        let mut load_code = LoadCellData::new(
            &data_loader,
            &outputs,
            &resolved_inputs,
            &resolved_deps,
            &group_inputs,
            &group_outputs,
        );

        assert!(machine.memory_mut().store_byte(addr, addr_size, 1).is_ok());

        assert!(load_code.ecall(&mut machine).is_err());

        for i in addr..addr + addr_size {
            assert_eq!(machine.memory_mut().load8(&i), Ok(1));
        }
    }

    #[test]
    fn test_load_code_slice_out_of_bound_error() {
        let mut machine = DefaultCoreMachine::<u64, WXorXMemory<u64, SparseMemory<u64>>>::default();
        let addr = 4096;
        let addr_size = 4096;
        let data = [2; 32];

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, addr_size); // size
        machine.set_register(A2, 0); // content offset
        machine.set_register(A3, data.len() as u64 + 3); // content size
        machine.set_register(A4, 0); //index
        machine.set_register(A5, u64::from(Source::Transaction(SourceEntry::Dep))); //source
        machine.set_register(A6, LoadDataType::Code as u64); // memory type
        machine.set_register(A7, LOAD_CELL_DATA_SYSCALL_NUMBER); // syscall number

        let dep_cell_data = Bytes::from(&data[..]);
        let dep_cell = build_resolved_dep(
            CellOutputBuilder::from_data(&dep_cell_data)
                .capacity(capacity_bytes!(10000))
                .build(),
            dep_cell_data,
        );

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let outputs = vec![];
        let resolved_inputs = vec![];
        let resolved_deps = vec![dep_cell];
        let group_inputs = vec![];
        let group_outputs = vec![];
        let mut load_code = LoadCellData::new(
            &data_loader,
            &outputs,
            &resolved_inputs,
            &resolved_deps,
            &group_inputs,
            &group_outputs,
        );

        assert!(machine.memory_mut().store_byte(addr, addr_size, 1).is_ok());

        assert!(load_code.ecall(&mut machine).is_ok());
        assert_eq!(machine.registers()[A0], u64::from(SLICE_OUT_OF_BOUND));

        for i in addr..addr + addr_size {
            assert_eq!(machine.memory_mut().load8(&i), Ok(1));
        }
    }

    #[test]
    fn test_load_code_not_enough_space_error() {
        let mut machine = DefaultCoreMachine::<u64, WXorXMemory<u64, SparseMemory<u64>>>::default();
        let addr = 4096;
        let addr_size = 4096;

        let mut data = vec![];
        data.resize(8000, 2);

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, addr_size); // size
        machine.set_register(A2, 0); // content offset
        machine.set_register(A3, data.len() as u64); // content size
        machine.set_register(A4, 0); //index
        machine.set_register(A5, u64::from(Source::Transaction(SourceEntry::Dep))); //source
        machine.set_register(A6, LoadDataType::Code as u64); // memory type
        machine.set_register(A7, LOAD_CELL_DATA_SYSCALL_NUMBER); // syscall number

        let dep_cell_data = Bytes::from(&data[..]);
        let dep_cell = build_resolved_dep(
            CellOutputBuilder::from_data(&dep_cell_data)
                .capacity(capacity_bytes!(10000))
                .build(),
            dep_cell_data,
        );

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let outputs = vec![];
        let resolved_inputs = vec![];
        let resolved_deps = vec![dep_cell];
        let group_inputs = vec![];
        let group_outputs = vec![];
        let mut load_code = LoadCellData::new(
            &data_loader,
            &outputs,
            &resolved_inputs,
            &resolved_deps,
            &group_inputs,
            &group_outputs,
        );

        assert!(machine.memory_mut().store_byte(addr, addr_size, 1).is_ok());

        assert!(load_code.ecall(&mut machine).is_ok());
        assert_eq!(machine.registers()[A0], u64::from(SLICE_OUT_OF_BOUND));

        for i in addr..addr + addr_size {
            assert_eq!(machine.memory_mut().load8(&i), Ok(1));
        }
    }
}
