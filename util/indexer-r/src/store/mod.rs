pub mod page;

use crate::indexer::append_block_header;
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

use std::fs::OpenOptions;
use std::marker::{Send, Unpin};
use std::path::PathBuf;
use std::str::FromStr;
use std::{fmt::Debug, sync::Arc, time::Duration};

const MEMORY_DB: &str = ":memory:";
const SQL_CREATE_SQLITE: &str = include_str!("../../resources/create_sqlite_table.sql");
const SQL_CREATE_POSTGRES: &str = include_str!("../../resources/create_postgres_table.sql");

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
                    self.create_tables_for_sqlite(db_config).await?;
                }
                Ok(())
            }
            DBDriver::Postgres => {
                let require_init = self.is_postgres_require_init(db_config).await?;
                let uri = build_url_for_postgres(db_config);
                log::debug!("postgres uri: {}", uri);
                let mut connection_options = AnyConnectOptions::from_str(&uri)?;
                connection_options.log_statements(LevelFilter::Trace);
                let pool = pool_options.connect_with(connection_options).await?;
                self.pool
                    .set(pool)
                    .map_err(|_| anyhow!("set pool failed"))?;
                if require_init {
                    self.create_tables_for_postgres(db_config).await?;
                }
                Ok(())
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

    async fn create_tables_for_sqlite(&self, config: &IndexerRConfig) -> Result<()> {
        let mut tx = self.transaction().await?;
        sqlx::query(SQL_CREATE_SQLITE).execute(&mut *tx).await?;
        if config.init_tip_hash.is_some() && config.init_tip_number.is_some() {
            append_block_header(
                config.init_tip_hash.clone().unwrap().as_bytes(),
                config.init_tip_number.unwrap() as i64,
                &mut tx,
            )
            .await?;
        }
        tx.commit().await.map_err(Into::into)
    }

    async fn create_tables_for_postgres(&mut self, config: &IndexerRConfig) -> Result<()> {
        let mut tx = self.transaction().await?;
        let commands = SQL_CREATE_POSTGRES.split(';');
        for command in commands {
            if !command.trim().is_empty() {
                sqlx::query(command).execute(&mut *tx).await?;
            }
        }
        if config.init_tip_hash.is_some() && config.init_tip_number.is_some() {
            append_block_header(
                config.init_tip_hash.clone().unwrap().as_bytes(),
                config.init_tip_number.unwrap() as i64,
                &mut tx,
            )
            .await?;
        }
        tx.commit().await.map_err(Into::into)
    }

    pub async fn is_postgres_require_init(&mut self, db_config: &IndexerRConfig) -> Result<bool> {
        // Connect to the "postgres" database first
        let mut temp_config = db_config.clone();
        temp_config.db_name = "postgres".to_string();
        let uri = build_url_for_postgres(&temp_config);
        log::info!("postgres uri: {}", uri);
        let mut connection_options = AnyConnectOptions::from_str(&uri)?;
        connection_options.log_statements(LevelFilter::Trace);
        let tmp_pool_options = AnyPoolOptions::new();
        let pool = tmp_pool_options.connect_with(connection_options).await?;

        // Check if database exists
        let query =
            SQLXPool::new_query(r#"SELECT EXISTS (SELECT FROM pg_database WHERE datname = $1)"#)
                .bind(db_config.db_name.as_str());
        let row = query.fetch_one(&pool).await?;

        // If database does not exist, create it
        if !row.get::<bool, _>(0) {
            let query = format!(r#"CREATE DATABASE "{}""#, db_config.db_name);
            SQLXPool::new_query(&query).execute(&pool).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

pub(crate) fn fetch_count_sql(table_name: &str) -> String {
    format!("SELECT COUNT(*) as {} FROM {}", COUNT_COLUMN, table_name)
}

fn build_url_for_sqlite(db_config: &IndexerRConfig) -> String {
    db_config.db_type.to_string() + db_config.store.to_str().expect("get store path")
}

fn build_url_for_postgres(db_config: &IndexerRConfig) -> String {
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
