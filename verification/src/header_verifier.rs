use super::pow_verifier::{PowVerifier, PowVerifierWrapper};
use super::Verifier;
use core::difficulty::cal_difficulty;
use core::header::Header;
use error::{DifficultyError, Error, HeightError, TimestampError};
use shared::ALLOWED_FUTURE_BLOCKTIME;
use time::now_ms;

pub struct HeaderVerifier<'a, T> {
    pub pow: PowVerifierWrapper<'a, T>,
    pub timestamp: TimestampVerifier<'a>,
    pub number: NumberVerifier<'a>,
    pub difficulty: DifficultyVerifier<'a>,
}

impl<'a, T> HeaderVerifier<'a, T>
where
    T: PowVerifier,
{
    pub fn new(parent: &'a Header, header: &'a Header, pow_verifier: T) -> Self {
        debug_assert_eq!(parent.hash(), header.parent_hash);
        HeaderVerifier {
            pow: PowVerifierWrapper::new(header, pow_verifier),
            timestamp: TimestampVerifier::new(parent, header),
            number: NumberVerifier::new(parent, header),
            difficulty: DifficultyVerifier::new(parent, header),
        }
    }
}

impl<'a, T> Verifier for HeaderVerifier<'a, T>
where
    T: PowVerifier,
{
    fn verify(&self) -> Result<(), Error> {
        self.number.verify()?;
        self.timestamp.verify()?;
        self.difficulty.verify()?;
        self.pow.verify()?;
        Ok(())
    }
}

pub struct TimestampVerifier<'a> {
    parent: &'a Header,
    header: &'a Header,
    now: u64,
}

impl<'a> TimestampVerifier<'a> {
    pub fn new(parent: &'a Header, header: &'a Header) -> Self {
        TimestampVerifier {
            parent,
            header,
            now: now_ms(),
        }
    }

    pub fn verify(&self) -> Result<(), Error> {
        let min = self.parent.timestamp + 1;
        if self.header.timestamp < min {
            return Err(Error::Timestamp(TimestampError::ZeroBlockTime {
                min,
                found: self.header.timestamp,
            }));
        }
        let max = self.now + ALLOWED_FUTURE_BLOCKTIME;
        if self.header.timestamp > max {
            return Err(Error::Timestamp(TimestampError::FutureBlockTime {
                max,
                found: self.header.timestamp,
            }));
        }
        Ok(())
    }
}

pub struct NumberVerifier<'a> {
    parent: &'a Header,
    header: &'a Header,
}

impl<'a> NumberVerifier<'a> {
    pub fn new(parent: &'a Header, header: &'a Header) -> Self {
        NumberVerifier { parent, header }
    }

    pub fn verify(&self) -> Result<(), Error> {
        if self.header.number != self.parent.number + 1 {
            return Err(Error::Height(HeightError {
                expected: self.parent.number + 1,
                actual: self.header.number,
            }));
        }
        Ok(())
    }
}

pub struct DifficultyVerifier<'a> {
    parent: &'a Header,
    header: &'a Header,
}

impl<'a> DifficultyVerifier<'a> {
    pub fn new(parent: &'a Header, header: &'a Header) -> Self {
        DifficultyVerifier { parent, header }
    }

    pub fn verify(&self) -> Result<(), Error> {
        let expected_difficulty = cal_difficulty(self.parent, self.header.timestamp);
        if expected_difficulty != self.header.difficulty {
            return Err(Error::Difficulty(DifficultyError {
                expected: expected_difficulty,
                actual: self.header.difficulty,
            }));
        }
        Ok(())
    }
}
