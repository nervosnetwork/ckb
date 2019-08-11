#[macro_use]
extern crate enum_display_derive;

mod error;

use byteorder::{ByteOrder, LittleEndian};
use ckb_types::{
    core::{Capacity, TransactionView},
    packed::{Byte32, OutPoint},
    prelude::*,
};
use failure::{Error as FailureError, Fail};
use std::collections::HashSet;
use ckb_error::Error;

pub use crate::error::DaoError;

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

pub fn genesis_dao_data(txs: Vec<&TransactionView>) -> Result<Byte32, Error> {
    let dead_cells = txs
        .iter()
        .flat_map(|tx| tx.inputs().into_iter().map(|input| input.previous_output()))
        .collect::<HashSet<_>>();
    let statistics_outputs = |tx: &TransactionView| -> Result<_, FailureError> {
        let c = tx
            .data()
            .raw()
            .outputs()
            .into_iter()
            .enumerate()
            .filter(|(index, _)| !dead_cells.contains(&OutPoint::new(tx.hash(), *index as u32)))
            .try_fold(Capacity::zero(), |capacity, (_, output)| {
                let cap: Capacity = output.capacity().unpack();
                capacity.safe_add(cap)
            })?;
        let u = tx
            .outputs_with_data_iter()
            .enumerate()
            .filter(|(index, _)| !dead_cells.contains(&OutPoint::new(tx.hash(), *index as u32)))
            .try_fold(Capacity::zero(), |capacity, (_, (output, data))| {
                Capacity::bytes(data.len()).and_then(|data_capacity| {
                    output
                        .occupied_capacity(data_capacity)
                        .and_then(|c| capacity.safe_add(c))
                })
            })?;
        Ok((c, u))
    };

    let result: Result<_, FailureError> =
        txs.into_iter()
            .try_fold((Capacity::zero(), Capacity::zero()), |(c, u), tx| {
                let (tx_c, tx_u) = statistics_outputs(tx)?;
                let c = c.safe_add(tx_c)?;
                let u = u.safe_add(tx_u)?;
                Ok((c, u))
            });
    let (c, u) = result?;
    Ok(pack_dao_data(DEFAULT_ACCUMULATED_RATE, c, u))
}

pub fn extract_dao_data(dao: Byte32) -> Result<(u64, Capacity, Capacity), Error> {
    let data = dao.raw_data();
    if data[0] != DAO_VERSION {
        return Err(DaoError::InvalidDaoFormat.into());
    }
    let ar = LittleEndian::read_u64(&data[8..16]);
    let c = Capacity::shannons(LittleEndian::read_u64(&data[16..24]));
    let u = Capacity::shannons(LittleEndian::read_u64(&data[24..32]));
    Ok((ar, c, u))
}

pub fn pack_dao_data(ar: u64, c: Capacity, u: Capacity) -> Byte32 {
    let mut buf = [0u8; DAO_SIZE];
    buf[0] = DAO_VERSION;
    LittleEndian::write_u64(&mut buf[8..16], ar);
    LittleEndian::write_u64(&mut buf[16..24], c.as_u64());
    LittleEndian::write_u64(&mut buf[24..32], u.as_u64());
    Byte32::from_slice(&buf).expect("impossible: fail to read array")
}
