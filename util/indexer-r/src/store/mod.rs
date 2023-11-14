pub mod page;

use page::COUNT_COLUMN;
pub use page::{build_next_cursor, PaginationRequest, PaginationResponse};

use anyhow::{anyhow, Result};
use ckb_app_config::{DBDriver, IndexerRConfig};
use futures::TryStreamExt;
use log::LevelFilter;
use once_cell::sync::OnceCell;
use sql_builder::SqlBuilder;
use sqlx::any::{Any, AnyArguments, AnyConnectOptions, AnyPool, AnyPoolOptions, AnyRow};
use sqlx::query::{Query, QueryAs};
use sqlx::{ConnectOptions, IntoArguments, Row, Transaction};

use std::fs;
use std::fs::OpenOptions;
use std::marker::{Send, Unpin};
use std::path::PathBuf;
use std::str::FromStr;
use std::{fmt::Debug, sync::Arc, time::Duration};

const MEMORY_DB: &str = ":memory:";
const RESOURCE_CREATE_SQLITE: &str = "./resources/create_sqlite_table.sql";

#[derive(Clone)]
pub struct SQLXPool {
    pool: Arc<OnceCell<AnyPool>>,
    driver: OnceCell<DBDriver>,
    max_conn: u32,
    min_conn: u32,
    conn_timeout: Duration,
    max_lifetime: Duration,
    idle_timeout: Duration,
}

impl Debug for SQLXPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SQLXPool")
            .field("max_conn", &self.max_conn)
            .field("min_conn", &self.min_conn)
            .field("conn_timeout", &self.conn_timeout)
            .field("max_lifetime", &self.max_lifetime)
            .field("idle_timeout", &self.idle_timeout)
            .finish()
    }
}

impl SQLXPool {
    pub fn new(
        max_connections: u32,
        min_connections: u32,
        connection_timeout: u64,
        max_lifetime: u64,
        idle_timeout: u64,
    ) -> Self {
        SQLXPool {
            pool: Arc::new(OnceCell::new()),
            driver: OnceCell::new(),
            max_conn: max_connections,
            min_conn: min_connections,
            conn_timeout: Duration::from_secs(connection_timeout),
            max_lifetime: Duration::from_secs(max_lifetime),
            idle_timeout: Duration::from_secs(idle_timeout),
        }
    }

    pub fn default() -> Self {
        SQLXPool::new(10, 0, 60, 1800, 30)
    }

    pub async fn connect(&mut self, db_config: &IndexerRConfig) -> Result<()> {
        self.driver
            .set(db_config.db_type.clone())
            .map_err(|_| anyhow!("set db driver failed!"))?;
        let pool_options = AnyPoolOptions::new()
            .max_connections(self.max_conn)
            .min_connections(self.min_conn)
            .acquire_timeout(self.conn_timeout)
            .max_lifetime(self.max_lifetime)
            .idle_timeout(self.idle_timeout);
        match db_config.db_type {
            DBDriver::Sqlite => {
                let require_init = is_sqlite_require_init(db_config);
                let uri = build_url_for_sqlite(db_config);
                log::debug!("sqlite uri: {}", uri);
                let mut connection_options = AnyConnectOptions::from_str(&uri)?;
                connection_options.log_statements(LevelFilter::Trace);
                let pool = pool_options.connect_with(connection_options).await?;
                self.pool
                    .set(pool)
                    .map_err(|_| anyhow!("set pool failed!"))?;
                if require_init {
                    self.create_tables_for_sqlite().await?;
                }
                Ok(())
            }
            DBDriver::Postgres => {
                let uri = build_url_for_posgres(db_config);
                let mut connection_options = AnyConnectOptions::from_str(&uri)?;
                connection_options.log_statements(LevelFilter::Trace);
                let pool = pool_options.connect_with(connection_options).await?;
                self.pool.set(pool).map_err(|_| anyhow!("set pool failed"))
            }
        }
    }

    pub async fn fetch_count(&self, table_name: &str) -> Result<u64> {
        let pool = self.get_pool()?;
        let row = sqlx::query(&fetch_count_sql(table_name))
            .fetch_one(pool)
            .await?;
        let count: i64 = row.get::<i64, _>(COUNT_COLUMN);
        Ok(count.try_into().expect("i64 to u64"))
    }

    pub fn new_query(sql: &str) -> Query<Any, AnyArguments> {
        sqlx::query(sql)
    }

    pub fn new_query_as<T>(sql: &str) -> QueryAs<Any, T, AnyArguments>
    where
        T: for<'r> sqlx::FromRow<'r, AnyRow>,
    {
        sqlx::query_as(sql)
    }

