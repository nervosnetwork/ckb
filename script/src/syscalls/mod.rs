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

#[cfg(test)]
mod pause;

#[cfg(test)]
mod tests;

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

#[cfg(test)]
pub use self::pause::Pause;

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
#[cfg(test)]
pub const DEBUG_PAUSE: u64 = 2178;

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
            _ => Err(Error::External(format!("CellField parse_from_u64 {i}"))),
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
            _ => Err(Error::External(format!("HeaderField parse_from_u64 {i}"))),
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
            _ => Err(Error::External(format!("InputField parse_from_u64 {i}"))),
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
            _ => Err(Error::External(format!("SourceEntry parse_from_u64 {i}"))),
        }
    }
}

pub(crate) const SOURCE_GROUP_FLAG: u64 = 0x0100_0000_0000_0000;
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

#[derive(Debug, PartialEq, Clone, Copy, Eq)]
enum Place {
    CellData,
    Witness,
}

impl Place {
    fn parse_from_u64(i: u64) -> Result<Place, Error> {
        match i {
            0 => Ok(Place::CellData),
            1 => Ok(Place::Witness),
            _ => Err(Error::External(format!("Place parse_from_u64 {i}"))),
        }
    }
}
