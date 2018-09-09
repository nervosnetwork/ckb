use super::Verifier;
use bigint::U256;
use chain::PowEngine;
use core::header::IndexedHeader;
use error::{DifficultyError, Error, HeightError, PowError, TimestampError};
use shared::ALLOWED_FUTURE_BLOCKTIME;
use std::sync::Arc;
use time::now_ms;

pub trait HeaderResolver {
    fn header(&self) -> &IndexedHeader;
    /// resolves parent header
    fn parent(&self) -> Option<&IndexedHeader>;
    /// resolves header difficulty
    fn calculate_difficulty(&self) -> Option<U256>;
}

pub struct HeaderVerifier<P, R> {
    pub resolver: R,
    pub pow: Arc<P>,
}

impl<P, R> HeaderVerifier<P, R>
where
    P: PowEngine,
    R: HeaderResolver,
{
    pub fn new(resolver: R, pow: &Arc<P>) -> Self {
        HeaderVerifier {
            resolver,
            pow: Arc::clone(pow),
        }
    }
}

impl<P, R> Verifier for HeaderVerifier<P, R>
where
    P: PowEngine,
    R: HeaderResolver,
{
    fn verify(&self) -> Result<(), Error> {
        let header = self.resolver.header();
        let parent = self
            .resolver
            .parent()
            .ok_or_else(|| Error::UnknownParent(header.parent_hash))?;
        NumberVerifier::new(parent, header).verify()?;
        TimestampVerifier::new(parent, header).verify()?;
        DifficultyVerifier::new(&self.resolver).verify()?;
        PowVerifier::new(header, &self.pow).verify()?;
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

pub struct DifficultyVerifier<'a, R: 'a> {
    resolver: &'a R,
}

impl<'a, R> DifficultyVerifier<'a, R>
where
    R: HeaderResolver,
{
    pub fn new(resolver: &'a R) -> Self {
        DifficultyVerifier { resolver }
    }

    pub fn verify(&self) -> Result<(), Error> {
        let expected = self
            .resolver
            .calculate_difficulty()
            .ok_or_else(|| Error::Difficulty(DifficultyError::AncestorNotFound))?;
        let actual = self.resolver.header().difficulty;
        if expected != actual {
            return Err(Error::Difficulty(DifficultyError::MixMismatch {
                expected,
                actual,
            }));
        }
        Ok(())
    }
}

pub struct PowVerifier<'a, P> {
    header: &'a IndexedHeader,
    pow: Arc<P>,
}

impl<'a, P> PowVerifier<'a, P>
where
    P: PowEngine,
{
    pub fn new(header: &'a IndexedHeader, pow: &Arc<P>) -> Self {
        PowVerifier {
            header,
            pow: Arc::clone(pow),
        }
    }

    pub fn verify(&self) -> Result<(), Error> {
        if self.pow.verify_header(self.header) {
            Ok(())
        } else {
            Err(Error::Pow(PowError::InvalidProof))
        }
    }
}
