pub use failure::Fail;
pub type Result<T> = ::std::result::Result<T, Error>;

#[derive(Fail, Debug, PartialEq, Eq, Clone)]
pub enum Error {
    #[fail(display = "Get root on an empty MMR")]
    GetRootOnEmpty,
    #[fail(display = "Inconsistent store")]
    InconsistentStore,
    #[fail(display = "Store error {}", _0)]
    StoreError(String),
}
