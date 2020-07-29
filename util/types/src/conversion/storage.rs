use crate::{core, packed, prelude::*};

impl Pack<packed::HeaderView> for core::HeaderView {
    fn pack(&self) -> packed::HeaderView {
        packed::HeaderView::new_builder()
            .data(self.data())
            .hash(self.hash())
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

impl Pack<packed::UncleBlockVecView> for core::UncleBlockVecView {
    fn pack(&self) -> packed::UncleBlockVecView {
        packed::UncleBlockVecView::new_builder()
            .data(self.data())
            .hashes(self.hashes())
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

impl Pack<packed::TransactionView> for core::TransactionView {
    fn pack(&self) -> packed::TransactionView {
        packed::TransactionView::new_builder()
            .data(self.data())
            .hash(self.hash())
            .witness_hash(self.witness_hash())
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

impl Pack<packed::BlockExt> for core::BlockExt {
    fn pack(&self) -> packed::BlockExt {
        packed::BlockExt::new_builder()
            .received_at(self.received_at.pack())
            .total_difficulty(self.total_difficulty.pack())
            .total_uncles_count(self.total_uncles_count.pack())
            .verified(self.verified.pack())
            .txs_fees((&self.txs_fees[..]).pack())
            .build()
    }
}

impl<'r> Unpack<core::BlockExt> for packed::BlockExtReader<'r> {
    fn unpack(&self) -> core::BlockExt {
        core::BlockExt {
            received_at: self.received_at().unpack(),
            total_difficulty: self.total_difficulty().unpack(),
            total_uncles_count: self.total_uncles_count().unpack(),
            verified: self.verified().unpack(),
            txs_fees: self.txs_fees().unpack(),
        }
    }
}
impl_conversion_for_entity_unpack!(core::BlockExt, BlockExt);

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