    pub async fn fetch_optional<'a, T>(&self, query: Query<'a, Any, T>) -> Result<Option<AnyRow>>
    where
        T: Send + IntoArguments<'a, Any> + 'a,
    {
        let pool = self.get_pool()?;
        query.fetch_optional(pool).await.map_err(Into::into)
    }

    pub async fn fetch_one<'a, T>(&self, query: Query<'a, Any, T>) -> Result<AnyRow>
    where
        T: Send + IntoArguments<'a, Any> + 'a,
    {
        let pool = self.get_pool()?;
        query.fetch_one(pool).await.map_err(Into::into)
    }

    pub async fn fetch_all<'a, T>(&self, query: Query<'a, Any, T>) -> Result<Vec<AnyRow>>
    where
        T: Send + IntoArguments<'a, Any> + 'a,
    {
        let pool = self.get_pool()?;
        query.fetch_all(pool).await.map_err(Into::into)
    }

    pub async fn fetch<'a, T>(&self, query: Query<'a, Any, T>) -> Result<Vec<AnyRow>>
    where
        T: Send + IntoArguments<'a, Any> + 'a,
    {
        let pool = self.get_pool()?;
        let mut res = vec![];
        let mut rows = query.fetch(pool);
        while let Some(row) = rows.try_next().await? {
            res.push(row)
        }
        Ok(res)
    }

    pub async fn fetch_one_by_query_as<T>(
        &self,
        query: QueryAs<'static, Any, T, AnyArguments<'static>>,
    ) -> Result<T>
    where
        T: for<'r> sqlx::FromRow<'r, AnyRow> + Unpin + Send,
    {
        let pool = self.get_pool()?;
        query.fetch_one(pool).await.map_err(Into::into)
    }

    pub async fn fetch_page<'a>(
        &self,
        query: Query<'a, Any, AnyArguments<'a>>,
        query_total: Query<'a, Any, AnyArguments<'a>>,
        pagination: &PaginationRequest,
    ) -> Result<PaginationResponse<AnyRow>> {
        let response = self.fetch(query).await?;
        let count = if pagination.return_count {
            let count = self
                .fetch_one(query_total)
                .await?
                .get::<i64, _>(COUNT_COLUMN) as u64;
            Some(count)
        } else {
            None
        };

        let next_cursor = if response.is_empty() {
            None
        } else {
            build_next_cursor(
                pagination.limit.unwrap_or(u16::MAX),
                response.last().unwrap().get::<i64, _>("id") as u64,
                response.len(),
                count,
            )
        };

        Ok(PaginationResponse {
            response,
            next_cursor,
            count,
        })
    }

    pub async fn transaction(&self) -> Result<Transaction<'_, Any>> {
        let pool = self.get_pool()?;
        pool.begin().await.map_err(Into::into)
    }

    pub fn get_pool(&self) -> Result<&AnyPool> {
        self.pool
            .get()
            .ok_or_else(|| anyhow!("pg pool not inited!"))
    }

    pub fn get_max_connections(&self) -> u32 {
        self.max_conn
    }

    async fn create_tables_for_sqlite(&self) -> Result<()> {
        let sql = fs::read_to_string(RESOURCE_CREATE_SQLITE)?;
        let mut tx = self.transaction().await?;
        sqlx::query(&sql).execute(&mut *tx).await?;
        tx.commit().await.map_err(Into::into)
    }
}

pub(crate) fn fetch_count_sql(table_name: &str) -> String {
    format!("SELECT COUNT(*) as {} FROM {}", COUNT_COLUMN, table_name)
}

fn build_url_for_sqlite(db_config: &IndexerRConfig) -> String {
    db_config.db_type.to_string() + db_config.store.to_str().expect("get store path")
}

fn build_url_for_posgres(db_config: &IndexerRConfig) -> String {
    db_config.db_type.to_string()
        + db_config.db_user.as_str()
        + ":"
        + db_config.db_password.as_str()
        + "@"
        + db_config.db_host.as_str()
        + ":"
        + db_config.db_port.to_string().as_str()
        + "/"
        + db_config.db_name.as_str()
}

fn is_sqlite_require_init(db_config: &IndexerRConfig) -> bool {
    // for test
    if db_config.store == Into::<PathBuf>::into(MEMORY_DB) {
        return true;
    }

    if !db_config.store.exists() {
        if let Some(parent) = db_config.store.parent() {
            std::fs::create_dir_all(parent).expect("Create db directory");
        }
        OpenOptions::new()
            .write(true)
            .create(true)
            .open(&db_config.store)
            .expect("Create db file");
        return true;
    }

    false
}
