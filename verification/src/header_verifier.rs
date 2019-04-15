use super::Verifier;
use crate::error::{DifficultyError, Error, NumberError, PowError, TimestampError};
use crate::shared::ALLOWED_FUTURE_BLOCKTIME;
use ckb_core::header::{Header, HEADER_VERSION};
use ckb_pow::PowEngine;
use ckb_traits::BlockMedianTimeContext;
use faketime::unix_time_as_millis;
use numext_fixed_uint::U256;
use std::marker::PhantomData;
use std::sync::Arc;

pub trait HeaderResolver {
    fn header(&self) -> &Header;
    /// resolves parent header
    fn parent(&self) -> Option<&Header>;
    /// resolves header difficulty
    fn calculate_difficulty(&self) -> Option<U256>;
}

pub struct HeaderVerifier<T, M> {
    pub pow: Arc<dyn PowEngine>,
    block_median_time_context: M,
    _phantom: PhantomData<T>,
}

impl<T, M: BlockMedianTimeContext> HeaderVerifier<T, M> {
    pub fn new(block_median_time_context: M, pow: Arc<dyn PowEngine>) -> Self {
        HeaderVerifier {
            pow,
            block_median_time_context,
            _phantom: PhantomData,
        }
    }
}

impl<T: HeaderResolver, M: BlockMedianTimeContext> Verifier for HeaderVerifier<T, M> {
    type Target = T;
    fn verify(&self, target: &T) -> Result<(), Error> {
        let header = target.header();
        VersionVerifier::new(header).verify()?;
        // POW check first
        PowVerifier::new(header, &self.pow).verify()?;
        let parent = target
            .parent()
            .ok_or_else(|| Error::UnknownParent(header.parent_hash().clone()))?;
        NumberVerifier::new(parent, header).verify()?;
        TimestampVerifier::new(&self.block_median_time_context, header).verify()?;
        DifficultyVerifier::verify(target)?;
        Ok(())
    }
}

pub struct VersionVerifier<'a> {
    header: &'a Header,
}

impl<'a> VersionVerifier<'a> {
    pub fn new(header: &'a Header) -> Self {
        VersionVerifier { header }
    }

    pub fn verify(&self) -> Result<(), Error> {
        if self.header.version() != HEADER_VERSION {
            return Err(Error::Version);
        }
        Ok(())
    }
}

pub struct TimestampVerifier<'a, M> {
    header: &'a Header,
    block_median_time_context: &'a M,
    now: u64,
}

impl<'a, M: BlockMedianTimeContext> TimestampVerifier<'a, M> {
    pub fn new(block_median_time_context: &'a M, header: &'a Header) -> Self {
        TimestampVerifier {
            block_median_time_context,
            header,
            now: unix_time_as_millis(),
        }
    }

    pub fn verify(&self) -> Result<(), Error> {
        // skip genesis block
        if self.header.is_genesis() {
            return Ok(());
        }
        let min = match self
            .header
            .number()
            .checked_sub(1)
            .and_then(|n| self.block_median_time_context.block_median_time(n))
        {
            Some(time) => time,
            None => return Err(Error::UnknownParent(self.header.parent_hash().clone())),
        };
        if self.header.timestamp() <= min {
            return Err(Error::Timestamp(TimestampError::BlockTimeTooOld {
                min,
                found: self.header.timestamp(),
            }));
        }
        let max = self.now + ALLOWED_FUTURE_BLOCKTIME;
        if self.header.timestamp() > max {
            return Err(Error::Timestamp(TimestampError::BlockTimeTooNew {
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
