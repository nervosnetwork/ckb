use crate::header_verifier::{NumberVerifier, PowVerifier, TimestampVerifier, VersionVerifier};
use crate::{BlockErrorKind, NumberError, PowError, TimestampError, ALLOWED_FUTURE_BLOCKTIME};
use ckb_error::assert_error_eq;
use ckb_pow::PowEngine;
use ckb_test_chain_utils::MockMedianTime;
use ckb_types::{
    constants::BLOCK_VERSION,
    core::{HeaderBuilder, HeaderContext},
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
    let header = HeaderBuilder::default()
        .version((BLOCK_VERSION + 1).pack())
        .build();
    let verifier = VersionVerifier::new(&header, BLOCK_VERSION);

    assert_error_eq!(verifier.verify().unwrap_err(), BlockErrorKind::Version);
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
    let timestamp_verifier = TimestampVerifier::new(&fake_block_median_time_context, &header);

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
    let timestamp_verifier = TimestampVerifier::new(&fake_block_median_time_context, &header);

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
    let timestamp_verifier = TimestampVerifier::new(&fake_block_median_time_context, &header);
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
    fn verify(&self, _header_ctx: &HeaderContext) -> bool {
        false
    }
}

#[test]
fn test_pow_verifier() {
    let header = HeaderBuilder::default().build();
    let header_ctx = HeaderContext::new(header);
    let fake_pow_engine: &dyn PowEngine = &FakePowEngine;
    let verifier = PowVerifier::new(&header_ctx, fake_pow_engine);

    assert_error_eq!(verifier.verify().unwrap_err(), PowError::InvalidNonce);
}
