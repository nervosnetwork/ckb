impl<'a> super::Bytes<'a> {
    pub fn from_slice(slice: &'a [u8]) -> Self {
        flatbuffers::get_root::<Self>(&slice)
    }
}

impl<'a> super::Header<'a> {
    pub fn from_slice(slice: &'a [u8]) -> Self {
        flatbuffers::get_root::<Self>(&slice)
    }
}

impl<'a> super::Block<'a> {
    pub fn from_slice(slice: &'a [u8]) -> Self {
        flatbuffers::get_root::<Self>(&slice)
    }
}

impl<'a> super::BlockBody<'a> {
    pub fn from_slice(slice: &'a [u8]) -> Self {
        flatbuffers::get_root::<Self>(&slice)
    }
}

impl<'a> super::UncleBlock<'a> {
    pub fn from_slice(slice: &'a [u8]) -> Self {
        flatbuffers::get_root::<Self>(&slice)
    }
}

impl<'a> super::Transaction<'a> {
    pub fn from_slice(slice: &'a [u8]) -> Self {
        flatbuffers::get_root::<Self>(&slice)
    }
}

impl<'a> super::Witness<'a> {
    pub fn from_slice(slice: &'a [u8]) -> Self {
        flatbuffers::get_root::<Self>(&slice)
    }
}

impl<'a> super::OutPoint<'a> {
    pub fn from_slice(slice: &'a [u8]) -> Self {
        flatbuffers::get_root::<Self>(&slice)
    }
}

impl<'a> super::CellInput<'a> {
    pub fn from_slice(slice: &'a [u8]) -> Self {
        flatbuffers::get_root::<Self>(&slice)
    }
}

impl<'a> super::CellOutput<'a> {
    pub fn from_slice(slice: &'a [u8]) -> Self {
        flatbuffers::get_root::<Self>(&slice)
    }
}

impl<'a> super::Script<'a> {
    pub fn from_slice(slice: &'a [u8]) -> Self {
        flatbuffers::get_root::<Self>(&slice)
    }
}

impl<'a> super::BlockExt<'a> {
    pub fn from_slice(slice: &'a [u8]) -> Self {
        flatbuffers::get_root::<Self>(&slice)
    }
}

impl<'a> super::TransactionMeta<'a> {
    pub fn from_slice(slice: &'a [u8]) -> Self {
        flatbuffers::get_root::<Self>(&slice)
    }
}

impl<'a> super::StoredBlock<'a> {
    pub fn from_slice(slice: &'a [u8]) -> Self {
        flatbuffers::get_root::<Self>(&slice)
    }
}

impl<'a> super::StoredBlockCache<'a> {
    pub fn from_slice(slice: &'a [u8]) -> Self {
        flatbuffers::get_root::<Self>(&slice)
    }
}

impl<'a> super::StoredBlockBody<'a> {
    pub fn from_slice(slice: &'a [u8]) -> Self {
        flatbuffers::get_root::<Self>(&slice)
    }
}

impl<'a> super::StoredBlockBodyCache<'a> {
    pub fn from_slice(slice: &'a [u8]) -> Self {
        flatbuffers::get_root::<Self>(&slice)
    }
}

impl<'a> super::StoredTransactionInfo<'a> {
    pub fn from_slice(slice: &'a [u8]) -> Self {
        flatbuffers::get_root::<Self>(&slice)
    }
}

impl<'a> super::StoredHeader<'a> {
    pub fn from_slice(slice: &'a [u8]) -> Self {
        flatbuffers::get_root::<Self>(&slice)
    }
}

impl<'a> super::StoredHeaderCache<'a> {
    pub fn from_slice(slice: &'a [u8]) -> Self {
        flatbuffers::get_root::<Self>(&slice)
    }
}

impl<'a> super::StoredUncleBlocks<'a> {
    pub fn from_slice(slice: &'a [u8]) -> Self {
        flatbuffers::get_root::<Self>(&slice)
    }
}

impl<'a> super::StoredUncleBlocksCache<'a> {
    pub fn from_slice(slice: &'a [u8]) -> Self {
        flatbuffers::get_root::<Self>(&slice)
    }
}

impl<'a> super::StoredProposalShortIds<'a> {
    pub fn from_slice(slice: &'a [u8]) -> Self {
        flatbuffers::get_root::<Self>(&slice)
    }
}

impl<'a> super::StoredEpochExt<'a> {
    pub fn from_slice(slice: &'a [u8]) -> Self {
        flatbuffers::get_root::<Self>(&slice)
    }
}

impl<'a> super::StoredCellMeta<'a> {
    pub fn from_slice(slice: &'a [u8]) -> Self {
        flatbuffers::get_root::<Self>(&slice)
    }
}
