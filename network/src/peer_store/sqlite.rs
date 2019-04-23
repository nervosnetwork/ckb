pub(crate) mod db;
#[cfg(db_trace)]
pub mod db_trace;
pub(crate) mod peer_store;

pub use peer_store::SqlitePeerStore;
pub use rusqlite::{Connection, Error as SqliteError};

#[derive(Debug)]
pub enum DBError {
    Sqlite(SqliteError),
}

impl From<SqliteError> for DBError {
    fn from(err: SqliteError) -> Self {
        DBError::Sqlite(err)
    }
}

pub enum StorePath {
    Memory(String),
    File(String),
}
