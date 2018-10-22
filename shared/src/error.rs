use db::kvdb::Error as DBError;

#[derive(Debug, PartialEq, Clone, Eq)]
pub enum SharedError {
    InvalidInput,
    InvalidOutput,
    DB(DBError),
}

impl From<DBError> for SharedError {
    fn from(err: DBError) -> Self {
        SharedError::DB(err)
    }
}
