#![allow(dead_code)]
use lazy_static::lazy_static;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::OpenFlags;
pub use rusqlite::{Connection, Error};

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
    Memory,
    File(String),
}

pub fn open_pool(store_path: StorePath, max_size: u32) -> ConnectionPool {
    let manager = match store_path {
        StorePath::Memory => {
            let manager = SqliteConnectionManager::memory();
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
        StorePath::Memory => Connection::open_in_memory_with_flags(*MEMORY_OPEN_FLAGS),
        StorePath::File(file_path) => Connection::open_with_flags(file_path, *FILE_OPEN_FLAGS),
    }
}
