use failure::Fail;

#[derive(Debug, PartialEq, Clone, Eq, Fail)]
pub enum Error {
    #[fail(display = "InvalidInput")]
    InvalidInput,
    #[fail(display = "InvalidOutput")]
    InvalidOutput,
    #[fail(display = "InvalidParams {}", _0)]
    InvalidParams(String),
}
