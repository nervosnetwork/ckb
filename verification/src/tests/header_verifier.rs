use crate::header_verifier::{
    EpochVerifier, NumberVerifier, PowVerifier, TimestampVerifier, VersionVerifier,
};
use crate::{
    BlockVersionError, EpochError, NumberError, PowError, TimestampError, ALLOWED_FUTURE_BLOCKTIME,
};
use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_error::assert_error_eq;
use ckb_pow::PowEngine;
use ckb_systemtime::unix_time_as_millis;
use ckb_test_chain_utils::{MockMedianTime, MOCK_MEDIAN_TIME_COUNT};
use ckb_types::{
    core::{
        hardfork::{HardForks, CKB2021, CKB2023},
        EpochNumberWithFraction, HeaderBuilder,
    },
    packed::Header,
    prelude::*,
};

use super::BuilderBaseOnBlockNumber;

fn mock_median_time_context() -> MockMedianTime {
    let now = unix_time_as_millis();
    let timestamps = (0..100).map(|_| now).collect();
    MockMedianTime::new(timestamps)
}

#[test]
pub fn test_version() {
    let hardfork_switch = HardForks {
        ckb2021: CKB2021::new_mirana(),
        ckb2023: CKB2023::new_mirana()
            .as_builder()
            .rfc_0146(10)
            .build()
            .unwrap(),
    };
    let consensus = ConsensusBuilder::default()
        .hardfork_switch(hardfork_switch)
        .build();

    let header = HeaderBuilder::default()
        .version((consensus.block_version() + 1).pack())
        .build();
    let verifier = VersionVerifier::new(&header, &consensus);

    assert_error_eq!(
        verifier.verify().unwrap_err(),
        BlockVersionError {
            expected: consensus.block_version(),
            actual: consensus.block_version() + 1
        }
    );
    let epoch = EpochNumberWithFraction::new(10, 40, 1000);
    let header = HeaderBuilder::default()
        .version((consensus.block_version() + 1).pack())
        .epoch(epoch.pack())
        .build();
    let verifier = VersionVerifier::new(&header, &consensus);
    assert!(verifier.verify().is_ok());
}

#[cfg(not(disable_faketime))]
#[test]
fn test_timestamp() {
    let _faketime_guard = ckb_systemtime::faketime();
    _faketime_guard.set_faketime(100_000);
    let fake_block_median_time_context = mock_median_time_context();
    let parent_hash = fake_block_median_time_context.get_block_hash(99);
    let timestamp = unix_time_as_millis() + 1;
    let header = HeaderBuilder::new_with_number(100)
        .parent_hash(parent_hash)
        .timestamp(timestamp.pack())
        .build();
    let timestamp_verifier = TimestampVerifier::new(
        &fake_block_median_time_context,
        &header,
        MOCK_MEDIAN_TIME_COUNT,
    );

    assert!(timestamp_verifier.verify().is_ok());
}

#[cfg(not(disable_faketime))]
#[test]
fn test_timestamp_too_old() {
    let _faketime_guard = ckb_systemtime::faketime();
    _faketime_guard.set_faketime(100_000);
    let fake_block_median_time_context = mock_median_time_context();
    let parent_hash = fake_block_median_time_context.get_block_hash(99);

    let min = unix_time_as_millis();
    let timestamp = unix_time_as_millis() - 1;
    let header = HeaderBuilder::new_with_number(100)
        .parent_hash(parent_hash)
        .timestamp(timestamp.pack())
        .build();
    let timestamp_verifier = TimestampVerifier::new(
        &fake_block_median_time_context,
        &header,
        MOCK_MEDIAN_TIME_COUNT,
    );

    assert_error_eq!(
        timestamp_verifier.verify().unwrap_err(),
        TimestampError::BlockTimeTooOld {
            min,
            actual: timestamp,
        },
    );
}

#[cfg(not(disable_faketime))]
#[test]
fn test_timestamp_too_new() {
    let _faketime_guard = ckb_systemtime::faketime();
    _faketime_guard.set_faketime(100_000);
    let fake_block_median_time_context = mock_median_time_context();
    let parent_hash = fake_block_median_time_context.get_block_hash(99);

    let max = unix_time_as_millis() + ALLOWED_FUTURE_BLOCKTIME;
    let timestamp = max + 1;
    let header = HeaderBuilder::new_with_number(100)
        .parent_hash(parent_hash)
        .timestamp(timestamp.pack())
        .build();
    let timestamp_verifier = TimestampVerifier::new(
        &fake_block_median_time_context,
        &header,
        MOCK_MEDIAN_TIME_COUNT,
    );
    assert_error_eq!(
        timestamp_verifier.verify().unwrap_err(),
        TimestampError::BlockTimeTooNew {
            max,
            actual: timestamp,
        },
    );
}

