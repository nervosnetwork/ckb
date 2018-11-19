use db::kvdb::Error as DBError;

#[derive(Debug, PartialEq, Clone, Eq)]
pub enum Error {
    InvalidInput,
    InvalidOutput,
    DB(DBError),
}

impl From<DBError> for Error {
    fn from(err: DBError) -> Self {
        Error::DB(err)
    }
}
