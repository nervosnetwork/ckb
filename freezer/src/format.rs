use crate::storage::invalid_data;
use sha2::{Digest, Sha256};
use std::io;

const MAGIC: &[u8; 8] = b"CKBFZ006";
pub(crate) const INDEX_ENTRY_SIZE: u64 = 96;
pub(crate) const COMMIT_SIZE: usize = 96;

pub(crate) fn checksum(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Commit {
    pub count: u64,
    pub file_id: u32,
    pub offset: u64,
    pub index_hash: [u8; 32],
}

impl Commit {
    pub fn encode(&self) -> Vec<u8> {
        let mut raw = Vec::with_capacity(COMMIT_SIZE);
        raw.extend_from_slice(MAGIC);
        raw.extend_from_slice(&self.count.to_le_bytes());
        raw.extend_from_slice(&self.file_id.to_le_bytes());
        raw.extend_from_slice(&0u32.to_le_bytes());
        raw.extend_from_slice(&self.offset.to_le_bytes());
        raw.extend_from_slice(&self.index_hash);
        raw.extend_from_slice(&checksum(&raw));
        raw
    }

    pub fn decode(raw: &[u8]) -> io::Result<Self> {
        let mut body = checked_body(raw, COMMIT_SIZE)?;
        if &take::<8>(&mut body)? != MAGIC {
            return Err(invalid_data("invalid archive format"));
        }
        let count = u64::from_le_bytes(take(&mut body)?);
        let file_id = u32::from_le_bytes(take(&mut body)?);
        if u32::from_le_bytes(take(&mut body)?) != 0 {
            return Err(invalid_data("invalid archive commit flags"));
        }
        let offset = u64::from_le_bytes(take(&mut body)?);
        let index_hash = take(&mut body)?;
        count
            .checked_add(1)
            .ok_or_else(|| invalid_data("invalid archive count"))?;
        Ok(Self {
            count,
            file_id,
            offset,
            index_hash,
        })
    }
}

pub(crate) struct IndexEntry {
    pub number: u64,
    pub file_id: u32,
    pub stored_len: u32,
    pub offset: u64,
    pub raw_len: u64,
    pub data_hash: [u8; 32],
}

impl IndexEntry {
    pub fn encode(&self) -> Vec<u8> {
        let mut raw = Vec::with_capacity(INDEX_ENTRY_SIZE as usize);
        raw.extend_from_slice(&self.number.to_le_bytes());
        raw.extend_from_slice(&self.file_id.to_le_bytes());
        raw.extend_from_slice(&self.stored_len.to_le_bytes());
        raw.extend_from_slice(&self.offset.to_le_bytes());
        raw.extend_from_slice(&self.raw_len.to_le_bytes());
        raw.extend_from_slice(&self.data_hash);
        raw.extend_from_slice(&checksum(&raw));
        raw
    }

    pub fn decode(raw: &[u8]) -> io::Result<Self> {
        let mut body = checked_body(raw, INDEX_ENTRY_SIZE as usize)?;
        Ok(Self {
            number: u64::from_le_bytes(take(&mut body)?),
            file_id: u32::from_le_bytes(take(&mut body)?),
            stored_len: u32::from_le_bytes(take(&mut body)?),
            offset: u64::from_le_bytes(take(&mut body)?),
            raw_len: u64::from_le_bytes(take(&mut body)?),
            data_hash: take(&mut body)?,
        })
    }
}

fn checked_body(raw: &[u8], size: usize) -> io::Result<&[u8]> {
    if raw.len() != size || checksum(&raw[..size - 32]) != raw[size - 32..] {
        return Err(invalid_data("archive metadata checksum mismatch"));
    }
    Ok(&raw[..size - 32])
}

fn take<const N: usize>(raw: &mut &[u8]) -> io::Result<[u8; N]> {
    let (head, tail) = raw
        .split_at_checked(N)
        .ok_or_else(|| invalid_data("short archive metadata"))?;
    *raw = tail;
    head.try_into().map_err(invalid_data)
}
