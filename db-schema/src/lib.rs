#![allow(non_snake_case)]
//! The schema include constants define the low level database column families.

use ckb_types::core::BlockNumber;
use ckb_types::packed::Byte32;
use ckb_types::packed::NumberHash;
use ckb_types::packed::OutPoint;
use ckb_types::packed::TransactionKey;
use ckb_types::prelude::*;
use ckb_types::BlockNumberAndHash;

/// Column families alias type
pub type Col = &'static str;

/// Total column number
pub const COLUMNS: u32 = 20;

pub mod COLUMN_INDEX {
    use super::*;

    pub const NAME: Col = "0";

    pub fn key_number(number: BlockNumber) -> impl AsRef<[u8]> {
        number.to_be_bytes()
    }

    pub fn key_hash(hash: Byte32) -> impl AsRef<[u8]> {
        hash.as_slice().to_vec()
    }
}

pub mod COLUMN_BLOCK_HEADER {
    use super::*;

    pub const NAME: Col = "1";

    pub fn key(num_hash: BlockNumberAndHash) -> Vec<u8> {
        let mut key = Vec::with_capacity(40);
        key.extend(num_hash.number().to_be_bytes());
        key.extend(num_hash.hash().as_slice());
        key
    }
}

pub mod COLUMN_BLOCK_BODY {
    use super::*;

    pub const NAME: Col = "2";

    pub fn key(num_hash: BlockNumberAndHash, tx_index: usize) -> Vec<u8> {
        TransactionKey::new_builder()
            .block_number(num_hash.number().pack())
            .block_hash(num_hash.hash())
            .index(tx_index.pack())
            .build()
            .as_slice()
            .to_vec()
    }

    pub fn prefix_key(num_hash: BlockNumberAndHash) -> Vec<u8> {
        TransactionKey::new_builder()
            .block_number(num_hash.number.pack())
            .block_hash(num_hash.hash)
            .build()
            .as_slice()[..40]
            .to_vec()
    }
}

fn num_hash_key(num_hash: BlockNumberAndHash) -> impl AsRef<[u8]> {
    let mut key = Vec::with_capacity(40);
    key.extend(num_hash.number.to_be_bytes());
    key.extend(num_hash.hash.as_slice());
    key
}

pub mod COLUMN_BLOCK_UNCLE {
    use super::*;

    /// Column store block's uncle and uncles’ proposal zones
    pub const NAME: Col = "3";

    pub fn key(num_hash: BlockNumberAndHash) -> impl AsRef<[u8]> {
        num_hash_key(num_hash)
    }
}

pub mod COLUMN_META {
    use super::*;

    /// Column store meta data
    pub const NAME: Col = "4";

    /// META_TIP_HEADER_KEY tracks the latest known best block header
    pub const META_TIP_HEADER_KEY: &[u8] = b"TIP_HEADER";
    /// META_CURRENT_EPOCH_KEY tracks the latest known epoch
    pub const META_CURRENT_EPOCH_KEY: &[u8] = b"CURRENT_EPOCH";
    /// META_FILTER_DATA_KEY tracks the latest built filter data block hash
    pub const META_LATEST_BUILT_FILTER_DATA_KEY: &[u8] = b"LATEST_BUILT_FILTER_DATA";

    /// CHAIN_SPEC_HASH_KEY tracks the hash of chain spec which created current database
    pub const CHAIN_SPEC_HASH_KEY: &[u8] = b"chain-spec-hash";
    /// MIGRATION_VERSION_KEY tracks the current database version.
    pub const MIGRATION_VERSION_KEY: &[u8] = b"db-version";
}

pub mod COLUMN_TRANSACTION_INFO {
    use super::*;

    /// Column store transaction extra information
    pub const NAME: Col = "5";
}

pub mod COLUMN_BLOCK_EXT {
    use super::*;

    /// Column store block extra information
    pub const NAME: Col = "6";

    pub fn key(num_hash: BlockNumberAndHash) -> impl AsRef<[u8]> {
        num_hash_key(num_hash)
    }
}

pub mod COLUMN_BLOCK_PROPOSAL_IDS {
    use super::*;

    /// Column store block's proposal ids
    pub const NAME: Col = "7";

    pub fn key(num_hash: BlockNumberAndHash) -> impl AsRef<[u8]> {
        num_hash_key(num_hash)
    }
}

pub mod COLUMN_BLOCK_EPOCH {
    use super::*;

    /// Column store indicates track block epoch
    pub const NAME: Col = "8";
}

pub mod COLUMN_EPOCH {
    use super::*;

    /// Column store indicates track block epoch
    pub const NAME: Col = "9";
}

pub mod COLUMN_CELL {
    use super::*;

    /// Column store cell
    pub const NAME: Col = "10";

    pub fn key(block_number: BlockNumber, cell: &OutPoint) -> impl AsRef<[u8]> {
        cell.to_cell_key(block_number)
    }
}

pub mod COLUMN_UNCLES {
    use super::*;

    /// Column store main chain consensus include uncles
    /// <https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0020-ckb-consensus-protocol/0020-ckb-consensus-protocol.md#specification>
    pub const NAME: Col = "11";
}

pub mod COLUMN_CELL_DATA {
    use super::*;

    /// Column store cell data
    pub const NAME: Col = "12";

    pub fn key(block_number: BlockNumber, cell: &OutPoint) -> impl AsRef<[u8]> {
        cell.to_cell_key(block_number)
    }
}

pub mod COLUMN_NUMBER_HASH {
    use super::*;

    /// Column store block number-hash pair
    pub const NAME: Col = "13";

    pub fn key(num_hash: BlockNumberAndHash) -> impl AsRef<[u8]> {
        num_hash_key(num_hash)
    }
}

pub mod COLUMN_CELL_DATA_HASH {
    use super::*;
    use ckb_types::packed::OutPoint;

    /// Column store cell data hash
    pub const NAME: Col = "14";

    pub fn key(block_number: BlockNumber, cell: &OutPoint) -> impl AsRef<[u8]> {
        cell.to_cell_key(block_number)
    }
}

pub mod COLUMN_BLOCK_EXTENSION {
    use super::*;

    /// Column store block extension data
    pub const NAME: Col = "15";
}

pub mod COLUMN_CHAIN_ROOT_MMR {
    use super::*;
    use ckb_types::packed;

    /// Column store chain root MMR data
    pub const NAME: Col = "16";

    /// Build COLUMN_CHAIN_ROOT_MMR's key
    pub fn key(pos: u64) -> packed::BeUint64 {
        let key: packed::BeUint64 = pos.pack();
        key
    }
}

pub mod COLUMN_BLOCK_FILTER {
    use super::*;

    /// Column store filter data for client-side filtering
    pub const NAME: Col = "17";

    /// Build COLUMN_BLOCK_FILTER's key
    pub fn key(num_hash: BlockNumberAndHash) -> impl AsRef<[u8]> {
        num_hash_key(num_hash)
    }
}

pub mod COLUMN_BLOCK_FILTER_HASH {
    use super::*;

    /// Column store filter data hash for client-side filtering
    pub const NAME: Col = "18";

    /// Build COLUMN_BLOCK_FILTER_HASH's key
    pub fn key(num_hash: BlockNumberAndHash) -> impl AsRef<[u8]> {
        num_hash_key(num_hash)
    }
}

pub mod COLUMN_BLOCK_HEADER_NUM {
    use super::*;

    pub const NAME: Col = "19";

    pub fn key(hash: Byte32) -> Vec<u8> {
        hash.as_slice().to_vec()
    }
}
