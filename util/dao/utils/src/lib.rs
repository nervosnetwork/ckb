use byteorder::{ByteOrder, LittleEndian};
use ckb_types::{
    bytes::Bytes,
    core::{Capacity, TransactionView},
    prelude::*,
};
use failure::{Error as FailureError, Fail};

// This is multiplied by 10**16 to make sure we have enough precision.
pub const DEFAULT_ACCUMULATED_RATE: u64 = 10_000_000_000_000_000;

pub const DAO_VERSION: u8 = 1;

pub const DAO_SIZE: usize = 32;

#[derive(Debug, PartialEq, Clone, Eq, Fail)]
pub enum Error {
    #[fail(display = "Unknown")]
    Unknown,
    #[fail(display = "InvalidHeader")]
    InvalidHeader,
    #[fail(display = "InvalidOutPoint")]
    InvalidOutPoint,
    #[fail(display = "Format")]
    Format,
}

pub fn genesis_dao_data(genesis_cellbase_tx: &TransactionView) -> Result<Bytes, FailureError> {
    let c = genesis_cellbase_tx
        .data()
        .slim()
        .raw()
        .outputs()
        .into_iter()
        .try_fold(Capacity::zero(), |capacity, output| {
            let cap: Capacity = output.capacity().unpack();
            capacity.safe_add(cap)
        })?;
    let u = genesis_cellbase_tx
        .data()
        .slim()
        .raw()
        .outputs()
        .into_iter()
        .zip(
            genesis_cellbase_tx
                .data()
                .outputs_data()
                .into_iter()
                .map(|d| d.raw_data()),
        )
        .try_fold(Capacity::zero(), |capacity, (output, data)| {
            Capacity::bytes(data.len()).and_then(|data_capacity| {
                output
                    .occupied_capacity(data_capacity)
                    .and_then(|c| capacity.safe_add(c))
            })
        })?;
    Ok(pack_dao_data(DEFAULT_ACCUMULATED_RATE, c, u))
}

pub fn extract_dao_data(data: &[u8]) -> Result<(u64, Capacity, Capacity), FailureError> {
    if data.len() != DAO_SIZE || data[0] != DAO_VERSION {
        return Err(Error::Format.into());
    }

    let ar = LittleEndian::read_u64(&data[8..16]);
    let c = Capacity::shannons(LittleEndian::read_u64(&data[16..24]));
    let u = Capacity::shannons(LittleEndian::read_u64(&data[24..32]));
    Ok((ar, c, u))
}

pub fn pack_dao_data(ar: u64, c: Capacity, u: Capacity) -> Bytes {
    let mut buf = [0u8; DAO_SIZE];
    buf[0] = DAO_VERSION;
    LittleEndian::write_u64(&mut buf[8..16], ar);
    LittleEndian::write_u64(&mut buf[16..24], c.as_u64());
    LittleEndian::write_u64(&mut buf[24..32], u.as_u64());
    (&buf[..]).into()
}
