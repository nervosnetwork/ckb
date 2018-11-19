use bigint::H256;
use core::difficulty::boundary_to_difficulty;
use core::header::IndexedHeader;
use error::{Error, PowError};
use ethash::{recover_boundary, Ethash, Pow};
use std::sync::Arc;

pub trait PowVerifier: Clone + Send + Sync {
    fn verify(&self, header: &IndexedHeader, pow_hash: &H256) -> Result<(), Error>;
}

impl<T> PowVerifier for Option<T>
where
    T: PowVerifier,
{
    fn verify(&self, header: &IndexedHeader, pow_hash: &H256) -> Result<(), Error> {
        match self {
            Some(ref verifier) => verifier.verify(header, pow_hash),
            None => Ok(()),
        }
    }
}

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

    fn cheap_verify(&self, header: &IndexedHeader, pow_hash: &H256) -> Result<(), Error> {
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

    fn heavy_verify(&self, header: &IndexedHeader, pow_hash: &H256) -> Result<(), Error> {
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

impl PowVerifier for EthashVerifier {
    fn verify(&self, header: &IndexedHeader, pow_hash: &H256) -> Result<(), Error> {
        self.cheap_verify(header, pow_hash)
            .and_then(|_| self.heavy_verify(header, pow_hash))
    }
}

pub struct PowVerifierWrapper<'a, T> {
    header: &'a IndexedHeader,
    verifier_impl: T,
}

impl<'a, T> PowVerifierWrapper<'a, T>
where
    T: PowVerifier,
{
    pub fn new(header: &'a IndexedHeader, verifier_impl: T) -> Self {
        PowVerifierWrapper {
            header,
            verifier_impl,
        }
    }

    pub fn verify(&self) -> Result<(), Error> {
        let pow_hash = self.header.pow_hash();
        self.verifier_impl.verify(self.header, &pow_hash)
    }
}
