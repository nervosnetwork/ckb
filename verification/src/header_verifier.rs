use crate::{
    BlockVersionError, EpochError, NumberError, PowError, TimestampError, UnknownParentError,
    ALLOWED_FUTURE_BLOCKTIME,
};
use ckb_chain_spec::consensus::Consensus;
use ckb_error::Error;
use ckb_pow::PowEngine;
use ckb_systemtime::unix_time_as_millis;
use ckb_traits::HeaderProvider;
use ckb_types::core::HeaderView;
use ckb_verification_traits::Verifier;

/// Context-dependent verification checks for block header
///
/// By "context", only mean the previous block headers here.
pub struct HeaderVerifier<'a, DL> {
    data_loader: &'a DL,
    consensus: &'a Consensus,
}

impl<'a, DL: HeaderProvider> HeaderVerifier<'a, DL> {
    /// Crate new HeaderVerifier
    pub fn new(data_loader: &'a DL, consensus: &'a Consensus) -> Self {
        HeaderVerifier {
            consensus,
            data_loader,
        }
    }
}

impl<'a, DL: HeaderProvider> Verifier for HeaderVerifier<'a, DL> {
    type Target = HeaderView;
    fn verify(&self, header: &Self::Target) -> Result<(), Error> {
        VersionVerifier::new(header, self.consensus).verify()?;
        // POW check first
        PowVerifier::new(header, self.consensus.pow_engine().as_ref()).verify()?;
        let parent = self
            .data_loader
            .get_header(&header.parent_hash())
            .ok_or_else(|| UnknownParentError {
                parent_hash: header.parent_hash(),
            })?;
        NumberVerifier::new(&parent, header).verify()?;
        EpochVerifier::new(&parent, header).verify()?;
        TimestampVerifier::new(
            self.data_loader,
            header,
            self.consensus.median_time_block_count(),
        )
        .verify()?;
        Ok(())
    }
}

pub struct VersionVerifier<'a> {
    header: &'a HeaderView,
    consensus: &'a Consensus,
}

impl<'a> VersionVerifier<'a> {
    pub fn new(header: &'a HeaderView, consensus: &'a Consensus) -> Self {
        VersionVerifier { header, consensus }
    }

    pub fn verify(&self) -> Result<(), Error> {
        if !self
            .consensus
            .hardfork_switch
            .ckb2023
            .is_remove_header_version_reservation_rule_enabled(self.header.epoch().number())
            && self.header.version() != self.consensus.block_version()
        {
            return Err(BlockVersionError {
                expected: self.consensus.block_version(),
                actual: self.header.version(),
            }
            .into());
        }
        Ok(())
    }
}

pub struct TimestampVerifier<'a, DL> {
    header: &'a HeaderView,
    data_loader: &'a DL,
    median_block_count: usize,
    now: u64,
}

impl<'a, DL: HeaderProvider> TimestampVerifier<'a, DL> {
    pub fn new(data_loader: &'a DL, header: &'a HeaderView, median_block_count: usize) -> Self {
        TimestampVerifier {
            data_loader,
            header,
            median_block_count,
            now: unix_time_as_millis(),
        }
    }

    pub fn verify(&self) -> Result<(), Error> {
        // skip genesis block
        if self.header.is_genesis() {
            return Ok(());
        }

        let min = self.data_loader.block_median_time(
            &self.header.data().raw().parent_hash(),
            self.median_block_count,
        );
        if self.header.timestamp() <= min {
            return Err(TimestampError::BlockTimeTooOld {
                min,
                actual: self.header.timestamp(),
            }
            .into());
        }
        let max = self.now + ALLOWED_FUTURE_BLOCKTIME;
        if self.header.timestamp() > max {
            return Err(TimestampError::BlockTimeTooNew {
                max,
                actual: self.header.timestamp(),
            }
            .into());
        }
        Ok(())
    }
}

/// Checks if the block number of the given header matches the expected number,
/// which is the parent block's number + 1.
pub struct NumberVerifier<'a> {
    parent: &'a HeaderView,
    header: &'a HeaderView,
}

impl<'a> NumberVerifier<'a> {
    pub fn new(parent: &'a HeaderView, header: &'a HeaderView) -> Self {
        NumberVerifier { parent, header }
    }

    pub fn verify(&self) -> Result<(), Error> {
        if self.header.number() != self.parent.number() + 1 {
            return Err(NumberError {
                expected: self.parent.number() + 1,
                actual: self.header.number(),
            }
            .into());
        }
        Ok(())
    }
}

pub struct EpochVerifier<'a> {
    parent: &'a HeaderView,
    header: &'a HeaderView,
}

impl<'a> EpochVerifier<'a> {
    pub fn new(parent: &'a HeaderView, header: &'a HeaderView) -> Self {
        EpochVerifier { parent, header }
    }

    pub fn verify(&self) -> Result<(), Error> {
        if !self.header.epoch().is_well_formed() {
            return Err(EpochError::Malformed {
                value: self.header.epoch(),
            }
            .into());
        }
        if !self.parent.is_genesis() && !self.header.epoch().is_successor_of(self.parent.epoch()) {
            return Err(EpochError::NonContinuous {
                current: self.header.epoch(),
                parent: self.parent.epoch(),
            }
            .into());
        }
        Ok(())
    }
}

pub struct PowVerifier<'a> {
    header: &'a HeaderView,
    pow: &'a dyn PowEngine,
}

impl<'a> PowVerifier<'a> {
    pub fn new(header: &'a HeaderView, pow: &'a dyn PowEngine) -> Self {
        PowVerifier { header, pow }
    }

    pub fn verify(&self) -> Result<(), Error> {
        if self.pow.verify(&self.header.data()) {
            Ok(())
        } else {
            Err(PowError::InvalidNonce.into())
        }
    }
}
