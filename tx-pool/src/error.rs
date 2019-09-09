use failure::Fail;

#[derive(Debug, PartialEq, Clone, Eq, Fail)]
pub enum BlockAssemblerError {
    #[fail(display = "InvalidInput")]
    InvalidInput,
    #[fail(display = "InvalidParams {}", _0)]
    InvalidParams(String),
    #[fail(display = "Disabled")]
    Disabled,
}
