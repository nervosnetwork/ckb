use crate::header_verifier::{NumberVerifier, PowVerifier, TimestampVerifier, VersionVerifier};
use crate::{BlockVersionError, NumberError, PowError, TimestampError, ALLOWED_FUTURE_BLOCKTIME};
use ckb_chain_spec::consensus::ConsensusBuilder;
use ckb_error::assert_error_eq;
use ckb_pow::PowEngine;
use ckb_test_chain_utils::{MockMedianTime, MOCK_MEDIAN_TIME_COUNT};
use ckb_types::{
    core::{hardfork::HardForkSwitch, EpochNumberWithFraction, HeaderBuilder},
    packed::Header,
    prelude::*,
};
use faketime::unix_time_as_millis;

fn mock_median_time_context() -> MockMedianTime {
    let now = unix_time_as_millis();
    let timestamps = (0..100).map(|_| now).collect();
    MockMedianTime::new(timestamps)
}

#[test]
pub fn test_version() {
    let fork_at = 10;
    let default_block_version = ConsensusBuilder::default().build().block_version(fork_at);
    let epoch = EpochNumberWithFraction::new(fork_at, 0, 10);
    let header1 = HeaderBuilder::default()
        .version(default_block_version.pack())
        .epoch(epoch.pack())
        .build();
    let header2 = HeaderBuilder::default()
        .version((default_block_version + 1).pack())
        .epoch(epoch.pack())
        .build();
    {
        let hardfork_switch = HardForkSwitch::new_without_any_enabled()
            .as_builder()
            .rfc_pr_0230(fork_at + 1)
            .build()
            .unwrap();
        let consensus = ConsensusBuilder::default()
            .hardfork_switch(hardfork_switch)
            .build();
        let result = VersionVerifier::new(&header1, &consensus).verify();
        assert!(result.is_ok(), "result = {:?}", result);

        let result = VersionVerifier::new(&header2, &consensus).verify();
        assert_error_eq!(
            result.unwrap_err(),
            BlockVersionError {
                expected: default_block_version,
                actual: default_block_version + 1
            }
        );
    }
    {
        let hardfork_switch = HardForkSwitch::new_without_any_enabled()
            .as_builder()
            .rfc_pr_0230(fork_at)
            .build()
            .unwrap();
        let consensus = ConsensusBuilder::default()
            .hardfork_switch(hardfork_switch)
            .build();
        let result = VersionVerifier::new(&header1, &consensus).verify();
        assert!(result.is_ok(), "result = {:?}", result);

        let result = VersionVerifier::new(&header2, &consensus).verify();
        assert!(result.is_ok(), "result = {:?}", result);
    }
}

#[cfg(not(disable_faketime))]
#[test]
fn test_timestamp() {
    let faketime_file = faketime::millis_tempfile(100_000).expect("create faketime file");
    faketime::enable(&faketime_file);
    let fake_block_median_time_context = mock_median_time_context();

    let timestamp = unix_time_as_millis() + 1;
    let header = HeaderBuilder::default()
        .number(10u64.pack())
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
    let faketime_file = faketime::millis_tempfile(100_000).expect("create faketime file");
    faketime::enable(&faketime_file);
    let fake_block_median_time_context = mock_median_time_context();

    let min = unix_time_as_millis();
    let timestamp = unix_time_as_millis() - 1;
    let header = HeaderBuilder::default()
        .number(10u64.pack())
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
    let faketime_file = faketime::millis_tempfile(100_000).expect("create faketime file");
    faketime::enable(&faketime_file);
    let fake_block_median_time_context = mock_median_time_context();

    let max = unix_time_as_millis() + ALLOWED_FUTURE_BLOCKTIME;
    let timestamp = max + 1;
    let header = HeaderBuilder::default()
        .number(10u64.pack())
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
    let parent = HeaderBuilder::default().number(10u64.pack()).build();
    let header = HeaderBuilder::default().number(10u64.pack()).build();

    let verifier = NumberVerifier::new(&parent, &header);
    assert_error_eq!(
        verifier.verify().unwrap_err(),
        NumberError {
            expected: 11,
            actual: 10,
        },
    );
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
