use crate::{
    bytes::{BufMut, Bytes, BytesMut},
    core::Capacity,
};
use std::mem::size_of;

const MAGIC_FLAGS: [u8; 8] = *b"occupied";

/// Extract virtual occupied capacity from data
pub fn extract_occupied_capacity(data: &Bytes) -> Option<Capacity> {
    let prefix_len = MAGIC_FLAGS.len();
    if data.len() != prefix_len + size_of::<u64>() {
        return None;
    }
    if data.slice_to(prefix_len) != MAGIC_FLAGS[..] {
        return None;
    }
    let mut occupied_bytes = [0u8; 8];
    occupied_bytes.copy_from_slice(&data.slice(prefix_len, prefix_len + size_of::<u64>()));
    let occupied = u64::from_le_bytes(occupied_bytes);
    Some(Capacity::shannons(occupied))
}

/// Generate data of virtual occupied capacity.
pub fn gen_occupied_data(occupied: Capacity) -> Bytes {
    let bin_occupied = occupied.as_u64().to_le_bytes();
    let bytes_len = MAGIC_FLAGS.len() + bin_occupied.len();
    let mut buf = BytesMut::with_capacity(bytes_len);
    buf.put_slice(&MAGIC_FLAGS);
    buf.put_slice(&bin_occupied);
    buf.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gen_data_then_extract() {
        let occupied = Capacity::bytes(42).unwrap();
        let data = gen_occupied_data(occupied);
        assert!(data.len() == 16);
        assert_eq!(extract_occupied_capacity(&data), Some(occupied));
    }
}
