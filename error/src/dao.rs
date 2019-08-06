use failure::Fail;

// TODO Merge DaoError into HeaderError?
#[derive(Fail, Debug, PartialEq, Clone, Eq)]
pub enum DaoError {
    #[fail(display = "Invalid Header")]
    InvalidHeader,
    #[fail(display = "Invalid OutPoint")]
    InvalidOutPoint,
    #[fail(display = "Invalid Dao format")]
    InvalidDaoFormat,
}
