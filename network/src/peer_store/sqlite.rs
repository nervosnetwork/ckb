#![allow(dead_code)]

pub(crate) mod db;
#[cfg(db_trace)]
pub mod db_trace;
pub(crate) mod peer_store;

use lazy_static::lazy_static;
pub use peer_store::SqlitePeerStore;
pub use r2d2::Error as PoolError;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::OpenFlags;
pub use rusqlite::{Connection, Error as SqliteError};

#[derive(Debug)]
pub enum DBError {
    Pool(PoolError),
    Sqlite(SqliteError),
}

impl From<PoolError> for DBError {
    fn from(err: PoolError) -> Self {
        DBError::Pool(err)
    }
}

impl From<SqliteError> for DBError {
    fn from(err: SqliteError) -> Self {
        DBError::Sqlite(err)
    }
}

pub type ConnectionPool = Pool<SqliteConnectionManager>;
pub type PooledConnection = r2d2::PooledConnection<SqliteConnectionManager>;

lazy_static! {
    static ref MEMORY_OPEN_FLAGS: OpenFlags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_CREATE
        | OpenFlags::SQLITE_OPEN_SHARED_CACHE
        | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    static ref FILE_OPEN_FLAGS: OpenFlags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_CREATE
        | OpenFlags::SQLITE_OPEN_NO_MUTEX;
}

pub enum StorePath {
    Memory(String),
    File(String),
}

pub fn open_pool(store_path: StorePath, max_size: u32) -> Result<ConnectionPool, DBError> {
    let manager = match store_path {
        StorePath::Memory(db) => {
            let manager =
                SqliteConnectionManager::file(format!("file:{}?mode=memory&cache=shared", db));
            manager.with_flags(*MEMORY_OPEN_FLAGS)
        }
        StorePath::File(file_path) => {
            let manager = SqliteConnectionManager::file(file_path);
            manager.with_flags(*FILE_OPEN_FLAGS)
        }
    };
    Pool::builder()
        .max_size(max_size)
        .build(manager)
        .map_err(Into::into)
}

pub fn open(store_path: StorePath) -> Result<Connection, DBError> {
    match store_path {
        StorePath::Memory(db) => Connection::open_with_flags(
            format!("file:{}?mode=memory&cache=shared", db),
            *MEMORY_OPEN_FLAGS,
        )
        .map_err(Into::into),
        StorePath::File(file_path) => {
            Connection::open_with_flags(file_path, *FILE_OPEN_FLAGS).map_err(Into::into)
        }
    }
}

pub trait ConnectionPoolExt {
    fn fetch<I, F: FnOnce(&mut PooledConnection) -> Result<I, DBError>>(
        &self,
        f: F,
    ) -> Result<I, DBError>;
}

impl ConnectionPoolExt for ConnectionPool {
    fn fetch<I, F: FnOnce(&mut PooledConnection) -> Result<I, DBError>>(
        &self,
        f: F,
    ) -> Result<I, DBError> {
        let mut connection = self.get()?;
        f(&mut connection)
    }
}
