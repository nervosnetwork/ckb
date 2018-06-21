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

// pub struct PowVerifier<'a> {
//     header: &'a Header,
//     ethash: Arc<Ethash>,
// }

// impl<'a> PowVerifier<'a> {
//     pub fn new(header: &'a Header, ethash: Arc<Ethash>) -> Self {
//         PowVerifier { header, ethash }
//     }

//     pub fn verify(&self) -> Result<(), Error> {
//         let pow_hash = self.header.pow_hash();
//         self.cheap_verify(&pow_hash)
//             .and_then(|_| self.heavy_verify(&pow_hash))
//     }

//     fn cheap_verify(&self, pow_hash: &H256) -> Result<(), Error> {
//         let difficulty = boundary_to_difficulty(&recover_boundary(
//             pow_hash,
//             self.header.seal.nonce,
//             &self.header.seal.mix_hash,
//         ));

//         if difficulty < self.header.difficulty {
//             Err(Error::Pow(PowError::Boundary {
//                 expected: self.header.difficulty,
//                 actual: difficulty,
//             }))
//         } else {
//             Ok(())
//         }
//     }

//     fn heavy_verify(&self, pow_hash: &H256) -> Result<(), Error> {
//         let Pow { mix, value } =
//             self.ethash
//                 .light_compute(self.header.number, *pow_hash, self.header.seal.nonce);
//         if mix != self.header.seal.mix_hash {
//             return Err(Error::Pow(PowError::MixMismatch {
//                 expected: self.header.seal.mix_hash,
//                 actual: mix,
//             }));
//         }
//         let difficulty = boundary_to_difficulty(&value);

//         if difficulty < self.header.difficulty {
//             return Err(Error::Pow(PowError::Boundary {
//                 expected: self.header.difficulty,
//                 actual: difficulty,
//             }));
//         }
//         Ok(())
//     }
// }

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
