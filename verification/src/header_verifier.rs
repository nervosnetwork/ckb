use super::Verifier;
use crate::ALLOWED_FUTURE_BLOCKTIME;
use ckb_core::extras::EpochExt;
use ckb_core::header::{Header, HEADER_VERSION};
use ckb_error::{
    BlockError, EpochError, Error, HeaderError, NumberError, PowError, TimestampError,
};
use ckb_pow::PowEngine;
use ckb_traits::BlockMedianTimeContext;
use faketime::unix_time_as_millis;
use std::marker::PhantomData;
use std::sync::Arc;

pub trait HeaderResolver {
    fn header(&self) -> &Header;
    /// resolves parent header
    fn parent(&self) -> Option<&Header>;
    /// resolves header difficulty
    fn epoch(&self) -> Option<&EpochExt>;
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
            .ok_or_else(|| BlockError::UnknownParent(header.parent_hash().to_owned()))?;
        NumberVerifier::new(parent, header).verify()?;
        TimestampVerifier::new(&self.block_median_time_context, header).verify()?;
        EpochVerifier::verify(target)?;
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
            Err(BlockError::MismatchedVersion)?;
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

        let min = self
            .block_median_time_context
            .block_median_time(self.header.parent_hash());
        if self.header.timestamp() <= min {
            Err(HeaderError::Timestamp(TimestampError::BlockTimeTooOld {
                min,
                actual: self.header.timestamp(),
            }))?;
        }
        let max = self.now + ALLOWED_FUTURE_BLOCKTIME;
        if self.header.timestamp() > max {
            Err(HeaderError::Timestamp(TimestampError::BlockTimeTooNew {
                max,
                actual: self.header.timestamp(),
            }))?;
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
            Err(HeaderError::Number(NumberError {
                expected: self.parent.number() + 1,
                actual: self.header.number(),
            }))?;
        }
        Ok(())
    }
}

pub struct EpochVerifier<T> {
    phantom: PhantomData<T>,
}

impl<T: HeaderResolver> EpochVerifier<T> {
    pub fn verify(target: &T) -> Result<(), Error> {
        let epoch = target
            .epoch()
            .ok_or_else(|| HeaderError::Epoch(EpochError::MissingAncestor))?;
        let actual_epoch_number = target.header().epoch();
        if actual_epoch_number != epoch.number() {
            Err(HeaderError::Epoch(EpochError::UnmatchedNumber {
                expected: epoch.number(),
                actual: actual_epoch_number,
            }))?;
        }
        let actual_difficulty = target.header().difficulty();
        if epoch.difficulty() != actual_difficulty {
            Err(HeaderError::Epoch(EpochError::UnmatchedDifficulty {
                expected: epoch.difficulty().clone(),
                actual: actual_difficulty.clone(),
            }))?;
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
            Err(HeaderError::Pow(PowError::InvalidProof).into())
        }
    }
}
