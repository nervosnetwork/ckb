use ckb_traits::{BlockEpoch, EpochProvider};
use ckb_types::{
    core::{
        capacity_bytes, BlockBuilder, Capacity, EpochExt, HeaderBuilder, HeaderView,
        TransactionBuilder,
    },
    packed::Bytes,
    prelude::*,
    utilities::DIFF_TWO,
};

use crate::consensus::{
    build_genesis_epoch_ext, ConsensusBuilder, DEFAULT_EPOCH_DURATION_TARGET,
    DEFAULT_ORPHAN_RATE_TARGET, GENESIS_EPOCH_LENGTH,
};

#[test]
fn test_init_epoch_reward() {
    let cellbase = TransactionBuilder::default()
        .witness(Bytes::default())
        .build();
    let epoch_ext = build_genesis_epoch_ext(
        capacity_bytes!(100),
        DIFF_TWO,
        GENESIS_EPOCH_LENGTH,
        DEFAULT_EPOCH_DURATION_TARGET,
        DEFAULT_ORPHAN_RATE_TARGET,
    );
    let genesis = BlockBuilder::default().transaction(cellbase).build();
    let consensus = ConsensusBuilder::new(genesis, epoch_ext)
        .initial_primary_epoch_reward(capacity_bytes!(100))
        .build();
    assert_eq!(capacity_bytes!(100), consensus.initial_primary_epoch_reward);
}

#[test]
fn test_halving_epoch_reward() {
    let cellbase = TransactionBuilder::default()
        .witness(Bytes::default())
        .build();
    let epoch_ext = build_genesis_epoch_ext(
        capacity_bytes!(100),
        DIFF_TWO,
        GENESIS_EPOCH_LENGTH,
        DEFAULT_EPOCH_DURATION_TARGET,
        DEFAULT_ORPHAN_RATE_TARGET,
    );
    let genesis = BlockBuilder::default().transaction(cellbase).build();
    let consensus = ConsensusBuilder::new(genesis, epoch_ext)
        .initial_primary_epoch_reward(capacity_bytes!(100))
        .build();
    let genesis_epoch = consensus.genesis_epoch_ext();

    let header = |epoch: &EpochExt, number: u64| {
        HeaderBuilder::default()
            .number(number.pack())
            .epoch(epoch.number_with_fraction(number).pack())
            .build()
    };

    struct DummyEpochProvider(EpochExt);
    impl EpochProvider for DummyEpochProvider {
        fn get_epoch_ext(&self, _block_header: &HeaderView) -> Option<EpochExt> {
            Some(self.0.clone())
        }
        fn get_block_epoch(&self, block_header: &HeaderView) -> Option<BlockEpoch> {
            let block_epoch =
                if block_header.number() == self.0.start_number() + self.0.length() - 1 {
                    BlockEpoch::TailBlock {
                        epoch: self.0.clone(),
                        epoch_uncles_count: 0,
                        epoch_duration_in_milliseconds: DEFAULT_EPOCH_DURATION_TARGET * 1000,
                    }
                } else {
                    BlockEpoch::NonTailBlock {
                        epoch: self.0.clone(),
                    }
                };
            Some(block_epoch)
        }
    }
    let initial_primary_epoch_reward = genesis_epoch.primary_reward();

    {
        let epoch = consensus
            .next_epoch_ext(
                &header(&genesis_epoch, genesis_epoch.length() - 1),
                &DummyEpochProvider(genesis_epoch.clone()),
            )
            .expect("test: get next epoch")
            .epoch();

        assert_eq!(initial_primary_epoch_reward, epoch.primary_reward());
    }

    let first_halving_epoch_number = consensus.primary_epoch_reward_halving_interval();

    // first_halving_epoch_number - 2
    let epoch = genesis_epoch
        .clone()
        .into_builder()
        .number(first_halving_epoch_number - 2)
        .build();

    // first_halving_epoch_number - 1
    let epoch = consensus
        .next_epoch_ext(
            &header(&epoch, epoch.start_number() + epoch.length() - 1),
            &DummyEpochProvider(epoch),
        )
        .expect("test: get next epoch")
        .epoch();
    assert_eq!(initial_primary_epoch_reward, epoch.primary_reward());

    // first_halving_epoch_number
    let epoch = consensus
        .next_epoch_ext(
            &header(&epoch, epoch.start_number() + epoch.length() - 1),
            &DummyEpochProvider(epoch),
        )
        .expect("test: get next epoch")
        .epoch();

    assert_eq!(
        initial_primary_epoch_reward.as_u64() / 2,
        epoch.primary_reward().as_u64()
    );

    // first_halving_epoch_number + 1
    let epoch = consensus
        .next_epoch_ext(
            &header(&epoch, epoch.start_number() + epoch.length() - 1),
            &DummyEpochProvider(epoch),
        )
        .expect("test: get next epoch")
        .epoch();

    assert_eq!(
        initial_primary_epoch_reward.as_u64() / 2,
        epoch.primary_reward().as_u64()
    );

    // first_halving_epoch_number * 4 - 2
    let epoch = genesis_epoch
        .clone()
        .into_builder()
        .number(first_halving_epoch_number * 4 - 2)
        .base_block_reward(Capacity::shannons(
            initial_primary_epoch_reward.as_u64() / 8 / genesis_epoch.length(),
        ))
        .remainder_reward(Capacity::shannons(
            initial_primary_epoch_reward.as_u64() / 8 % genesis_epoch.length(),
        ))
        .build();

    // first_halving_epoch_number * 4 - 1
    let epoch = consensus
        .next_epoch_ext(
            &header(&epoch, epoch.start_number() + epoch.length() - 1),
            &DummyEpochProvider(epoch),
        )
        .expect("test: get next epoch")
        .epoch();

    assert_eq!(
        initial_primary_epoch_reward.as_u64() / 8,
        epoch.primary_reward().as_u64()
    );

    // first_halving_epoch_number * 4
    let epoch = consensus
        .next_epoch_ext(
            &header(&epoch, epoch.start_number() + epoch.length() - 1),
            &DummyEpochProvider(epoch),
        )
        .expect("test: get next epoch")
        .epoch();

    assert_eq!(
        initial_primary_epoch_reward.as_u64() / 16,
        epoch.primary_reward().as_u64()
    );
}
