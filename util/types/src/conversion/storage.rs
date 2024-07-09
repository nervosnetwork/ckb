use crate::{core, packed, prelude::*};

impl Pack<packed::HeaderView> for core::HeaderView {
    fn pack(&self) -> packed::HeaderView {
        packed::HeaderView::new_builder()
            .data(self.data())
            .hash(self.hash())
            .build()
    }
}

impl From<core::HeaderView> for packed::HeaderView {
    fn from(value: core::HeaderView) -> Self {
        (&value).into()
    }
}

impl From<&core::HeaderView> for packed::HeaderView {
    fn from(value: &core::HeaderView) -> Self {
        packed::HeaderView::new_builder()
            .data(value.data())
            .hash(value.hash())
            .build()
    }
}

impl<'r> Unpack<core::HeaderView> for packed::HeaderViewReader<'r> {
    fn unpack(&self) -> core::HeaderView {
        core::HeaderView {
            data: self.data().to_entity(),
            hash: self.hash().to_entity(),
        }
    }
}
impl_conversion_for_entity_unpack!(core::HeaderView, HeaderView);

impl<'r> From<packed::HeaderViewReader<'r>> for core::HeaderView {
    fn from(value: packed::HeaderViewReader<'r>) -> core::HeaderView {
        core::HeaderView {
            data: value.data().to_entity(),
            hash: value.hash().to_entity(),
        }
    }
}
impl_conversion_for_entity_from!(core::HeaderView, HeaderView);

impl Pack<packed::UncleBlockVecView> for core::UncleBlockVecView {
    fn pack(&self) -> packed::UncleBlockVecView {
        packed::UncleBlockVecView::new_builder()
            .data(self.data())
            .hashes(self.hashes())
            .build()
    }
}

impl From<core::UncleBlockVecView> for packed::UncleBlockVecView {
    fn from(value: core::UncleBlockVecView) -> Self {
        (&value).into()
    }
}

impl From<&core::UncleBlockVecView> for packed::UncleBlockVecView {
    fn from(value: &core::UncleBlockVecView) -> Self {
        packed::UncleBlockVecView::new_builder()
            .data(value.data())
            .hashes(value.hashes())
            .build()
    }
}

impl<'r> Unpack<core::UncleBlockVecView> for packed::UncleBlockVecViewReader<'r> {
    fn unpack(&self) -> core::UncleBlockVecView {
        core::UncleBlockVecView {
            data: self.data().to_entity(),
            hashes: self.hashes().to_entity(),
        }
    }
}
impl_conversion_for_entity_unpack!(core::UncleBlockVecView, UncleBlockVecView);

impl<'r> From<packed::UncleBlockVecViewReader<'r>> for core::UncleBlockVecView {
    fn from(value: packed::UncleBlockVecViewReader<'r>) -> core::UncleBlockVecView {
        core::UncleBlockVecView {
            data: value.data().to_entity(),
            hashes: value.hashes().to_entity(),
        }
    }
}
impl_conversion_for_entity_from!(core::UncleBlockVecView, UncleBlockVecView);

impl Pack<packed::TransactionView> for core::TransactionView {
    fn pack(&self) -> packed::TransactionView {
        packed::TransactionView::new_builder()
            .data(self.data())
            .hash(self.hash())
            .witness_hash(self.witness_hash())
            .build()
    }
}

impl From<core::TransactionView> for packed::TransactionView {
    fn from(value: core::TransactionView) -> Self {
        (&value).into()
    }
}

impl From<&core::TransactionView> for packed::TransactionView {
    fn from(value: &core::TransactionView) -> Self {
        packed::TransactionView::new_builder()
            .data(value.data())
            .hash(value.hash())
            .witness_hash(value.witness_hash())
            .build()
    }
}

impl<'r> Unpack<core::TransactionView> for packed::TransactionViewReader<'r> {
    fn unpack(&self) -> core::TransactionView {
        core::TransactionView {
            data: self.data().to_entity(),
            hash: self.hash().to_entity(),
            witness_hash: self.witness_hash().to_entity(),
        }
    }
}
impl_conversion_for_entity_unpack!(core::TransactionView, TransactionView);

