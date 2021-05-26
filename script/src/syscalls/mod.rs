mod current_cycles;
mod debugger;
mod exec;
mod load_cell;
mod load_cell_data;
mod load_header;
mod load_input;
mod load_script;
mod load_script_hash;
mod load_tx;
mod load_witness;
mod utils;
mod vm_version;

pub use self::current_cycles::CurrentCycles;
pub use self::debugger::Debugger;
pub use self::exec::Exec;
pub use self::load_cell::LoadCell;
pub use self::load_cell_data::LoadCellData;
pub use self::load_header::LoadHeader;
pub use self::load_input::LoadInput;
pub use self::load_script::LoadScript;
pub use self::load_script_hash::LoadScriptHash;
pub use self::load_tx::LoadTx;
pub use self::load_witness::LoadWitness;
pub use self::vm_version::VMVersion;

use ckb_vm::Error;

pub const SUCCESS: u8 = 0;
// INDEX_OUT_OF_BOUND is returned when requesting the 4th output in a transaction
// with only 3 outputs; while ITEM_MISSING is returned when requesting (for example)
// the type field on an output without type script, or requesting the cell data
// for a dep OutPoint which only references a block header.
pub const INDEX_OUT_OF_BOUND: u8 = 1;
pub const ITEM_MISSING: u8 = 2;
pub const SLICE_OUT_OF_BOUND: u8 = 3;
pub const WRONG_FORMAT: u8 = 4;

pub const VM_VERSION: u64 = 2041;
pub const CURRENT_CYCLES: u64 = 2042;
pub const EXEC: u64 = 2043;
pub const LOAD_TRANSACTION_SYSCALL_NUMBER: u64 = 2051;
pub const LOAD_SCRIPT_SYSCALL_NUMBER: u64 = 2052;
pub const LOAD_TX_HASH_SYSCALL_NUMBER: u64 = 2061;
pub const LOAD_SCRIPT_HASH_SYSCALL_NUMBER: u64 = 2062;
pub const LOAD_CELL_SYSCALL_NUMBER: u64 = 2071;
pub const LOAD_HEADER_SYSCALL_NUMBER: u64 = 2072;
pub const LOAD_INPUT_SYSCALL_NUMBER: u64 = 2073;
pub const LOAD_WITNESS_SYSCALL_NUMBER: u64 = 2074;
pub const LOAD_CELL_BY_FIELD_SYSCALL_NUMBER: u64 = 2081;
pub const LOAD_HEADER_BY_FIELD_SYSCALL_NUMBER: u64 = 2082;
pub const LOAD_INPUT_BY_FIELD_SYSCALL_NUMBER: u64 = 2083;
pub const LOAD_CELL_DATA_AS_CODE_SYSCALL_NUMBER: u64 = 2091;
pub const LOAD_CELL_DATA_SYSCALL_NUMBER: u64 = 2092;
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

// While all fields here share the same prefix for now, later
// we might add other fields from the header which won't have
// this prefix.
#[allow(clippy::enum_variant_names)]
#[derive(Debug, PartialEq, Clone, Copy, Eq)]
enum HeaderField {
    EpochNumber = 0,
    EpochStartBlockNumber = 1,
    EpochLength = 2,
}

