//! Rows of one epoch CF, sorted as a metadata row at the 32-byte block hash
//! followed by existing TransactionView values keyed by hash and big-endian index.
use crate::{ChainStore, archive::invalid};
use ckb_db::{Direction, IteratorMode, iter::DBIter};
use ckb_error::Error;
use ckb_types::{
    core::{BlockView, TransactionView},
    packed,
    prelude::*,
};

fn transaction_key(hash: &packed::Byte32, index: usize) -> packed::TransactionKey {
    packed::TransactionKey::new_builder()
        .block_hash(hash.clone())
        .index(index)
        .build()
}

/// Emit every row in one insertion transaction. Keep the ordinary transaction
/// encoding so a point lookup reads one row and uses its stored hashes.
pub(super) fn write_rows(
    block: &BlockView,
    mut put: impl FnMut(&[u8], &[u8]) -> Result<(), Error>,
) -> Result<(), Error> {
    let hash = block.hash();
    let fields = if let Some(extension) = block.extension() {
        packed::BlockV1::new_builder()
            .header(block.data().header())
            .uncles(block.data().uncles())
            .proposals(block.data().proposals())
            .extension(extension)
            .build()
            .as_v0()
    } else {
        block
            .data()
            .as_builder()
            .transactions(packed::TransactionVec::default())
            .build()
    };
    let mut meta = Vec::with_capacity(4 + fields.as_slice().len());
    let count = u32::try_from(block.data().transactions().len()).map_err(invalid)?;
    meta.extend_from_slice(&count.to_le_bytes());
    meta.extend_from_slice(fields.as_slice());
    put(hash.as_slice(), &meta)?;
    for (i, tx) in block.transactions().into_iter().enumerate() {
        let value: packed::TransactionView = tx.into();
        put(transaction_key(&hash, i).as_slice(), value.as_slice())?;
    }
    Ok(())
}

fn metadata(hash: &packed::Byte32, meta: &[u8]) -> Result<(u32, packed::Block), Error> {
    let (count, fields) = meta
        .split_first_chunk::<4>()
        .ok_or_else(|| invalid("short epoch metadata"))?;
    let fields = packed::Block::from_compatible_slice(fields).map_err(invalid)?;
    if fields.calc_header_hash() != *hash
        || !fields.transactions().is_empty()
        || fields.as_reader().count_extra_fields() > 1
    {
        return Err(invalid("invalid epoch metadata"));
    }
    Ok((u32::from_le_bytes(*count), fields))
}

/// Consume exactly one physical member. A missing, extra, orphan or
/// out-of-order transaction makes a full scan fail before CF retirement.
pub(super) fn next_block(iter: &mut DBIter<'_>) -> Result<Option<BlockView>, Error> {
    let Some((key, value)) = iter.next() else {
        iter.status().map_err(invalid)?;
        return Ok(None);
    };
    let hash = packed::Byte32::from_slice(&key).map_err(invalid)?;
    let (count, fields) = metadata(&hash, &value)?;
    let mut body = Vec::new();
    for index in 0..count {
        let (key, value) = iter
            .next()
            .ok_or_else(|| invalid("incomplete epoch body"))?;
        if key.as_ref() != transaction_key(&hash, index as usize).as_slice() {
            return Err(invalid("epoch transaction index mismatch"));
        }
        body.push(
            packed::TransactionViewReader::from_slice(&value)
                .map_err(invalid)?
                .into(),
        );
    }
    let h = fields.header().into_view();
    let uncles = fields.uncles().into();
    let proposals = fields.proposals();
    Ok(Some(if let Some(ext) = fields.extension() {
        BlockView::new_unchecked_with_extension(h, uncles, body, proposals, ext)
    } else {
        BlockView::new_unchecked(h, uncles, body, proposals)
    }))
}

pub(super) struct SegmentBlock {
    pub start: u64,
    pub hash: packed::Byte32,
}

impl SegmentBlock {
    pub fn fields(&self, store: &impl ChainStore) -> Result<packed::Block, Error> {
        let meta = store
            .segment_value(self.start, self.hash.as_slice())?
            .ok_or_else(|| invalid("missing indexed epoch metadata"))?;
        metadata(&self.hash, &meta).map(|(_, fields)| fields)
    }

    pub fn read(&self, store: &impl ChainStore) -> Result<BlockView, Error> {
        let mut iter = store.segment_rows(
            self.start,
            IteratorMode::From(self.hash.as_slice(), Direction::Forward),
        )?;
        let block = next_block(&mut iter)?.ok_or_else(|| invalid("missing indexed epoch block"))?;
        if block.hash() != self.hash {
            return Err(invalid("indexed epoch block has a different hash"));
        }
        Ok(block)
    }

    pub fn transaction(
        &self,
        store: &impl ChainStore,
        index: usize,
    ) -> Result<Option<TransactionView>, Error> {
        store
            .segment_value(self.start, transaction_key(&self.hash, index).as_slice())?
            .map(|raw| {
                packed::TransactionViewReader::from_slice(&raw)
                    .map(Into::into)
                    .map_err(invalid)
            })
            .transpose()
    }
}