impl<'r> From<packed::TransactionViewReader<'r>> for core::TransactionView {
    fn from(value: packed::TransactionViewReader<'r>) -> core::TransactionView {
        core::TransactionView {
            data: value.data().to_entity(),
            hash: value.hash().to_entity(),
            witness_hash: value.witness_hash().to_entity(),
        }
    }
}
impl_conversion_for_entity_from!(core::TransactionView, TransactionView);

impl<'r> Unpack<core::BlockExt> for packed::BlockExtReader<'r> {
    fn unpack(&self) -> core::BlockExt {
        core::BlockExt {
            received_at: self.received_at().unpack(),
            total_difficulty: self.total_difficulty().unpack(),
            total_uncles_count: self.total_uncles_count().unpack(),
            verified: self.verified().unpack(),
            txs_fees: self.txs_fees().unpack(),
            cycles: None,
            txs_sizes: None,
        }
    }
}
impl_conversion_for_entity_unpack!(core::BlockExt, BlockExt);

impl<'r> From<packed::BlockExtReader<'r>> for core::BlockExt {
    fn from(value: packed::BlockExtReader<'r>) -> core::BlockExt {
        core::BlockExt {
            received_at: value.received_at().into(),
            total_difficulty: value.total_difficulty().into(),
            total_uncles_count: value.total_uncles_count().into(),
            verified: value.verified().into(),
            txs_fees: value.txs_fees().into(),
            cycles: None,
            txs_sizes: None,
        }
    }
}
impl_conversion_for_entity_from!(core::BlockExt, BlockExt);

impl Pack<packed::BlockExtV1> for core::BlockExt {
    fn pack(&self) -> packed::BlockExtV1 {
        packed::BlockExtV1::new_builder()
            .received_at(self.received_at.pack())
            .total_difficulty(self.total_difficulty.pack())
            .total_uncles_count(self.total_uncles_count.pack())
            .verified(self.verified.pack())
            .txs_fees((self.txs_fees[..]).pack())
            .cycles(self.cycles.pack())
            .txs_sizes(self.txs_sizes.pack())
            .build()
    }
}

impl From<core::BlockExt> for packed::BlockExtV1 {
    fn from(value: core::BlockExt) -> Self {
        (&value).into()
    }
}

impl From<&core::BlockExt> for packed::BlockExtV1 {
    fn from(value: &core::BlockExt) -> Self {
        packed::BlockExtV1::new_builder()
            .received_at(value.received_at.into())
            .total_difficulty((&value.total_difficulty).into())
            .total_uncles_count(value.total_uncles_count.into())
            .verified(value.verified.into())
            .txs_fees((value.txs_fees[..]).into())
            .cycles(value.cycles.pack())
            .txs_sizes((&value.txs_sizes).into())
            .build()
    }
}

impl<'r> Unpack<core::BlockExt> for packed::BlockExtV1Reader<'r> {
    fn unpack(&self) -> core::BlockExt {
        core::BlockExt {
            received_at: self.received_at().unpack(),
            total_difficulty: self.total_difficulty().unpack(),
            total_uncles_count: self.total_uncles_count().unpack(),
            verified: self.verified().unpack(),
            txs_fees: self.txs_fees().unpack(),
            cycles: self.cycles().unpack(),
            txs_sizes: self.txs_sizes().unpack(),
        }
    }
}
impl_conversion_for_entity_unpack!(core::BlockExt, BlockExtV1);

impl<'r> From<packed::BlockExtV1Reader<'r>> for core::BlockExt {
    fn from(value: packed::BlockExtV1Reader<'r>) -> core::BlockExt {
        core::BlockExt {
            received_at: value.received_at().into(),
            total_difficulty: value.total_difficulty().into(),
            total_uncles_count: value.total_uncles_count().into(),
            verified: value.verified().into(),
            txs_fees: value.txs_fees().into(),
            cycles: value.cycles().into(),
            txs_sizes: value.txs_sizes().into(),
        }
    }
}
impl_conversion_for_entity_from!(core::BlockExt, BlockExtV1);

impl Pack<packed::EpochExt> for core::EpochExt {
    fn pack(&self) -> packed::EpochExt {
        packed::EpochExt::new_builder()
            .number(self.number().pack())
            .base_block_reward(self.base_block_reward().pack())
            .remainder_reward(self.remainder_reward().pack())
            .previous_epoch_hash_rate(self.previous_epoch_hash_rate().pack())
            .last_block_hash_in_previous_epoch(self.last_block_hash_in_previous_epoch())
            .start_number(self.start_number().pack())
            .length(self.length().pack())
            .compact_target(self.compact_target().pack())
            .build()
    }
}

