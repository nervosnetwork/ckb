use failure::Fail;

#[derive(Debug, Fail, PartialEq)]
pub enum Error {
    #[fail(display = "deserialize data from database should be ok")]
    Deserialize,
}

pub type Result<T> = ::std::result::Result<T, Error>;