#[test]
fn test_number() {
    let parent = HeaderBuilder::new_with_number(10).build();
    let header = HeaderBuilder::new_with_number(10).build();

    let verifier = NumberVerifier::new(&parent, &header);
    assert_error_eq!(
        verifier.verify().unwrap_err(),
        NumberError {
            expected: 11,
            actual: 10,
        },
    );
}

#[test]
fn test_epoch() {
    {
        let parent = HeaderBuilder::default()
            .number(1u64.pack())
            .epoch(EpochNumberWithFraction::new(1, 1, 10).pack())
            .build();
        let epochs_malformed = vec![
            EpochNumberWithFraction::new_unchecked(1, 0, 0),
            EpochNumberWithFraction::new_unchecked(1, 10, 0),
            EpochNumberWithFraction::new_unchecked(1, 10, 5),
            EpochNumberWithFraction::new_unchecked(1, 10, 10),
        ];

        for epoch_malformed in epochs_malformed {
            let malformed = HeaderBuilder::default()
                .epoch(epoch_malformed.pack())
                .build();
            let result = EpochVerifier::new(&parent, &malformed).verify();
            assert!(result.is_err(), "input: {epoch_malformed:#}");
            assert_error_eq!(
                result.unwrap_err(),
                EpochError::Malformed {
                    value: epoch_malformed
                },
            );
        }
    }
    {
        let epochs = vec![
            (
                EpochNumberWithFraction::new_unchecked(1, 5, 10),
                EpochNumberWithFraction::new_unchecked(1, 5, 10),
            ),
            (
                EpochNumberWithFraction::new_unchecked(1, 5, 10),
                EpochNumberWithFraction::new_unchecked(1, 5, 11),
            ),
            (
                EpochNumberWithFraction::new_unchecked(1, 5, 10),
                EpochNumberWithFraction::new_unchecked(2, 5, 10),
            ),
            (
                EpochNumberWithFraction::new_unchecked(1, 5, 10),
                EpochNumberWithFraction::new_unchecked(1, 6, 11),
            ),
            (
                EpochNumberWithFraction::new_unchecked(1, 5, 10),
                EpochNumberWithFraction::new_unchecked(2, 6, 10),
            ),
            (
                EpochNumberWithFraction::new_unchecked(1, 9, 10),
                EpochNumberWithFraction::new_unchecked(2, 1, 10),
            ),
            (
                EpochNumberWithFraction::new_unchecked(1, 9, 10),
                EpochNumberWithFraction::new_unchecked(3, 0, 10),
            ),
        ];

        for (epoch_parent, epoch_current) in epochs {
            let parent = HeaderBuilder::default()
                .number(1u64.pack())
                .epoch(epoch_parent.pack())
                .build();
            let current = HeaderBuilder::default().epoch(epoch_current.pack()).build();

            let result = EpochVerifier::new(&parent, &current).verify();
            assert!(result.is_err(), "current: {current:#}, parent: {parent:#}");
            assert_error_eq!(
                result.unwrap_err(),
                EpochError::NonContinuous {
                    current: epoch_current,
                    parent: epoch_parent,
                },
            );
        }
    }
    {
        let epochs = vec![
            (
                EpochNumberWithFraction::new_unchecked(1, 5, 10),
                EpochNumberWithFraction::new_unchecked(1, 6, 10),
            ),
            (
                EpochNumberWithFraction::new_unchecked(1, 9, 10),
                EpochNumberWithFraction::new_unchecked(2, 0, 10),
            ),
            (
                EpochNumberWithFraction::new_unchecked(1, 9, 10),
                EpochNumberWithFraction::new_unchecked(2, 0, 11),
            ),
        ];
        for (epoch_parent, epoch_current) in epochs {
            let parent = HeaderBuilder::default()
                .number(1u64.pack())
                .epoch(epoch_parent.pack())
                .build();
            let current = HeaderBuilder::default().epoch(epoch_current.pack()).build();

            let result = EpochVerifier::new(&parent, &current).verify();
            assert!(result.is_ok(), "current: {current:#}, parent: {parent:#}");
        }
    }
}

struct FakePowEngine;

impl PowEngine for FakePowEngine {
    fn verify(&self, _header: &Header) -> bool {
        false
    }
}

#[test]
fn test_pow_verifier() {
    let header = HeaderBuilder::default().build();
    let fake_pow_engine: &dyn PowEngine = &FakePowEngine;
    let verifier = PowVerifier::new(&header, fake_pow_engine);

    assert_error_eq!(verifier.verify().unwrap_err(), PowError::InvalidNonce);
}
