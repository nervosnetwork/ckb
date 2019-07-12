use crate::error::{EpochError, Error, NumberError, PowError, TimestampError};
use crate::header_verifier::{
    EpochVerifier, HeaderResolver, NumberVerifier, PowVerifier, TimestampVerifier, VersionVerifier,
};
use crate::ALLOWED_FUTURE_BLOCKTIME;
use ckb_core::extras::EpochExt;
use ckb_core::header::{BlockNumber, Header, HeaderBuilder, HEADER_VERSION};
use ckb_pow::PowEngine;
use ckb_test_chain_utils::MockMedianTime;
use faketime::unix_time_as_millis;
use numext_fixed_uint::U256;
use std::sync::Arc;

fn mock_median_time_context() -> MockMedianTime {
    let now = unix_time_as_millis();
    let timestamps = (0..100).map(|_| now).collect();
    MockMedianTime::new(timestamps)
}

#[test]
pub fn test_version() {
    let header = HeaderBuilder::default().version(HEADER_VERSION + 1).build();
    let verifier = VersionVerifier::new(&header);

    assert_eq!(verifier.verify().err(), Some(Error::Version));
}

#[cfg(not(disable_faketime))]
#[test]
fn test_timestamp() {
    let faketime_file = faketime::millis_tempfile(100_000).expect("create faketime file");
    faketime::enable(&faketime_file);
    let fake_block_median_time_context = mock_median_time_context();

    let timestamp = unix_time_as_millis() + 1;
    let header = HeaderBuilder::default()
        .number(10)
        .timestamp(timestamp)
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
        .number(10)
        .timestamp(timestamp)
        .build();
    let timestamp_verifier = TimestampVerifier::new(&fake_block_median_time_context, &header);

    assert_eq!(
        timestamp_verifier.verify().err(),
        Some(Error::Timestamp(TimestampError::BlockTimeTooOld {
            min,
            found: timestamp,
        }))
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
        .number(10)
        .timestamp(timestamp)
        .build();
    let timestamp_verifier = TimestampVerifier::new(&fake_block_median_time_context, &header);
    assert_eq!(
        timestamp_verifier.verify().err(),
        Some(Error::Timestamp(TimestampError::BlockTimeTooNew {
            max,
            found: timestamp,
        }))
    );
}

#[test]
fn test_number() {
    let parent = HeaderBuilder::default().number(10).build();
    let header = HeaderBuilder::default().number(10).build();

    let verifier = NumberVerifier::new(&parent, &header);
    assert_eq!(
        verifier.verify().err(),
        Some(Error::Number(NumberError {
            expected: 11,
            actual: 10,
        }))
    );
}

struct FakeHeaderResolver {
    header: Header,
    epoch: EpochExt,
}

impl FakeHeaderResolver {
    fn new(header: Header, epoch: EpochExt) -> Self {
        Self { header, epoch }
    }
}

impl HeaderResolver for FakeHeaderResolver {
    fn header(&self) -> &Header {
        &self.header
    }

    fn parent(&self) -> Option<&Header> {
        unimplemented!();
    }

    fn epoch(&self) -> Option<&EpochExt> {
        Some(&self.epoch)
    }
}

#[test]
fn test_epoch_number() {
    let header = HeaderBuilder::default().epoch(2).build();
    let fake_header_resolver = FakeHeaderResolver::new(header, EpochExt::default());

    assert_eq!(
        EpochVerifier::verify(&fake_header_resolver).err(),
        Some(Error::Epoch(EpochError::NumberMismatch {
            expected: 0,
            actual: 2,
        }))
    )
}

#[test]
fn test_epoch_difficulty() {
    let header = HeaderBuilder::default()
        .difficulty(U256::from(2u64))
        .build();
    let mut epoch = EpochExt::default();
    epoch.set_difficulty(U256::from(1u64));
    let fake_header_resolver = FakeHeaderResolver::new(header, epoch);

    assert_eq!(
        EpochVerifier::verify(&fake_header_resolver).err(),
        Some(Error::Epoch(EpochError::DifficultyMismatch {
            expected: U256::from(1u64),
            actual: U256::from(2u64),
        }))
    );
}

struct FakePowEngine;

impl PowEngine for FakePowEngine {
    fn verify_header(&self, _header: &Header) -> bool {
        false
    }

    fn verify_proof_difficulty(&self, _proof: &[u8], _difficulty: &U256) -> bool {
        unimplemented!()
    }

    fn verify(&self, _number: BlockNumber, _message: &[u8], _proof: &[u8]) -> bool {
        unimplemented!();
    }

    fn proof_size(&self) -> usize {
        unimplemented!();
    }
}

#[test]
fn test_pow_verifier() {
    let header = HeaderBuilder::default().build();
    let fake_pow_engine: Arc<dyn PowEngine> = Arc::new(FakePowEngine);
    let verifier = PowVerifier::new(&header, &fake_pow_engine);

    assert_eq!(
        verifier.verify().err(),
        Some(Error::Pow(PowError::InvalidProof))
    );
}
