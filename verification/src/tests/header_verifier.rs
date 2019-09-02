use crate::header_verifier::{
    EpochVerifier, HeaderResolver, NumberVerifier, PowVerifier, TimestampVerifier, VersionVerifier,
};
use crate::{
    BlockErrorKind, EpochError, NumberError, PowError, TimestampError, ALLOWED_FUTURE_BLOCKTIME,
};
use ckb_error::assert_error_eq;
use ckb_pow::PowEngine;
use ckb_test_chain_utils::MockMedianTime;
use ckb_types::{
    constants::HEADER_VERSION,
    core::{EpochExt, HeaderBuilder, HeaderView},
    packed::Header,
    prelude::*,
    U256,
};
use faketime::unix_time_as_millis;
use std::sync::Arc;

fn mock_median_time_context() -> MockMedianTime {
    let now = unix_time_as_millis();
    let timestamps = (0..100).map(|_| now).collect();
    MockMedianTime::new(timestamps)
}

#[test]
pub fn test_version() {
    let header = HeaderBuilder::default()
        .version((HEADER_VERSION + 1).pack())
        .build();
    let verifier = VersionVerifier::new(&header);

    assert_error_eq(
        verifier.verify().err(),
        Some(BlockErrorKind::MismatchedVersion.into()),
    );
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

    assert_error_eq(
        timestamp_verifier.verify().err(),
        Some(
            TimestampError::BlockTimeTooOld {
                min,
                actual: timestamp,
            }
            .into(),
        ),
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
    assert_error_eq(
        timestamp_verifier.verify().err(),
        Some(
            TimestampError::BlockTimeTooNew {
                max,
                actual: timestamp,
            }
            .into(),
        ),
    );
}

#[test]
fn test_number() {
    let parent = HeaderBuilder::default().number(10u64.pack()).build();
    let header = HeaderBuilder::default().number(10u64.pack()).build();

    let verifier = NumberVerifier::new(&parent, &header);
    assert_error_eq(
        verifier.verify().err(),
        Some(
            NumberError {
                expected: 11,
                actual: 10,
            }
            .into(),
        ),
    );
}

struct FakeHeaderResolver {
    header: HeaderView,
    epoch: EpochExt,
}

impl FakeHeaderResolver {
    fn new(header: HeaderView, epoch: EpochExt) -> Self {
        Self { header, epoch }
    }
}

impl HeaderResolver for FakeHeaderResolver {
    fn header(&self) -> &HeaderView {
        &self.header
    }

    fn parent(&self) -> Option<&HeaderView> {
        unimplemented!();
    }

    fn epoch(&self) -> Option<&EpochExt> {
        Some(&self.epoch)
    }
}

#[test]
fn test_epoch_number() {
    let header = HeaderBuilder::default().epoch(2u64.pack()).build();
    let fake_header_resolver = FakeHeaderResolver::new(header, EpochExt::default());

    assert_error_eq(
        EpochVerifier::verify(&fake_header_resolver).err(),
        Some(
            EpochError::UnmatchedNumber {
                expected: 0,
                actual: 2,
            }
            .into(),
        ),
    )
}

#[test]
fn test_epoch_difficulty() {
    let header = HeaderBuilder::default()
        .difficulty(U256::from(2u64).pack())
        .build();
    let mut epoch = EpochExt::default();
    epoch.set_difficulty(U256::from(1u64));
    let fake_header_resolver = FakeHeaderResolver::new(header, epoch);

    assert_error_eq(
        EpochVerifier::verify(&fake_header_resolver).err(),
        Some(
            EpochError::UnmatchedDifficulty {
                expected: U256::from(1u64).pack(),
                actual: U256::from(2u64).pack(),
            }
            .into(),
        ),
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
    let fake_pow_engine: Arc<dyn PowEngine> = Arc::new(FakePowEngine);
    let verifier = PowVerifier::new(&header, &fake_pow_engine);

    assert_error_eq(verifier.verify().err(), Some(PowError::InvalidNonce.into()));
}