impl HeaderField {
    fn parse_from_u64(i: u64) -> Result<HeaderField, Error> {
        match i {
            0 => Ok(HeaderField::EpochNumber),
            1 => Ok(HeaderField::EpochStartBlockNumber),
            2 => Ok(HeaderField::EpochLength),
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
    // Cell dep
    CellDep,
    // Header dep
    HeaderDep,
}

impl From<SourceEntry> for u64 {
    fn from(s: SourceEntry) -> u64 {
        match s {
            SourceEntry::Input => 1,
            SourceEntry::Output => 2,
            SourceEntry::CellDep => 3,
            SourceEntry::HeaderDep => 4,
        }
    }
}

impl SourceEntry {
    fn parse_from_u64(i: u64) -> Result<SourceEntry, Error> {
        match i {
            1 => Ok(SourceEntry::Input),
            2 => Ok(SourceEntry::Output),
            3 => Ok(SourceEntry::CellDep),
            4 => Ok(SourceEntry::HeaderDep),
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

#[cfg(test)]
mod tests {
    use super::*;
    use byteorder::{ByteOrder, LittleEndian, WriteBytesExt};
    use ckb_db::RocksDB;
    use ckb_db_schema::COLUMNS;
    use ckb_hash::blake2b_256;
    use ckb_store::{data_loader_wrapper::DataLoaderWrapper, ChainDB};
    use ckb_traits::{CellDataProvider, HeaderProvider};
    use ckb_types::{
        bytes::Bytes,
        core::{
            cell::CellMeta, Capacity, EpochNumberWithFraction, HeaderBuilder, HeaderView,
            ScriptHashType, TransactionBuilder,
        },
        packed::{Byte32, CellOutput, OutPoint, Script, ScriptBuilder},
        prelude::*,
        H256,
    };
    use ckb_vm::{machine::DefaultCoreMachine, SupportMachine};
    use ckb_vm::{
        machine::{VERSION0, VERSION1},
        memory::{FLAG_DIRTY, FLAG_EXECUTABLE, FLAG_FREEZED, FLAG_WRITABLE},
        registers::{A0, A1, A2, A3, A4, A5, A7},
        CoreMachine, Error as VMError, Memory, SparseMemory, Syscalls, WXorXMemory, ISA_IMC,
        RISCV_PAGESIZE,
    };
    use proptest::{collection::size_range, prelude::*};
    use std::collections::HashMap;

    fn new_store() -> ChainDB {
        ChainDB::new(RocksDB::open_tmp(COLUMNS), Default::default())
    }

    fn build_cell_meta(capacity_bytes: usize, data: Bytes) -> CellMeta {
        let capacity = Capacity::bytes(capacity_bytes).expect("capacity bytes overflow");
        let builder = CellOutput::new_builder().capacity(capacity.pack());
        let data_hash = CellOutput::calc_data_hash(&data);
        CellMeta {
            out_point: OutPoint::default(),
            transaction_info: None,
            cell_output: builder.build(),
            data_bytes: data.len() as u64,
            mem_cell_data: Some(data),
            mem_cell_data_hash: Some(data_hash),
        }
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

        let output_cell_data = Bytes::from(data.to_owned());
        let output = build_cell_meta(100, output_cell_data);
        let input_cell_data: Bytes = data.iter().rev().cloned().collect();
        let input_cell = build_cell_meta(100, input_cell_data);
        let outputs = vec![output];
        let resolved_inputs = vec![input_cell];
        let resolved_cell_deps = vec![];
        let group_inputs = vec![];
        let group_outputs = vec![];
        let mut load_cell = LoadCell::new(
            &outputs,
            &resolved_inputs,
            &resolved_cell_deps,
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

        let output_cell_data = Bytes::from(data.to_owned());
        let output = build_cell_meta(100, output_cell_data);
        let input_cell_data: Bytes = data.iter().rev().cloned().collect();
        let input_cell = build_cell_meta(100, input_cell_data);
        let outputs = vec![output.clone()];
        let resolved_inputs = vec![input_cell.clone()];
        let resolved_cell_deps = vec![];
        let group_inputs = vec![];
        let group_outputs = vec![];
        let mut load_cell = LoadCell::new(
            &outputs,
            &resolved_inputs,
            &resolved_cell_deps,
            &group_inputs,
            &group_outputs,
        );

        let input_correct_data = input_cell.cell_output.as_slice();
        let output_correct_data = output.cell_output.as_slice();

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

        let output_cell_data = Bytes::from(data.to_owned());
        let output = build_cell_meta(100, output_cell_data);
        let input_cell_data: Bytes = data.iter().rev().cloned().collect();
        let input_cell = build_cell_meta(100, input_cell_data);
        let outputs = vec![output];
        let resolved_inputs = vec![input_cell.clone()];
        let resolved_cell_deps = vec![];
        let group_inputs = vec![];
        let group_outputs = vec![];
        let mut load_cell = LoadCell::new(
            &outputs,
            &resolved_inputs,
            &resolved_cell_deps,
            &group_inputs,
            &group_outputs,
        );

        let input_correct_data = input_cell.cell_output.as_slice();

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

    fn _test_load_cell_partial(data: &[u8], offset: u64) -> Result<(), TestCaseError> {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory<u64>>::default();
        let size_addr: u64 = 0;
        let addr: u64 = 100;

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, size_addr); // size_addr
        machine.set_register(A2, offset); // offset
        machine.set_register(A3, 0); // index
        machine.set_register(A4, u64::from(Source::Transaction(SourceEntry::Input))); // source: 1 input
        machine.set_register(A7, LOAD_CELL_SYSCALL_NUMBER); // syscall number

        let output_cell_data = Bytes::from(data.to_owned());
        let output = build_cell_meta(100, output_cell_data);

        let input_cell_data: Bytes = data.iter().rev().cloned().collect();
        let input_cell = build_cell_meta(100, input_cell_data);
        let outputs = vec![output];
        let resolved_inputs = vec![input_cell.clone()];
        let resolved_cell_deps = vec![];
        let group_inputs = vec![];
        let group_outputs = vec![];
        let mut load_cell = LoadCell::new(
            &outputs,
            &resolved_inputs,
            &resolved_cell_deps,
            &group_inputs,
            &group_outputs,
        );

        let input_correct_data = input_cell.cell_output.as_slice();

        prop_assert!(machine
            .memory_mut()
            .store64(&size_addr, &(input_correct_data.len() as u64))
            .is_ok());

        prop_assert!(load_cell.ecall(&mut machine).is_ok());
        prop_assert_eq!(machine.registers()[A0], u64::from(SUCCESS));

        for (i, addr) in
            (addr..addr + (input_correct_data.len() as u64).saturating_sub(offset)).enumerate()
        {
            prop_assert_eq!(
                machine.memory_mut().load8(&addr),
                Ok(u64::from(input_correct_data[i + offset as usize]))
            );
        }
        Ok(())
    }

    proptest! {
        #[test]
        fn test_load_cell_partial(ref data in any_with::<Vec<u8>>(size_range(1000).lift()), offset in 0u64..2000) {
            _test_load_cell_partial(data, offset)?;
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

        let data = Bytes::new();
        let data_hash = CellOutput::calc_data_hash(&data);
        let input_cell = CellMeta {
            out_point: OutPoint::default(),
            transaction_info: None,
            cell_output: CellOutput::new_builder().capacity(capacity.pack()).build(),
            data_bytes: 0,
            mem_cell_data: Some(data),
            mem_cell_data_hash: Some(data_hash),
        };
        let outputs = vec![];
        let resolved_inputs = vec![input_cell];
        let resolved_cell_deps = vec![];
        let group_inputs = vec![];
        let group_outputs = vec![];
        let mut load_cell = LoadCell::new(
            &outputs,
            &resolved_inputs,
            &resolved_cell_deps,
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

        let output_cell = build_cell_meta(100, Bytes::new());
        let outputs = vec![output_cell];
        let resolved_inputs = vec![];
        let resolved_cell_deps = vec![];
        let group_inputs = vec![];
        let group_outputs = vec![];
        let mut load_cell = LoadCell::new(
            &outputs,
            &resolved_inputs,
            &resolved_cell_deps,
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

    struct MockDataLoader {
        headers: HashMap<Byte32, HeaderView>,
    }

    impl CellDataProvider for MockDataLoader {
        fn get_cell_data(&self, _out_point: &OutPoint) -> Option<Bytes> {
            None
        }

        fn get_cell_data_hash(&self, _out_point: &OutPoint) -> Option<Byte32> {
            None
        }
    }

    impl HeaderProvider for MockDataLoader {
        fn get_header(&self, block_hash: &Byte32) -> Option<HeaderView> {
            self.headers.get(block_hash).cloned()
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
        machine.set_register(A4, u64::from(Source::Transaction(SourceEntry::HeaderDep))); //source: 4 header
        machine.set_register(A7, LOAD_HEADER_SYSCALL_NUMBER); // syscall number

        let data_hash = blake2b_256(&data).pack();
        let header = HeaderBuilder::default()
            .transactions_root(data_hash)
            .build();

        let header_correct_bytes = header.data();
        let header_correct_data = header_correct_bytes.as_slice();

        let mut headers = HashMap::default();
        headers.insert(header.hash(), header.clone());
        let data_loader = MockDataLoader { headers };
        let header_deps = vec![header.hash()];
        let resolved_inputs = vec![];
        let resolved_cell_deps = vec![];
        let group_inputs = vec![];
        let mut load_header = LoadHeader::new(
            &data_loader,
            header_deps.pack(),
            &resolved_inputs,
            &resolved_cell_deps,
            &group_inputs,
        );

        prop_assert!(machine
            .memory_mut()
            .store64(&size_addr, &(header_correct_data.len() as u64 + 20))
            .is_ok());

        prop_assert!(load_header.ecall(&mut machine).is_ok());
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

    fn _test_load_epoch_number(data: &[u8]) -> Result<(), TestCaseError> {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory<u64>>::default();
        let size_addr: u64 = 0;
        let addr: u64 = 100;

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, size_addr); // size_addr
        machine.set_register(A2, 0); // offset
        machine.set_register(A3, 0); //index
        machine.set_register(A4, u64::from(Source::Transaction(SourceEntry::HeaderDep))); //source: 4 header
        machine.set_register(A5, HeaderField::EpochNumber as u64);
        machine.set_register(A7, LOAD_HEADER_BY_FIELD_SYSCALL_NUMBER); // syscall number

        let data_hash: H256 = blake2b_256(&data).into();
        let header = HeaderBuilder::default()
            .transactions_root(data_hash.pack())
            .number(2000.pack())
            .epoch(EpochNumberWithFraction::new(1, 40, 1000).pack())
            .build();

        let mut correct_data = [0u8; 8];
        LittleEndian::write_u64(&mut correct_data, 1);

        let mut headers = HashMap::default();
        headers.insert(header.hash(), header.clone());
        let data_loader = MockDataLoader { headers };
        let header_deps = vec![header.hash()];
        let resolved_inputs = vec![];
        let resolved_cell_deps = vec![];
        let group_inputs = vec![];
        let mut load_header = LoadHeader::new(
            &data_loader,
            header_deps.pack(),
            &resolved_inputs,
            &resolved_cell_deps,
            &group_inputs,
        );

        prop_assert!(machine
            .memory_mut()
            .store64(&size_addr, &(correct_data.len() as u64 + 20))
            .is_ok());

        prop_assert!(load_header.ecall(&mut machine).is_ok());
        prop_assert_eq!(machine.registers()[A0], u64::from(SUCCESS));

        prop_assert_eq!(
            machine.memory_mut().load64(&size_addr),
            Ok(correct_data.len() as u64)
        );

        Ok(())
    }

    proptest! {
        #[test]
        fn test_load_epoch_number(ref data in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_epoch_number(data)?;
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

        let transaction_view = TransactionBuilder::default()
            .output_data(data.pack())
            .build();

        let hash = transaction_view.hash();
        let hash_len = 32u64;
        let mut load_tx = LoadTx::new(&transaction_view);

        prop_assert!(machine
            .memory_mut()
            .store64(&size_addr, &(hash_len + 20))
            .is_ok());

        prop_assert!(load_tx.ecall(&mut machine).is_ok());
        prop_assert_eq!(machine.registers()[A0], u64::from(SUCCESS));

        prop_assert_eq!(machine.memory_mut().load64(&size_addr), Ok(hash_len));

        for (i, addr) in (addr..addr + hash_len as u64).enumerate() {
            prop_assert_eq!(
                machine.memory_mut().load8(&addr),
                Ok(u64::from(hash.as_slice()[i]))
            );
        }
        Ok(())
    }

    proptest! {
        #[test]
        fn test_load_tx_hash(ref data in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_tx_hash(data)?;
        }
    }

    fn _test_load_tx(data: &[u8]) -> Result<(), TestCaseError> {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory<u64>>::default();
        let size_addr: u64 = 0;
        let addr: u64 = 100;

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, size_addr); // size_addr
        machine.set_register(A2, 0); // offset
        machine.set_register(A7, LOAD_TRANSACTION_SYSCALL_NUMBER); // syscall number

        let transaction_view = TransactionBuilder::default()
            .output_data(data.pack())
            .build();

        let tx = transaction_view.data();
        let tx_len = transaction_view.data().as_slice().len() as u64;
        let mut load_tx = LoadTx::new(&transaction_view);

        prop_assert!(machine
            .memory_mut()
            .store64(&size_addr, &(tx_len + 20))
            .is_ok());

        prop_assert!(load_tx.ecall(&mut machine).is_ok());
        prop_assert_eq!(machine.registers()[A0], u64::from(SUCCESS));

        prop_assert_eq!(machine.memory_mut().load64(&size_addr), Ok(tx_len));

        for (i, addr) in (addr..addr + tx_len as u64).enumerate() {
            prop_assert_eq!(
                machine.memory_mut().load8(&addr),
                Ok(u64::from(tx.as_slice()[i]))
            );
        }
        Ok(())
    }

    proptest! {
        #[test]
        fn test_load_tx(ref data in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_tx(data)?;
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

        let script = Script::new_builder()
            .args(Bytes::from(data.to_owned()).pack())
            .hash_type(ScriptHashType::Data.into())
            .build();
        let hash = script.calc_script_hash();
        let data = hash.raw_data();
        let mut load_script_hash = LoadScriptHash::new(hash);

        prop_assert!(machine.memory_mut().store64(&size_addr, &64).is_ok());

        prop_assert!(load_script_hash.ecall(&mut machine).is_ok());
        prop_assert_eq!(machine.registers()[A0], u64::from(SUCCESS));

        prop_assert_eq!(
            machine.memory_mut().load64(&size_addr),
            Ok(data.len() as u64)
        );

        for (i, addr) in (addr..addr + data.len() as u64).enumerate() {
            prop_assert_eq!(machine.memory_mut().load8(&addr), Ok(u64::from(data[i])));
        }

        machine.set_register(A0, addr); // addr
        prop_assert!(machine.memory_mut().store64(&size_addr, &0).is_ok());

        prop_assert!(load_script_hash.ecall(&mut machine).is_ok());
        prop_assert_eq!(machine.registers()[A0], u64::from(SUCCESS));

        prop_assert_eq!(
            machine.memory_mut().load64(&size_addr),
            Ok(data.len() as u64)
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

        let script = Script::new_builder()
            .args(Bytes::from(data.to_owned()).pack())
            .hash_type(ScriptHashType::Data.into())
            .build();
        let h = script.calc_script_hash();
        let hash = h.as_bytes();

        let mut input_cell = build_cell_meta(1000, Bytes::new());
        let output_with_lock = input_cell
            .cell_output
            .clone()
            .as_builder()
            .lock(script)
            .build();
        input_cell.cell_output = output_with_lock;
        let outputs = vec![];
        let resolved_inputs = vec![input_cell];
        let resolved_cell_deps = vec![];
        let group_inputs = vec![];
        let group_outputs = vec![];
        let mut load_cell = LoadCell::new(
            &outputs,
            &resolved_inputs,
            &resolved_cell_deps,
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

    fn _test_load_witness(data: &[u8], source: SourceEntry) -> Result<(), TestCaseError> {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory<u64>>::default();
        let size_addr: u64 = 0;
        let addr: u64 = 100;

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, size_addr); // size_addr
        machine.set_register(A2, 0); // offset
        machine.set_register(A3, 0); //index
        machine.set_register(A4, u64::from(Source::Transaction(source))); //source
        machine.set_register(A7, LOAD_WITNESS_SYSCALL_NUMBER); // syscall number

        let witness = Bytes::from(data.to_owned()).pack();

        let witness_correct_data = witness.raw_data();

        let witnesses = vec![witness];
        let group_inputs = vec![];
        let group_outputs = vec![];
        let mut load_witness = LoadWitness::new(witnesses.pack(), &group_inputs, &group_outputs);

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
        fn test_load_witness_by_input(ref data in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_witness(data, SourceEntry::Input)?;
        }

        #[test]
        fn test_load_witness_by_output(ref data in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_witness(data, SourceEntry::Output)?;
        }
    }

    fn _test_load_group_witness(data: &[u8], source: SourceEntry) -> Result<(), TestCaseError> {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory<u64>>::default();
        let size_addr: u64 = 0;
        let addr: u64 = 100;

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, size_addr); // size_addr
        machine.set_register(A2, 0); // offset
        machine.set_register(A3, 0); //index
        machine.set_register(A4, u64::from(Source::Group(source))); //source
        machine.set_register(A7, LOAD_WITNESS_SYSCALL_NUMBER); // syscall number

        let witness = Bytes::from(data.to_owned()).pack();

        let witness_correct_data = witness.raw_data();

        let dummy_witness = Bytes::default().pack();
        let witnesses = vec![dummy_witness, witness];
        let group_inputs = vec![1];
        let group_outputs = vec![1];
        let mut load_witness = LoadWitness::new(witnesses.pack(), &group_inputs, &group_outputs);

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
        fn test_load_group_witness_by_input(ref data in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_group_witness(data, SourceEntry::Input)?;
        }

        fn test_load_group_witness_by_output(ref data in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_group_witness(data, SourceEntry::Output)?;
        }
    }

    fn _test_load_script(data: &[u8]) -> Result<(), TestCaseError> {
        let mut machine = DefaultCoreMachine::<u64, SparseMemory<u64>>::default();
        let size_addr: u64 = 0;
        let addr: u64 = 100;

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, size_addr); // size_addr
        machine.set_register(A2, 0); // offset
        machine.set_register(A7, LOAD_SCRIPT_SYSCALL_NUMBER); // syscall number

        let script = ScriptBuilder::default()
            .args(Bytes::from(data.to_owned()).pack())
            .build();
        let script_correct_data = script.as_slice();

        let mut load_script = LoadScript::new(script.clone());

        prop_assert!(machine
            .memory_mut()
            .store64(&size_addr, &(script_correct_data.len() as u64 + 20))
            .is_ok());

        prop_assert!(load_script.ecall(&mut machine).is_ok());
        prop_assert_eq!(machine.registers()[A0], u64::from(SUCCESS));

        prop_assert_eq!(
            machine.memory_mut().load64(&size_addr),
            Ok(script_correct_data.len() as u64)
        );

        for (i, addr) in (addr..addr + script_correct_data.len() as u64).enumerate() {
            prop_assert_eq!(
                machine.memory_mut().load8(&addr),
                Ok(u64::from(script_correct_data[i]))
            );
        }
        Ok(())
    }

    proptest! {
        #[test]
        fn test_load_script(ref data in any_with::<Vec<u8>>(size_range(1000).lift())) {
            _test_load_script(data)?;
        }
    }

    fn _test_load_cell_data_as_code(data: &[u8]) -> Result<(), TestCaseError> {
        let mut machine = DefaultCoreMachine::<u64, WXorXMemory<SparseMemory<u64>>>::new(
            ISA_IMC,
            VERSION0,
            u64::max_value(),
        );

        let addr = 4096;
        let addr_size = 4096;

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, addr_size); // size
        machine.set_register(A2, 0); // content offset
        machine.set_register(A3, data.len() as u64); // content size
        machine.set_register(A4, 0); //index
        machine.set_register(A5, u64::from(Source::Transaction(SourceEntry::CellDep))); //source
        machine.set_register(A7, LOAD_CELL_DATA_AS_CODE_SYSCALL_NUMBER); // syscall number

        let dep_cell_data = Bytes::from(data.to_owned());
        let dep_cell = build_cell_meta(10000, dep_cell_data);

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let outputs = vec![];
        let resolved_inputs = vec![];
        let resolved_cell_deps = vec![dep_cell];
        let group_inputs = vec![];
        let group_outputs = vec![];
        let mut load_code = LoadCellData::new(
            &data_loader,
            &outputs,
            &resolved_inputs,
            &resolved_cell_deps,
            &group_inputs,
            &group_outputs,
        );

        prop_assert!(machine.memory_mut().store_byte(addr, addr_size, 1).is_ok());

        prop_assert!(load_code.ecall(&mut machine).is_ok());
        prop_assert_eq!(machine.registers()[A0], u64::from(SUCCESS));

        let flags = FLAG_EXECUTABLE | FLAG_FREEZED | FLAG_DIRTY;
        prop_assert_eq!(
            machine
                .memory_mut()
                .fetch_flag(addr / RISCV_PAGESIZE as u64),
            Ok(flags)
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

    fn _test_load_cell_data(data: &[u8]) -> Result<(), TestCaseError> {
        let mut machine = DefaultCoreMachine::<u64, WXorXMemory<SparseMemory<u64>>>::new(
            ISA_IMC,
            VERSION0,
            u64::max_value(),
        );
        let size_addr: u64 = 100;
        let addr = 4096;
        let addr_size = 4096;

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, size_addr); // size
        machine.set_register(A2, 0); // offset
        machine.set_register(A3, 0); //index
        machine.set_register(A4, u64::from(Source::Transaction(SourceEntry::CellDep))); //source
        machine.set_register(A7, LOAD_CELL_DATA_SYSCALL_NUMBER); // syscall number

        prop_assert!(machine.memory_mut().store64(&size_addr, &addr_size).is_ok());

        let dep_cell_data = Bytes::from(data.to_owned());
        let dep_cell = build_cell_meta(10000, dep_cell_data);

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

        prop_assert!(load_code.ecall(&mut machine).is_ok());
        prop_assert_eq!(machine.registers()[A0], u64::from(SUCCESS));

        let flags = FLAG_WRITABLE | FLAG_DIRTY;
        prop_assert_eq!(
            machine
                .memory_mut()
                .fetch_flag(addr / RISCV_PAGESIZE as u64),
            Ok(flags)
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
            _test_load_cell_data_as_code(data)?;
        }

        #[test]
        fn test_load_data(ref data in any_with::<Vec<u8>>(size_range(4096).lift())) {
            _test_load_cell_data(data)?;
        }
    }

    #[test]
    fn test_load_overflowed_cell_data_as_code() {
        let data = vec![0, 1, 2, 3, 4, 5];
        let mut machine = DefaultCoreMachine::<u64, WXorXMemory<SparseMemory<u64>>>::new(
            ISA_IMC,
            VERSION0,
            u64::max_value(),
        );
        let addr = 4096;
        let addr_size = 4096;

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, addr_size); // size
        machine.set_register(A2, 3); // content offset
        machine.set_register(A3, u64::max_value() - 1); // content size
        machine.set_register(A4, 0); //index
        machine.set_register(A5, u64::from(Source::Transaction(SourceEntry::CellDep))); //source
        machine.set_register(A7, LOAD_CELL_DATA_AS_CODE_SYSCALL_NUMBER); // syscall number

        let dep_cell_data = Bytes::from(data);
        let dep_cell = build_cell_meta(10000, dep_cell_data);

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let outputs = vec![];
        let resolved_inputs = vec![];
        let resolved_cell_deps = vec![dep_cell];
        let group_inputs = vec![];
        let group_outputs = vec![];
        let mut load_code = LoadCellData::new(
            &data_loader,
            &outputs,
            &resolved_inputs,
            &resolved_cell_deps,
            &group_inputs,
            &group_outputs,
        );

        assert!(machine.memory_mut().store_byte(addr, addr_size, 1).is_ok());

        let result = load_code.ecall(&mut machine);
        assert_eq!(result.unwrap_err(), VMError::OutOfBound);
    }

    fn _test_load_cell_data_on_freezed_memory(
        as_code: bool,
        data: &[u8],
    ) -> Result<(), TestCaseError> {
        let mut machine = DefaultCoreMachine::<u64, WXorXMemory<SparseMemory<u64>>>::new(
            ISA_IMC,
            VERSION0,
            u64::max_value(),
        );
        let addr = 8192;
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
        machine.set_register(A5, u64::from(Source::Transaction(SourceEntry::CellDep))); //source
        let syscall = if as_code {
            LOAD_CELL_DATA_AS_CODE_SYSCALL_NUMBER
        } else {
            LOAD_CELL_DATA_SYSCALL_NUMBER
        };
        machine.set_register(A7, syscall); // syscall number

        let dep_cell_data = Bytes::from(data.to_owned());
        let dep_cell = build_cell_meta(10000, dep_cell_data);

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let outputs = vec![];
        let resolved_inputs = vec![];
        let resolved_cell_deps = vec![dep_cell];
        let group_inputs = vec![];
        let group_outputs = vec![];
        let mut load_code = LoadCellData::new(
            &data_loader,
            &outputs,
            &resolved_inputs,
            &resolved_cell_deps,
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
            _test_load_cell_data_on_freezed_memory(true, data)?;
        }

        #[test]
        fn test_load_data_on_freezed_memory(ref data in any_with::<Vec<u8>>(size_range(4096).lift())) {
            _test_load_cell_data_on_freezed_memory(false, data)?;
        }
    }

    #[test]
    fn test_load_code_unaligned_error() {
        let mut machine = DefaultCoreMachine::<u64, WXorXMemory<SparseMemory<u64>>>::new(
            ISA_IMC,
            VERSION0,
            u64::max_value(),
        );
        let addr = 4097;
        let addr_size = 4096;
        let data = [2; 32];

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, addr_size); // size
        machine.set_register(A2, 0); // content offset
        machine.set_register(A3, data.len() as u64); // content size
        machine.set_register(A4, 0); //index
        machine.set_register(A5, u64::from(Source::Transaction(SourceEntry::CellDep))); //source
        machine.set_register(A7, LOAD_CELL_DATA_AS_CODE_SYSCALL_NUMBER); // syscall number
        let dep_cell_data = Bytes::from(data.to_vec());
        let dep_cell = build_cell_meta(10000, dep_cell_data);

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let outputs = vec![];
        let resolved_inputs = vec![];
        let resolved_cell_deps = vec![dep_cell];
        let group_inputs = vec![];
        let group_outputs = vec![];
        let mut load_code = LoadCellData::new(
            &data_loader,
            &outputs,
            &resolved_inputs,
            &resolved_cell_deps,
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
        let mut machine = DefaultCoreMachine::<u64, WXorXMemory<SparseMemory<u64>>>::new(
            ISA_IMC,
            VERSION0,
            u64::max_value(),
        );
        let addr = 4096;
        let addr_size = 4096;
        let data = [2; 32];

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, addr_size); // size
        machine.set_register(A2, 0); // content offset
        machine.set_register(A3, data.len() as u64 + 3); // content size
        machine.set_register(A4, 0); //index
        machine.set_register(A5, u64::from(Source::Transaction(SourceEntry::CellDep))); //source
        machine.set_register(A7, LOAD_CELL_DATA_AS_CODE_SYSCALL_NUMBER); // syscall number

        let dep_cell_data = Bytes::from(data.to_vec());
        let dep_cell = build_cell_meta(10000, dep_cell_data);

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let outputs = vec![];
        let resolved_inputs = vec![];
        let resolved_cell_deps = vec![dep_cell];
        let group_inputs = vec![];
        let group_outputs = vec![];
        let mut load_code = LoadCellData::new(
            &data_loader,
            &outputs,
            &resolved_inputs,
            &resolved_cell_deps,
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
        let mut machine = DefaultCoreMachine::<u64, WXorXMemory<SparseMemory<u64>>>::new(
            ISA_IMC,
            VERSION0,
            u64::max_value(),
        );
        let addr = 4096;
        let addr_size = 4096;

        let mut data = vec![];
        data.resize(8000, 2);

        machine.set_register(A0, addr); // addr
        machine.set_register(A1, addr_size); // size
        machine.set_register(A2, 0); // content offset
        machine.set_register(A3, data.len() as u64); // content size
        machine.set_register(A4, 0); //index
        machine.set_register(A5, u64::from(Source::Transaction(SourceEntry::CellDep))); //source
        machine.set_register(A7, LOAD_CELL_DATA_AS_CODE_SYSCALL_NUMBER); // syscall number

        let dep_cell_data = Bytes::from(data);
        let dep_cell = build_cell_meta(10000, dep_cell_data);

        let store = new_store();
        let data_loader = DataLoaderWrapper::new(&store);
        let outputs = vec![];
        let resolved_inputs = vec![];
        let resolved_cell_deps = vec![dep_cell];
        let group_inputs = vec![];
        let group_outputs = vec![];
        let mut load_code = LoadCellData::new(
            &data_loader,
            &outputs,
            &resolved_inputs,
            &resolved_cell_deps,
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
    fn test_vm_version0() {
        let mut machine = DefaultCoreMachine::<u64, WXorXMemory<SparseMemory<u64>>>::new(
            ISA_IMC,
            VERSION0,
            u64::max_value(),
        );

        machine.set_register(A0, 0);
        machine.set_register(A1, 0);
        machine.set_register(A2, 0);
        machine.set_register(A3, 0);
        machine.set_register(A4, 0);
        machine.set_register(A5, 0);
        machine.set_register(A7, VM_VERSION);

        let result = VMVersion::new().ecall(&mut machine);

        assert_eq!(result.unwrap(), true);
        assert_eq!(machine.registers()[A0], 0);
    }

    #[test]
    fn test_vm_version1() {
        let mut machine = DefaultCoreMachine::<u64, WXorXMemory<SparseMemory<u64>>>::new(
            ISA_IMC,
            VERSION1,
            u64::max_value(),
        );

        machine.set_register(A0, 0);
        machine.set_register(A1, 0);
        machine.set_register(A2, 0);
        machine.set_register(A3, 0);
        machine.set_register(A4, 0);
        machine.set_register(A5, 0);
        machine.set_register(A7, VM_VERSION);

        let result = VMVersion::new().ecall(&mut machine);

        assert_eq!(result.unwrap(), true);
        assert_eq!(machine.registers()[A0], 1);
    }

    #[test]
    fn test_current_cycles() {
        let mut machine = DefaultCoreMachine::<u64, WXorXMemory<SparseMemory<u64>>>::new(
            ISA_IMC,
            VERSION1,
            u64::max_value(),
        );

        machine.set_register(A0, 0);
        machine.set_register(A1, 0);
        machine.set_register(A2, 0);
        machine.set_register(A3, 0);
        machine.set_register(A4, 0);
        machine.set_register(A5, 0);
        machine.set_register(A7, CURRENT_CYCLES);

        machine.set_cycles(100);

        let result = CurrentCycles::new().ecall(&mut machine);

        assert_eq!(result.unwrap(), true);
        assert_eq!(machine.registers()[A0], 100);
    }
}
