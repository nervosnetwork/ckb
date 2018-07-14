use super::pow_verifier::EthashVerifier;
use super::Verifier;
use chain::chain::{ChainClient, SealerType};
use core::difficulty::cal_difficulty;
use core::header::Header;
use error::{DifficultyError, Error, HeightError, TimestampError};
use shared::ALLOWED_FUTURE_BLOCKTIME;
use std::sync::Arc;
use time::now_ms;

pub struct HeaderVerifier<'a, C> {
    pub header: &'a Header,
    pub chain: Arc<C>,
}

impl<'a, C> HeaderVerifier<'a, C>
where
    C: ChainClient,
{
    pub fn new(header: &'a Header, chain: Arc<C>) -> Self {
        HeaderVerifier { header, chain }
    }
}

impl<'a, C> Verifier for HeaderVerifier<'a, C>
where
    C: ChainClient,
{
    fn verify(&self) -> Result<(), Error> {
        match self.chain.block_header(&self.header.hash()) {
            Some(_) => Err(Error::DuplicateHeader),
            None => {
                let header = self.header;
                let parent = self
                    .chain
                    .block_header(&header.parent_hash)
                    .ok_or(Error::UnknownParent)?;

                verify_number(&header, &parent)?;
                verify_timestamp(&header, &parent)?;
                verify_diffculty(&header, &parent)?;
                verify_pow(&header, &self.chain)
            }
        }
    }
}

fn verify_number(header: &Header, parent: &Header) -> Result<(), Error> {
    if header.number != parent.number + 1 {
        return Err(Error::Height(HeightError {
            expected: parent.number + 1,
            actual: header.number,
        }));
    }
    Ok(())
}

fn verify_timestamp(header: &Header, parent: &Header) -> Result<(), Error> {
    let min = parent.timestamp + 1;
    if header.timestamp < min {
        return Err(Error::Timestamp(TimestampError::ZeroBlockTime {
            min,
            found: header.timestamp,
        }));
    }
    let max = now_ms() + ALLOWED_FUTURE_BLOCKTIME;
    if header.timestamp > max {
        return Err(Error::Timestamp(TimestampError::FutureBlockTime {
            max,
            found: header.timestamp,
        }));
    }
    Ok(())
}

fn verify_diffculty(header: &Header, parent: &Header) -> Result<(), Error> {
    let expected_difficulty = cal_difficulty(parent, header.timestamp);
    if expected_difficulty != header.difficulty {
        return Err(Error::Difficulty(DifficultyError {
            expected: expected_difficulty,
            actual: header.difficulty,
        }));
    }
    Ok(())
}

fn verify_pow<C: ChainClient>(header: &Header, chain: &Arc<C>) -> Result<(), Error> {
    match chain.sealer_type() {
        SealerType::Normal => EthashVerifier::new(&chain.ethash().expect("Ethash exists"))
            .verify(header, &header.pow_hash()),
        SealerType::Noop => Ok(()),
    }
}
