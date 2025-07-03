use crate::{bytes, core, generated::packed, prelude::*, vec::Vec};

type BlockNumber = u64;

impl packed::Byte32 {
    /// Creates a new `Bytes32` whose bits are all zeros.
    pub fn zero() -> Self {
        Self::default()
    }

    /// Creates a new `Byte32` whose bits are all ones.
    pub fn max_value() -> Self {
        [u8::MAX; 32].into()
    }

    /// Checks whether all bits in self are zeros.
    pub fn is_zero(&self) -> bool {
        self.as_slice().iter().all(|x| *x == 0)
    }

    /// Creates a new `Bytes32`.
    pub fn new(v: [u8; 32]) -> Self {
        v.into()
    }
}

impl packed::ProposalShortId {
    /// Creates a new `ProposalShortId` from a transaction hash.
    pub fn from_tx_hash(h: &packed::Byte32) -> Self {
        let mut inner = [0u8; 10];
        inner.copy_from_slice(&h.as_slice()[..10]);
        inner.into()
    }

    /// Creates a new `ProposalShortId` whose bits are all zeros.
    pub fn zero() -> Self {
        Self::default()
    }

    /// Creates a new `ProposalShortId`.
    pub fn new(v: [u8; 10]) -> Self {
        v.into()
    }
}

impl packed::OutPoint {
    /// Creates a new `OutPoint`.
    pub fn new(tx_hash: packed::Byte32, index: u32) -> Self {
        packed::OutPoint::new_builder()
            .tx_hash(tx_hash)
            .index(index)
            .build()
    }

    /// Creates a new null `OutPoint`.
    pub fn null() -> Self {
        packed::OutPoint::new_builder().index(u32::MAX).build()
    }

    /// Checks whether self is a null `OutPoint`.
    pub fn is_null(&self) -> bool {
        self.tx_hash().is_zero() && Into::<u32>::into(self.index().as_reader()) == u32::MAX
    }

    /// Generates a binary data to be used as a key for indexing cells in storage.
    ///
    /// # Notice
    ///
    /// The difference between [`Self::as_slice()`](../prelude/trait.Entity.html#tymethod.as_slice)
    /// and [`Self::to_cell_key()`](#method.to_cell_key) is the byteorder of the field `index`.
    ///
    /// - Uses little endian for the field `index` in serialization.
    ///
    ///   Because in the real world, the little endian machines make up the majority, we can cast
    ///   it as a number without re-order the bytes.
    ///
    /// - Uses big endian for the field `index` to index cells in storage.
    ///
    ///   So we can use `tx_hash` as key prefix to seek the cells from storage in the forward
    ///   order, so as to traverse cells in the forward order too.
    pub fn to_cell_key(&self) -> Vec<u8> {
        let mut key = Vec::with_capacity(36);
        let index: u32 = self.index().as_reader().into();
        key.extend_from_slice(self.tx_hash().as_slice());
        key.extend_from_slice(&index.to_be_bytes());
        key
    }
}

impl packed::CellInput {
    /// Creates a new `CellInput`.
    pub fn new(previous_output: packed::OutPoint, block_number: BlockNumber) -> Self {
        packed::CellInput::new_builder()
            .since(block_number)
            .previous_output(previous_output)
            .build()
    }
    /// Creates a new `CellInput` with a null `OutPoint`.
    pub fn new_cellbase_input(block_number: BlockNumber) -> Self {
        Self::new(packed::OutPoint::null(), block_number)
    }
}

impl packed::Script {
    /// Converts self into bytes of [`CellbaseWitness`](struct.CellbaseWitness.html).
    pub fn into_witness(self) -> packed::Bytes {
        packed::CellbaseWitness::new_builder()
            .lock(self)
            .build()
            .as_bytes()
            .into()
    }

    /// Converts from bytes of [`CellbaseWitness`](struct.CellbaseWitness.html).
    pub fn from_witness(witness: packed::Bytes) -> Option<Self> {
        packed::CellbaseWitness::from_slice(&witness.raw_data())
            .map(|cellbase_witness| cellbase_witness.lock())
            .ok()
    }

    /// Checks whether the own [`hash_type`](#method.hash_type) is
    /// [`Type`](../core/enum.ScriptHashType.html#variant.Type).
    pub fn is_hash_type_type(&self) -> bool {
        Into::<u8>::into(self.hash_type()) == Into::<u8>::into(core::ScriptHashType::Type)
    }
}

impl packed::Transaction {
    /// Checks whether self is a cellbase.
    pub fn is_cellbase(&self) -> bool {
        let raw_tx = self.raw();
        raw_tx.inputs().len() == 1
            && self.witnesses().len() == 1
            && raw_tx
                .inputs()
                .get(0)
                .should_be_ok()
                .previous_output()
                .is_null()
    }

    /// Generates a proposal short id after calculating the transaction hash.
    pub fn proposal_short_id(&self) -> packed::ProposalShortId {
        packed::ProposalShortId::from_tx_hash(&self.calc_tx_hash())
    }
}

impl packed::Block {
    /// Converts self to an uncle block.
    pub fn as_uncle(&self) -> packed::UncleBlock {
        packed::UncleBlock::new_builder()
            .header(self.header())
            .proposals(self.proposals())
            .build()
    }

