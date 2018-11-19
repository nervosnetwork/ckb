use super::Verifier;
use bigint::U256;
use core::header::Header;
use error::{DifficultyError, Error, NumberError, PowError, TimestampError};
use pow::PowEngine;
use shared::ALLOWED_FUTURE_BLOCKTIME;
use std::sync::Arc;
use time::now_ms;

pub trait HeaderResolver {
    fn header(&self) -> &Header;
    /// resolves parent header
    fn parent(&self) -> Option<&Header>;
    /// resolves header difficulty
    fn calculate_difficulty(&self) -> Option<U256>;
}

pub struct HeaderVerifier<R> {
    pub resolver: R,
    pub pow: Arc<dyn PowEngine>,
}

impl<R> HeaderVerifier<R>
where
    R: HeaderResolver,
{
    pub fn new(resolver: R, pow: &Arc<dyn PowEngine>) -> Self {
        HeaderVerifier {
            resolver,
            pow: Arc::clone(pow),
        }
    }
}

impl<R> Verifier for HeaderVerifier<R>
where
    R: HeaderResolver,
{
    fn verify(&self) -> Result<(), Error> {
        let header = self.resolver.header();

        // POW check first
        PowVerifier::new(header, &self.pow).verify()?;
        let parent = self
            .resolver
            .parent()
            .ok_or_else(|| Error::UnknownParent(header.parent_hash()))?;
        NumberVerifier::new(parent, header).verify()?;
        TimestampVerifier::new(parent, header).verify()?;
        DifficultyVerifier::new(&self.resolver).verify()?;
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
        let min = self.parent.timestamp() + 1;
        if self.header.timestamp() < min {
            return Err(Error::Timestamp(TimestampError::ZeroBlockTime {
                min,
                found: self.header.timestamp(),
            }));
        }
        let max = self.now + ALLOWED_FUTURE_BLOCKTIME;
        if self.header.timestamp() > max {
            return Err(Error::Timestamp(TimestampError::FutureBlockTime {
                max,
                found: self.header.timestamp(),
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
        if self.header.number() != self.parent.number() + 1 {
            return Err(Error::Number(NumberError {
                expected: self.parent.number() + 1,
                actual: self.header.number(),
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
        let actual = self.resolver.header().difficulty();
        if expected != actual {
            return Err(Error::Difficulty(DifficultyError::MixMismatch {
                expected,
                actual,
            }));
        }
        Ok(())
    }
}

pub struct PowVerifier<'a> {
    header: &'a Header,
    pow: Arc<dyn PowEngine>,
}

impl<'a> PowVerifier<'a> {
    pub fn new(header: &'a Header, pow: &Arc<dyn PowEngine>) -> Self {
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
