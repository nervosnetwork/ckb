use crate::internal_error;
use crate::reader::{ArchiveRecord, RecordRanges};
use crate::storage::invalid_data;
use ckb_error::Error;
use ckb_types::{packed, prelude::*};
use std::{borrow::Cow, io, ops::Range};

/// A block's verified header and layout, backed by immutable archive ranges.
/// Each requested field is checksummed and validated before it is returned.
/// Full-record verification also checks chunks outside the requested fields.
pub struct ArchivedBlock<'a> {
    data: RecordRanges<'a>,
    uncles: Range<usize>,
    transactions: Range<usize>,
    proposals: Range<usize>,
    extension: Option<Range<usize>>,
}

impl<'a> ArchivedBlock<'a> {
    pub(crate) fn new(record: ArchiveRecord<'a>, hash: &packed::Byte32) -> io::Result<Self> {
        if u32::try_from(record.raw_len()).is_err() {
            return Err(invalid_data("archive block exceeds Molecule size limit"));
        }
        let data = record.ranges()?;
        let prefix = data.read(0..8)?;
        if !matches!(number(&prefix[4..]), 20 | 24) {
            return Err(invalid_data("unsupported archived block fields"));
        }
        let table = offsets(&data, 0..data.len())?;
        let field = |index| {
            table
                .range(index)
                .ok_or_else(|| invalid_data("missing block field"))
        };
        let header_range = field(0)?;
        if header_range.len() != packed::HeaderReader::TOTAL_SIZE {
            return Err(invalid_data("invalid archived header length"));
        }
        let bytes = data.read(header_range)?;
        let header = packed::HeaderReader::from_slice(&bytes).map_err(invalid_data)?;
        if header.calc_header_hash() != *hash {
            return Err(invalid_data("archive mapping hash mismatch"));
        }
        let uncles = field(1)?;
        let transactions = field(2)?;
        let proposals = field(3)?;
        let extension = table.range(4);
        Ok(Self {
            data,
            uncles,
            transactions,
            proposals,
            extension,
        })
    }

    /// Read one transaction, including its witnesses, without decoding its peers.
    pub fn transaction(&self, index: usize) -> Result<Option<packed::Transaction>, Error> {
        self.transaction_with_count(index)
            .map(|(transaction, _)| transaction)
    }

    /// Read a transaction and the total count from one validated offset table.
    pub fn transaction_with_count(
        &self,
        index: usize,
    ) -> Result<(Option<packed::Transaction>, usize), Error> {
        let table = offsets(&self.data, self.transactions.clone()).map_err(internal_error)?;
        let transaction = table
            .range(index)
            .map(|range| self.entity::<packed::TransactionReader>(range))
            .transpose()?;
        Ok((transaction, table.len()))
    }

    /// Read the uncle headers and their proposals.
    pub fn uncles(&self) -> Result<packed::UncleBlockVec, Error> {
        self.entity::<packed::UncleBlockVecReader>(self.uncles.clone())
    }

    /// Read the proposal identifiers.
    pub fn proposals(&self) -> Result<packed::ProposalShortIdVec, Error> {
        self.entity::<packed::ProposalShortIdVecReader>(self.proposals.clone())
    }

    /// Match Block::extension: an absent or invalid extra Bytes field is None.
    pub fn extension(&self) -> Result<Option<packed::Bytes>, Error> {
        let Some(range) = self.extension.clone() else {
            return Ok(None);
        };
        let bytes = self.data.read(range).map_err(internal_error)?;
        if packed::BytesReader::verify(&bytes, false).is_err() {
            return Ok(None);
        }
        Ok(Some(packed::Bytes::new_unchecked(
            bytes.into_owned().into(),
        )))
    }

    fn entity<'r, R: Reader<'r>>(&self, range: Range<usize>) -> Result<R::Entity, Error> {
        let bytes = self.data.read(range).map_err(internal_error)?;
        R::verify(&bytes, true).map_err(internal_error)?;
        Ok(R::Entity::new_unchecked(bytes.into_owned().into()))
    }
}

/// Molecule tables and dynamic vectors share the same offset representation.
/// Validate every offset before allowing a field or item range to escape.
struct Offsets<'a> {
    bytes: Cow<'a, [u8]>,
    range: Range<usize>,
}

impl Offsets<'_> {
    fn len(&self) -> usize {
        self.bytes.len() / 4
    }

    fn range(&self, index: usize) -> Option<Range<usize>> {
        if index >= self.len() {
            return None;
        }
        let start = number(&self.bytes[index * 4..]);
        let end = if index + 1 == self.len() {
            self.range.len()
        } else {
            number(&self.bytes[(index + 1) * 4..])
        };
        Some(self.range.start + start..self.range.start + end)
    }
}

fn offsets<'a>(data: &'a RecordRanges<'_>, range: Range<usize>) -> io::Result<Offsets<'a>> {
    if range.len() < 4 {
        return Err(invalid_data("truncated archive Molecule table"));
    }
    let header = data.read(range.start..range.start + range.len().min(8))?;
    if number(&header) != range.len() {
        return Err(invalid_data("archive Molecule table length mismatch"));
    }
    if range.len() == 4 {
        return Ok(Offsets {
            bytes: Cow::Borrowed(&[]),
            range,
        });
    }
    if header.len() < 8 {
        return Err(invalid_data("truncated archive Molecule offset"));
    }
    let first = number(&header[4..]);
    if first < 8 || !first.is_multiple_of(4) || first > range.len() {
        return Err(invalid_data("invalid archive Molecule first offset"));
    }
    let bytes = data.read(range.start + 4..range.start + first)?;
    let mut previous = first;
    for offset in bytes.chunks_exact(4).map(number) {
        if offset < previous || offset > range.len() {
            return Err(invalid_data("invalid archive Molecule offsets"));
        }
        previous = offset;
    }
    Ok(Offsets { bytes, range })
}

fn number(bytes: &[u8]) -> usize {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize
}
