use super::Verifier;
use ckb_core::header::Header;
use ckb_pow::PowEngine;
use ckb_time::now_ms;
use error::{DifficultyError, Error, NumberError, PowError, TimestampError};
use numext_fixed_uint::U256;
use shared::ALLOWED_FUTURE_BLOCKTIME;
use std::marker::PhantomData;
use std::sync::Arc;

pub trait HeaderResolver {
    fn header(&self) -> &Header;
    /// resolves parent header
    fn parent(&self) -> Option<&Header>;
    /// resolves header difficulty
    fn calculate_difficulty(&self) -> Option<U256>;
}

pub struct HeaderVerifier<T> {
    pub pow: Arc<dyn PowEngine>,
    _phantom: PhantomData<T>,
}

impl<T> HeaderVerifier<T> {
    pub fn new(pow: Arc<dyn PowEngine>) -> Self {
        HeaderVerifier {
            pow,
            _phantom: PhantomData,
        }
    }
}

impl<T: HeaderResolver> Verifier for HeaderVerifier<T> {
    type Target = T;
    fn verify(&self, target: &T) -> Result<(), Error> {
        let header = target.header();

        // POW check first
        PowVerifier::new(header, &self.pow).verify()?;
        let parent = target
            .parent()
            .ok_or_else(|| Error::UnknownParent(header.parent_hash().clone()))?;
        NumberVerifier::new(parent, header).verify()?;
        TimestampVerifier::new(parent, header).verify()?;
        DifficultyVerifier::verify(target)?;
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

pub struct DifficultyVerifier<T> {
    phantom: PhantomData<T>,
}

impl<T: HeaderResolver> DifficultyVerifier<T> {
    pub fn verify(resolver: &T) -> Result<(), Error> {
        let expected = resolver
            .calculate_difficulty()
            .ok_or_else(|| Error::Difficulty(DifficultyError::AncestorNotFound))?;
        let actual = resolver.header().difficulty();
        if &expected != actual {
            return Err(Error::Difficulty(DifficultyError::MixMismatch {
                expected,
                actual: actual.clone(),
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
