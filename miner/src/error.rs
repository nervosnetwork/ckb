use ckb_error::prelude::*;

/// TODO(doc): @quake
#[derive(Error, Debug, PartialEq, Clone, Eq)]
pub enum Error {
    /// TODO(doc): @quake
    #[error("InvalidInput")]
    InvalidInput,
    /// TODO(doc): @quake
    #[error("InvalidOutput")]
    InvalidOutput,
    /// TODO(doc): @quake
    #[error("InvalidParams {0}")]
    InvalidParams(String),
}