impl From<core::EpochExt> for packed::EpochExt {
    fn from(value: core::EpochExt) -> Self {
        (&value).into()
    }
}

impl From<&core::EpochExt> for packed::EpochExt {
    fn from(value: &core::EpochExt) -> Self {
        packed::EpochExt::new_builder()
            .number(value.number().into())
            .base_block_reward(value.base_block_reward().into())
            .remainder_reward(value.remainder_reward().into())
            .previous_epoch_hash_rate(value.previous_epoch_hash_rate().into())
            .last_block_hash_in_previous_epoch(value.last_block_hash_in_previous_epoch())
            .start_number(value.start_number().into())
            .length(value.length().into())
            .compact_target(value.compact_target().into())
            .build()
    }
}

impl<'r> Unpack<core::EpochExt> for packed::EpochExtReader<'r> {
    fn unpack(&self) -> core::EpochExt {
        core::EpochExt {
            number: self.number().unpack(),
            base_block_reward: self.base_block_reward().unpack(),
            remainder_reward: self.remainder_reward().unpack(),
            previous_epoch_hash_rate: self.previous_epoch_hash_rate().unpack(),
            last_block_hash_in_previous_epoch: self.last_block_hash_in_previous_epoch().to_entity(),
            start_number: self.start_number().unpack(),
            length: self.length().unpack(),
            compact_target: self.compact_target().unpack(),
        }
    }
}
impl_conversion_for_entity_unpack!(core::EpochExt, EpochExt);

impl<'r> From<packed::EpochExtReader<'r>> for core::EpochExt {
    fn from(value: packed::EpochExtReader<'r>) -> core::EpochExt {
        core::EpochExt {
            number: value.number().into(),
            base_block_reward: value.base_block_reward().into(),
            remainder_reward: value.remainder_reward().into(),
            previous_epoch_hash_rate: value.previous_epoch_hash_rate().into(),
            last_block_hash_in_previous_epoch: value
                .last_block_hash_in_previous_epoch()
                .to_entity(),
            start_number: value.start_number().into(),
            length: value.length().into(),
            compact_target: value.compact_target().into(),
        }
    }
}
impl_conversion_for_entity_from!(core::EpochExt, EpochExt);

impl Pack<packed::TransactionInfo> for core::TransactionInfo {
    fn pack(&self) -> packed::TransactionInfo {
        let key = packed::TransactionKey::new_builder()
            .block_hash(self.block_hash.clone())
            .index(self.index.pack())
            .build();
        packed::TransactionInfo::new_builder()
            .key(key)
            .block_number(self.block_number.pack())
            .block_epoch(self.block_epoch.pack())
            .build()
    }
}

impl From<core::TransactionInfo> for packed::TransactionInfo {
    fn from(value: core::TransactionInfo) -> Self {
        (&value).into()
    }
}

impl From<&core::TransactionInfo> for packed::TransactionInfo {
    fn from(value: &core::TransactionInfo) -> Self {
        let key = packed::TransactionKey::new_builder()
            .block_hash(value.block_hash.clone())
            .index(value.index.into())
            .build();
        packed::TransactionInfo::new_builder()
            .key(key)
            .block_number(value.block_number.into())
            .block_epoch(value.block_epoch.into())
            .build()
    }
}

impl<'r> Unpack<core::TransactionInfo> for packed::TransactionInfoReader<'r> {
    fn unpack(&self) -> core::TransactionInfo {
        core::TransactionInfo {
            block_hash: self.key().block_hash().to_entity(),
            index: self.key().index().unpack(),
            block_number: self.block_number().unpack(),
            block_epoch: self.block_epoch().unpack(),
        }
    }
}
impl_conversion_for_entity_unpack!(core::TransactionInfo, TransactionInfo);

impl<'r> From<packed::TransactionInfoReader<'r>> for core::TransactionInfo {
    fn from(value: packed::TransactionInfoReader<'r>) -> core::TransactionInfo {
        core::TransactionInfo {
            block_hash: value.key().block_hash().to_entity(),
            index: value.key().index().into(),
            block_number: value.block_number().into(),
            block_epoch: value.block_epoch().into(),
        }
    }
}
impl_conversion_for_entity_from!(core::TransactionInfo, TransactionInfo);
