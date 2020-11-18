use failure::Fail;

/// TODO(doc): @quake
#[derive(Debug, PartialEq, Clone, Eq, Fail)]
pub enum Error {
    /// TODO(doc): @quake
    #[fail(display = "InvalidInput")]
    InvalidInput,
    /// TODO(doc): @quake
    #[fail(display = "InvalidOutput")]
    InvalidOutput,
    /// TODO(doc): @quake
    #[fail(display = "InvalidParams {}", _0)]
    InvalidParams(String),
}