    /// Gets the i-th extra field if it exists; i started from 0.
    pub fn extra_field(&self, index: usize) -> Option<bytes::Bytes> {
        let count = self.count_extra_fields();
        if count > index {
            let slice = self.as_slice();
            let i = (1 + Self::FIELD_COUNT + index) * molecule::NUMBER_SIZE;
            let start = molecule::unpack_number(&slice[i..]) as usize;
            if count == index + 1 {
                Some(self.as_bytes().slice(start..))
            } else {
                let j = i + molecule::NUMBER_SIZE;
                let end = molecule::unpack_number(&slice[j..]) as usize;
                Some(self.as_bytes().slice(start..end))
            }
        } else {
            None
        }
    }

    /// Gets the extension field if it existed.
    ///
    /// # Panics
    ///
    /// Panics if the first extra field exists but not a valid [`Bytes`](struct.Bytes.html).
    pub fn extension(&self) -> Option<packed::Bytes> {
        self.extra_field(0)
            .map(|data| packed::Bytes::from_slice(&data).unwrap())
    }
}

impl packed::CompactBlock {
    /// Calculates the length of transactions.
    pub fn txs_len(&self) -> usize {
        self.prefilled_transactions().len() + self.short_ids().len()
    }

    /// Gets the i-th extra field if it exists; i started from 0.
    pub fn extra_field(&self, index: usize) -> Option<bytes::Bytes> {
        let count = self.count_extra_fields();
        if count > index {
            let slice = self.as_slice();
            let i = (1 + Self::FIELD_COUNT + index) * molecule::NUMBER_SIZE;
            let start = molecule::unpack_number(&slice[i..]) as usize;
            if count == index + 1 {
                Some(self.as_bytes().slice(start..))
            } else {
                let j = i + molecule::NUMBER_SIZE;
                let end = molecule::unpack_number(&slice[j..]) as usize;
                Some(self.as_bytes().slice(start..end))
            }
        } else {
            None
        }
    }

    /// Gets the extension field if it existed.
    ///
    /// # Panics
    ///
    /// Panics if the first extra field exists but not a valid [`Bytes`](struct.Bytes.html).
    pub fn extension(&self) -> Option<packed::Bytes> {
        self.extra_field(0)
            .map(|data| packed::Bytes::from_slice(&data).unwrap())
    }
}

impl packed::BlockV1 {
    /// Converts to a compatible [`Block`](struct.Block.html) with an extra field.
    pub fn as_v0(&self) -> packed::Block {
        packed::Block::new_unchecked(self.as_bytes())
    }
}

impl<'r> packed::BlockReader<'r> {
    /// Gets the i-th extra field if it exists; i started from 0.
    pub fn extra_field(&self, index: usize) -> Option<&[u8]> {
        let count = self.count_extra_fields();
        if count > index {
            let slice = self.as_slice();
            let i = (1 + Self::FIELD_COUNT + index) * molecule::NUMBER_SIZE;
            let start = molecule::unpack_number(&slice[i..]) as usize;
            if count == index + 1 {
                Some(&self.as_slice()[start..])
            } else {
                let j = i + molecule::NUMBER_SIZE;
                let end = molecule::unpack_number(&slice[j..]) as usize;
                Some(&self.as_slice()[start..end])
            }
        } else {
            None
        }
    }

    /// Gets the extension field if it existed.
    ///
    /// # Panics
    ///
    /// Panics if the first extra field exists but not a valid [`BytesReader`](struct.BytesReader.html).
    pub fn extension(&self) -> Option<packed::BytesReader> {
        self.extra_field(0)
            .map(|data| packed::BytesReader::from_slice(data).unwrap())
    }
}

impl<'r> packed::BlockV1Reader<'r> {
    /// Converts to a compatible [`BlockReader`](struct.BlockReader.html) with an extra field.
    pub fn as_v0(&self) -> packed::BlockReader {
        packed::BlockReader::new_unchecked(self.as_slice())
    }
}

impl packed::CompactBlockV1 {
    /// Converts to a compatible [`CompactBlock`](struct.CompactBlock.html) with an extra field.
    pub fn as_v0(&self) -> packed::CompactBlock {
        packed::CompactBlock::new_unchecked(self.as_bytes())
    }
}

impl<'r> packed::CompactBlockV1Reader<'r> {
    /// Converts to a compatible [`CompactBlockReader`](struct.CompactBlockReader.html) with an extra field.
    pub fn as_v0(&self) -> packed::CompactBlockReader {
        packed::CompactBlockReader::new_unchecked(self.as_slice())
    }
}

impl AsRef<[u8]> for packed::TransactionKey {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl packed::HeaderDigest {
    /// Checks if the `HeaderDigest` is the default value.
    pub fn is_default(&self) -> bool {
        let default = Self::default();
        self.as_slice() == default.as_slice()
    }
}

impl From<packed::SendBlocksProofV1> for packed::LightClientMessageUnion {
    fn from(item: packed::SendBlocksProofV1) -> Self {
        packed::LightClientMessageUnion::SendBlocksProof(packed::SendBlocksProof::new_unchecked(
            item.as_bytes(),
        ))
    }
}

impl From<packed::SendTransactionsProofV1> for packed::LightClientMessageUnion {
    fn from(item: packed::SendTransactionsProofV1) -> Self {
        packed::LightClientMessageUnion::SendTransactionsProof(
            packed::SendTransactionsProof::new_unchecked(item.as_bytes()),
        )
    }
}
