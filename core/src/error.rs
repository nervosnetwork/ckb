use crypto::secp::Error as CrypError;
use numext_fixed_hash::{H256, H512};
use numext_fixed_uint::U256;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    InvalidTimestamp(u64, u64),
    InvalidTransactionsRoot(H256, H256),
    InvalidPublicKey(H512),
    InvalidProof,
    InvalidDifficulty(U256, U256),
    InvalidSignature(CrypError),
    InvalidHash(H256, H256),
}

impl From<CrypError> for Error {
    fn from(e: CrypError) -> Self {
        Error::InvalidSignature(e)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum TxError {
    OutOfBound,
    NotMatch,
    EmptyGroup,
    WrongFormat,
}
