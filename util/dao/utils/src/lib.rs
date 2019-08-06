use byteorder::{ByteOrder, LittleEndian};
use ckb_core::transaction::Transaction;
use ckb_core::{Bytes, Capacity};
use ckb_error::{DaoError, Error};

// This is multiplied by 10**16 to make sure we have enough precision.
pub const DEFAULT_ACCUMULATED_RATE: u64 = 10_000_000_000_000_000;

pub const DAO_VERSION: u8 = 1;

pub fn genesis_dao_data(genesis_cellbase_tx: &Transaction) -> Result<Bytes, Error> {
    let c = genesis_cellbase_tx
        .outputs()
        .iter()
        .try_fold(Capacity::zero(), |capacity, output| {
            capacity.safe_add(output.capacity)
        })?;
    let u = genesis_cellbase_tx.outputs_with_data_iter().try_fold(
        Capacity::zero(),
        |capacity, (output, data)| {
            Capacity::bytes(data.len()).and_then(|data_capacity| {
                output
                    .occupied_capacity(data_capacity)
                    .and_then(|c| capacity.safe_add(c))
            })
        },
    )?;
    Ok(pack_dao_data(DEFAULT_ACCUMULATED_RATE, c, u))
}

pub fn extract_dao_data(data: &[u8]) -> Result<(u64, Capacity, Capacity), Error> {
    if data.len() != 32 || data[0] != DAO_VERSION {
        return Err(DaoError::InvalidDaoFormat.into());
    }

    let ar = LittleEndian::read_u64(&data[8..16]);
    let c = Capacity::shannons(LittleEndian::read_u64(&data[16..24]));
    let u = Capacity::shannons(LittleEndian::read_u64(&data[24..32]));
    Ok((ar, c, u))
}

pub fn pack_dao_data(ar: u64, c: Capacity, u: Capacity) -> Bytes {
    let mut buf = [0u8; 32];
    buf[0] = DAO_VERSION;
    LittleEndian::write_u64(&mut buf[8..16], ar);
    LittleEndian::write_u64(&mut buf[16..24], c.as_u64());
    LittleEndian::write_u64(&mut buf[24..32], u.as_u64());
    (&buf[..]).into()
}
