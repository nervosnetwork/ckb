use bigint::H256;
use core::difficulty::boundary_to_difficulty;
use core::header::Header;
use error::{Error, PowError};
use ethash::{recover_boundary, Ethash, Pow};
use std::sync::Arc;

#[derive(Clone)]
pub struct EthashVerifier {
    inner: Arc<Ethash>,
}

impl EthashVerifier {
    pub fn new(ethash: &Arc<Ethash>) -> Self {
        EthashVerifier {
            inner: Arc::clone(ethash),
        }
    }

    pub fn verify(&self, header: &Header, pow_hash: &H256) -> Result<(), Error> {
        self.cheap_verify(header, pow_hash)
            .and_then(|_| self.heavy_verify(header, pow_hash))
    }

    fn cheap_verify(&self, header: &Header, pow_hash: &H256) -> Result<(), Error> {
        let difficulty = boundary_to_difficulty(&recover_boundary(
            pow_hash,
            header.seal.nonce,
            &header.seal.mix_hash,
        ));

        if difficulty < header.difficulty {
            Err(Error::Pow(PowError::Boundary {
                expected: header.difficulty,
                actual: difficulty,
            }))
        } else {
            Ok(())
        }
    }

    fn heavy_verify(&self, header: &Header, pow_hash: &H256) -> Result<(), Error> {
        let Pow { mix, value } =
            self.inner
                .light_compute(header.number, *pow_hash, header.seal.nonce);
        if mix != header.seal.mix_hash {
            return Err(Error::Pow(PowError::MixMismatch {
                expected: header.seal.mix_hash,
                actual: mix,
            }));
        }
        let difficulty = boundary_to_difficulty(&value);

        if difficulty < header.difficulty {
            return Err(Error::Pow(PowError::Boundary {
                expected: header.difficulty,
                actual: difficulty,
            }));
        }
        Ok(())
    }
}
