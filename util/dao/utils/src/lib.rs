//! This crate provides several util functions to operate the dao field and NervosDAO related errors.

mod error;

use byteorder::{ByteOrder, LittleEndian};
use ckb_types::{
    H160,
    core::{Capacity, Ratio, TransactionView, capacity_bytes},
    packed::{Byte32, OutPoint},
    prelude::*,
};
use std::collections::HashSet;

pub use crate::error::DaoError;

// This is multiplied by 10**16 to make sure we have enough precision.
const DEFAULT_GENESIS_ACCUMULATE_RATE: u64 = 10_000_000_000_000_000;

/// Calculate the dao field for the genesis block.
///
/// NOTE: Used for testing only!
#[doc(hidden)]
pub fn genesis_dao_data(txs: Vec<&TransactionView>) -> Result<Byte32, DaoError> {
    genesis_dao_data_with_satoshi_gift(
        txs,
        &H160([0u8; 20]),
        Ratio::new(1, 1),
        capacity_bytes!(1_000_000),
        capacity_bytes!(1000),
    )
}

/// Calculate the dao field for the genesis block.
#[doc(hidden)]
pub fn genesis_dao_data_with_satoshi_gift(
    txs: Vec<&TransactionView>,
    satoshi_pubkey_hash: &H160,
    satoshi_cell_occupied_ratio: Ratio,
    initial_primary_issuance: Capacity,
    initial_secondary_issuance: Capacity,
) -> Result<Byte32, DaoError> {
    let dead_cells = txs
        .iter()
        .flat_map(|tx| tx.inputs().into_iter().map(|input| input.previous_output()))
        .collect::<HashSet<_>>();
    let statistics_outputs = |tx_index, tx: &TransactionView| -> Result<_, DaoError> {
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
                // detect satoshi gift cell
                let occupied_capacity = if tx_index == 0
                    && output.lock().args().raw_data() == satoshi_pubkey_hash.0[..]
                {
                    Unpack::<Capacity>::unpack(&output.capacity())
                        .safe_mul_ratio(satoshi_cell_occupied_ratio)
                } else {
                    Capacity::bytes(data.len()).and_then(|c| output.occupied_capacity(c))
                };
                occupied_capacity.and_then(|c| capacity.safe_add(c))
            })?;
        Ok((c, u))
    };

    let initial_issuance = initial_primary_issuance.safe_add(initial_secondary_issuance)?;
    let result: Result<_, DaoError> = txs.into_iter().enumerate().try_fold(
        (initial_issuance, Capacity::zero()),
        |(c, u), (tx_index, tx)| {
            let (tx_c, tx_u) = statistics_outputs(tx_index, tx)?;
            let c = c.safe_add(tx_c)?;
            let u = u.safe_add(tx_u)?;
            Ok((c, u))
        },
    );
    let (c, u) = result?;
    // C cannot be zero, otherwise DAO stats calculation might result in
    // division by zero errors.
    if c == Capacity::zero() {
        return Err(DaoError::ZeroC);
    }
    Ok(pack_dao_data(
        DEFAULT_GENESIS_ACCUMULATE_RATE,
        c,
        initial_secondary_issuance,
        u,
    ))
}

/// Extract `ar`, `c`, `s`, and `u` from [`Byte32`].
///
/// [`Byte32`]: ../ckb_types/packed/struct.Byte32.html
pub fn extract_dao_data(dao: Byte32) -> (u64, Capacity, Capacity, Capacity) {
    let data = dao.raw_data();
    let c = Capacity::shannons(LittleEndian::read_u64(&data[0..8]));
    let ar = LittleEndian::read_u64(&data[8..16]);
    let s = Capacity::shannons(LittleEndian::read_u64(&data[16..24]));
    let u = Capacity::shannons(LittleEndian::read_u64(&data[24..32]));
    (ar, c, s, u)
}

/// Pack `ar`, `c`, `s`, and `u` into [`Byte32`] in little endian.
///
/// [`Byte32`]: ../ckb_types/packed/struct.Byte32.html
pub fn pack_dao_data(ar: u64, c: Capacity, s: Capacity, u: Capacity) -> Byte32 {
    let mut buf = [0u8; 32];
    LittleEndian::write_u64(&mut buf[0..8], c.as_u64());
    LittleEndian::write_u64(&mut buf[8..16], ar);
    LittleEndian::write_u64(&mut buf[16..24], s.as_u64());
    LittleEndian::write_u64(&mut buf[24..32], u.as_u64());
    Byte32::from_slice(&buf).expect("impossible: fail to read array")
}

#[cfg(test)]
mod tests {
    pub use super::{extract_dao_data, pack_dao_data};
    pub use ckb_types::core::Capacity;
    pub use ckb_types::h256;
    pub use ckb_types::packed::Byte32;
    pub use ckb_types::prelude::Pack;

    #[test]
    #[allow(clippy::unreadable_literal)]
    fn test_dao_data() {
        let cases = vec![
            (
                // mainnet block[0]
                h256!("0x8874337e541ea12e0000c16ff286230029bfa3320800000000710b00c0fefe06"),
                10000000000000000,
                Capacity::shannons(3360000145238488200),
                Capacity::shannons(35209330473),
                Capacity::shannons(504120308900000000),
            ),
            (
                // mainnet block[1]
                h256!("0x10e9164f761ea12ea5f6ff75f28623007b7f682a0f00000000710b00c0fefe06"),
                10000000104789669,
                Capacity::shannons(3360000290476976400),
                Capacity::shannons(65136000891),
                Capacity::shannons(504120308900000000),
            ),
            (
                // mainnet block[5892]
                h256!("0x95b47fdcff26a42ed0fb76e081872300bb585ebd10a000000043c2f76b5eff06"),
                10000616071298000,
                Capacity::shannons(3360854102283105429),
                Capacity::shannons(175993756997819),
                Capacity::shannons(504225501100000000),
            ),
        ];
        for (dao_h256, ar, c, s, u) in cases {
            let dao_byte32: Byte32 = dao_h256.pack();
            assert_eq!((ar, c, s, u), extract_dao_data(dao_byte32.clone()));
            assert_eq!(dao_byte32, pack_dao_data(ar, c, s, u,));
        }
    }
}
