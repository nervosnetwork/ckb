#![allow(dead_code)]
use lazy_static::lazy_static;
use r2d2::{Error as PoolError, Pool};
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::OpenFlags;
pub use rusqlite::{Connection, Error as SqliteError};

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

const SHARED_MEMORY_PATH: &str = "file::memory:?cache=shared";

pub enum StorePath {
    Memory,
    File(String),
}

pub fn open_pool(store_path: StorePath, max_size: u32) -> ConnectionPool {
    let manager = match store_path {
        StorePath::Memory => {
            let manager = SqliteConnectionManager::file(SHARED_MEMORY_PATH);
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
        .expect("build connection pool")
}

pub fn open(store_path: StorePath) -> Result<Connection, Error> {
    match store_path {
        StorePath::Memory => {
            Connection::open_with_flags(SHARED_MEMORY_PATH, *MEMORY_OPEN_FLAGS).map_err(Into::into)
        }
        StorePath::File(file_path) => {
            Connection::open_with_flags(file_path, *FILE_OPEN_FLAGS).map_err(Into::into)
        }
    }
}

#[derive(Debug)]
pub enum Error {
    Pool(PoolError),
    Sqlite(SqliteError),
}

impl From<PoolError> for Error {
    fn from(err: PoolError) -> Self {
        Error::Pool(err)
    }
}

impl From<SqliteError> for Error {
    fn from(err: SqliteError) -> Self {
        Error::Sqlite(err)
    }
}

pub trait ConnectionPoolExt {
    fn fetch<I, F: FnMut(&mut PooledConnection) -> Result<I, Error>>(
        &self,
        f: F,
    ) -> Result<I, Error>;
}

impl ConnectionPoolExt for ConnectionPool {
    fn fetch<I, F: FnMut(&mut PooledConnection) -> Result<I, Error>>(
        &self,
        mut f: F,
    ) -> Result<I, Error> {
        let mut connection = self.get()?;
        f(&mut connection)
    }
}
