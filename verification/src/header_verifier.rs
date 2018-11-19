use super::pow_verifier::{PowVerifier, PowVerifierWrapper};
use super::Verifier;
use core::difficulty::cal_difficulty;
use core::header::IndexedHeader;
use error::{DifficultyError, Error, HeightError, TimestampError};
use shared::ALLOWED_FUTURE_BLOCKTIME;
use time::now_ms;

pub struct HeaderVerifier<'a, P> {
    pub pow: PowVerifierWrapper<'a, P>,
    pub timestamp: TimestampVerifier<'a>,
    pub number: NumberVerifier<'a>,
    pub difficulty: DifficultyVerifier<'a>,
}

impl<'a, P> HeaderVerifier<'a, P>
where
    P: PowVerifier,
{
    pub fn new(parent: &'a IndexedHeader, header: &'a IndexedHeader, pow: P) -> Self {
        debug_assert_eq!(parent.hash(), header.parent_hash);
        HeaderVerifier {
            pow: PowVerifierWrapper::new(header, pow),
            timestamp: TimestampVerifier::new(parent, header),
            number: NumberVerifier::new(parent, header),
            difficulty: DifficultyVerifier::new(parent, header),
        }
    }
}

impl<'a, P> Verifier for HeaderVerifier<'a, P>
where
    P: PowVerifier,
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
    parent: &'a IndexedHeader,
    header: &'a IndexedHeader,
    now: u64,
}

impl<'a> TimestampVerifier<'a> {
    pub fn new(parent: &'a IndexedHeader, header: &'a IndexedHeader) -> Self {
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
    parent: &'a IndexedHeader,
    header: &'a IndexedHeader,
}

impl<'a> NumberVerifier<'a> {
    pub fn new(parent: &'a IndexedHeader, header: &'a IndexedHeader) -> Self {
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
    parent: &'a IndexedHeader,
    header: &'a IndexedHeader,
}

impl<'a> DifficultyVerifier<'a> {
    pub fn new(parent: &'a IndexedHeader, header: &'a IndexedHeader) -> Self {
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
