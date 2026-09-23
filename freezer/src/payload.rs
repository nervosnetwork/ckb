use crate::format::checksum;
use crate::storage::invalid_data;
use std::{io, ops::Range};

pub(crate) const CHUNK_SIZE: usize = 8192;
pub(crate) const CHUNK_THRESHOLD: usize = 32 * 1024;
const CHUNK_ENTRY_SIZE: usize = 36;

pub(crate) fn buffer(length: usize) -> io::Result<Vec<u8>> {
    let mut data = Vec::new();
    data.try_reserve_exact(length)
        .map_err(|e| io::Error::new(io::ErrorKind::OutOfMemory, e))?;
    data.resize(length, 0);
    Ok(data)
}

pub(crate) fn table_len(raw_len: usize) -> io::Result<usize> {
    if raw_len <= CHUNK_THRESHOLD {
        return Ok(0);
    }
    raw_len
        .div_ceil(CHUNK_SIZE)
        .checked_mul(CHUNK_ENTRY_SIZE)
        .ok_or_else(|| invalid_data("archive chunk table size overflow"))
}

/// Include the chunk table and each independent compressor's output bound.
pub(crate) fn encoded_size_bound(raw_len: usize) -> io::Result<usize> {
    let bound = if raw_len <= CHUNK_THRESHOLD {
        20 + raw_len as u128 * 110 / 100
    } else {
        let full = raw_len / CHUNK_SIZE;
        let tail = raw_len % CHUNK_SIZE;
        table_len(raw_len)? as u128
            + full as u128 * lz4_flex::block::get_maximum_output_size(CHUNK_SIZE) as u128
            + if tail == 0 {
                0
            } else {
                lz4_flex::block::get_maximum_output_size(tail) as u128
            }
    };
    usize::try_from(bound).map_err(invalid_data)
}

/// Small records are one LZ4 block. Large records start with a checksummed
/// table of compressed lengths and hashes, followed by independent LZ4 chunks.
pub(crate) fn encode(raw: &[u8]) -> io::Result<(Vec<u8>, [u8; 32])> {
    let table_len = table_len(raw.len())?;
    let mut data = buffer(encoded_size_bound(raw.len())?)?;
    let hash = if table_len == 0 {
        let size = lz4_flex::block::compress_into(raw, &mut data).map_err(invalid_data)?;
        data.truncate(size);
        checksum(&data)
    } else {
        let mut offset = table_len;
        for (index, chunk) in raw.chunks(CHUNK_SIZE).enumerate() {
            let size =
                lz4_flex::block::compress_into(chunk, &mut data[offset..]).map_err(invalid_data)?;
            let hash = checksum(&data[offset..offset + size]);
            let entry = &mut data[index * CHUNK_ENTRY_SIZE..(index + 1) * CHUNK_ENTRY_SIZE];
            entry[..4].copy_from_slice(&(size as u32).to_le_bytes());
            entry[4..].copy_from_slice(&hash);
            offset += size;
        }
        data.truncate(offset);
        checksum(&data[..table_len])
    };
    Ok((data, hash))
}

pub(crate) fn decode_into(data: &[u8], hash: &[u8], output: &mut [u8]) -> io::Result<()> {
    if checksum(data) != hash {
        return Err(invalid_data("archive payload checksum mismatch"));
    }
    let length = lz4_flex::block::decompress_into(data, output).map_err(invalid_data)?;
    if length != output.len() {
        return Err(invalid_data("archive payload decoded length mismatch"));
    }
    Ok(())
}

/// A validated view borrows its encoded table; full reads need no second table
/// allocation, and positional reads retain only the table and first chunk.
pub(crate) struct ChunkTable<'a> {
    table: &'a [u8],
}

impl<'a> ChunkTable<'a> {
    pub fn parse(
        data: &'a [u8],
        raw_len: usize,
        stored_len: usize,
        hash: &[u8; 32],
    ) -> io::Result<Self> {
        let length = table_len(raw_len)?;
        if length == 0 || length >= stored_len {
            return Err(invalid_data("invalid archive chunk table length"));
        }
        let table = data
            .get(..length)
            .ok_or_else(|| invalid_data("truncated archive chunk table"))?;
        if checksum(table) != *hash {
            return Err(invalid_data("archive chunk table checksum mismatch"));
        }
        let mut end = length;
        for (index, entry) in table.chunks_exact(CHUNK_ENTRY_SIZE).enumerate() {
            let size = chunk_length(entry);
            let raw_size = (raw_len - index * CHUNK_SIZE).min(CHUNK_SIZE);
            if size == 0 || size > lz4_flex::block::get_maximum_output_size(raw_size) {
                return Err(invalid_data("invalid archive compressed chunk length"));
            }
            end = end
                .checked_add(size)
                .ok_or_else(|| invalid_data("archive chunk offset overflow"))?;
        }
        if end != stored_len {
            return Err(invalid_data(
                "archive chunk table does not cover the record",
            ));
        }
        Ok(Self { table })
    }

    /// The table has already been checked against the immutable index entry.
    pub fn validated(table: &'a [u8]) -> Self {
        Self { table }
    }

    pub fn chunks(&self) -> impl Iterator<Item = (Range<usize>, &'a [u8])> + '_ {
        self.table
            .chunks_exact(CHUNK_ENTRY_SIZE)
            .scan(self.table.len(), |offset, entry| {
                let start = *offset;
                *offset += chunk_length(entry);
                Some((start..*offset, &entry[4..]))
            })
    }
}

fn chunk_length(entry: &[u8]) -> usize {
    u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]) as usize
}
