use anyhow::{anyhow, Result};
use ckb_app_config::{DBDriver, RichIndexerConfig};
use futures::TryStreamExt;
use include_dir::{include_dir, Dir};
use log::LevelFilter;
use once_cell::sync::OnceCell;
use sqlx::{
    any::{Any, AnyArguments, AnyConnectOptions, AnyPoolOptions, AnyRow},
    migrate::Migrator,
    query::{Query, QueryAs},
    AnyPool, ConnectOptions, IntoArguments, Row, Transaction,
};
use tempfile::tempdir;

use std::fs::{self, OpenOptions};
use std::marker::{Send, Unpin};
use std::path::PathBuf;
use std::str::FromStr;
use std::{fmt::Debug, sync::Arc, time::Duration};

const MEMORY_DB: &str = "sqlite://?mode=memory";
const SQL_SQLITE_CREATE_TABLE: &str = include_str!("../resources/create_sqlite_table.sql");
const SQL_SQLITE_CREATE_INDEX: &str = include_str!("../resources/create_sqlite_index.sql");
const SQL_POSTGRES_CREATE_TABLE: &str = include_str!("../resources/create_postgres_table.sql");
const SQL_POSTGRES_CREATE_INDEX: &str = include_str!("../resources/create_postgres_index.sql");
static MIGRATIONS_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/resources/migrations");

#[derive(Clone, Default)]
pub struct SQLXPool {
    pool: Arc<OnceCell<AnyPool>>,
    pub(crate) db_driver: DBDriver,
}

impl Debug for SQLXPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SQLXPool")
            .field("db_driver", &self.db_driver)
            .finish()
    }
}

impl SQLXPool {
    pub async fn connect(&mut self, db_config: &RichIndexerConfig) -> Result<()> {
        // if not init, it will panic, see doc for more
        sqlx::any::install_default_drivers();
        let mut pool_options = AnyPoolOptions::new()
            .max_connections(10)
            .min_connections(0)
            .acquire_timeout(Duration::from_secs(60))
            .max_lifetime(Duration::from_secs(1800))
            .idle_timeout(Duration::from_secs(30));
        if db_config.store == Into::<PathBuf>::into(MEMORY_DB) {
            // See related issue: https://github.com/launchbadge/sqlx/issues/2510
            pool_options = pool_options.max_connections(1);
        }
        let pool = match db_config.db_type {
            DBDriver::Sqlite => {
                let require_init = is_sqlite_require_init(db_config);
                let uri = build_url_for_sqlite(db_config);
                let connection_options =
                    AnyConnectOptions::from_str(&uri)?.log_statements(LevelFilter::Trace);
                let pool = pool_options.connect_with(connection_options).await?;
                log::info!("SQLite is connected.");
                self.pool
                    .set(pool.clone())
                    .map_err(|_| anyhow!("set pool failed!"))?;
                if require_init {
                    self.create_tables_for_sqlite().await?;
                }
                self.db_driver = DBDriver::Sqlite;
                pool
            }
            DBDriver::Postgres => {
                let require_init = self.is_postgres_require_init(db_config).await?;
                let uri = build_url_for_postgres(db_config);
                let connection_options =
                    AnyConnectOptions::from_str(&uri)?.log_statements(LevelFilter::Trace);
                let pool = pool_options.connect_with(connection_options).await?;
                log::info!("PostgreSQL is connected.");
                self.pool
                    .set(pool.clone())
                    .map_err(|_| anyhow!("set pool failed"))?;
                if require_init {
                    self.create_tables_for_postgres().await?;
                }
                self.db_driver = DBDriver::Postgres;
                pool
            }
        };

        // Run migrations
        log::info!("Running migrations...");
        let temp_dir = tempdir()?;
        for file in MIGRATIONS_DIR.files() {
            log::info!("Found migration file: {:?}", file.path());
            let file_path = temp_dir.path().join(file.path());
            if let Some(parent) = file_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&file_path, file.contents())?;
        }
        let migrator = Migrator::new(temp_dir.path()).await?;
        migrator.run(&pool).await?;
        log::info!("Migrations are done.");

        Ok(())
    }

    pub async fn fetch_count(&self, table_name: &str) -> Result<u64> {
        let pool = self.get_pool()?;
        let sql = format!("SELECT COUNT(*) as count FROM {}", table_name);
        let row = sqlx::query(&sql).fetch_one(pool).await?;
        let count: i64 = row.get::<i64, _>("count");
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

    pub async fn transaction(&self) -> Result<Transaction<'_, Any>> {
        let pool = self.get_pool()?;
        pool.begin().await.map_err(Into::into)
    }

    pub fn get_pool(&self) -> Result<&AnyPool> {
        self.pool
            .get()
            .ok_or_else(|| anyhow!("pg pool not inited!"))
    }

    async fn create_tables_for_sqlite(&self) -> Result<()> {
        let mut tx = self.transaction().await?;
        sqlx::query(SQL_SQLITE_CREATE_TABLE)
            .execute(&mut *tx)
            .await?;
        sqlx::query(SQL_SQLITE_CREATE_INDEX)
            .execute(&mut *tx)
            .await?;
        tx.commit().await.map_err(Into::into)
    }

    async fn create_tables_for_postgres(&mut self) -> Result<()> {
        let commands_table = SQL_POSTGRES_CREATE_TABLE.split(';');
        let commands_index = SQL_POSTGRES_CREATE_INDEX.split(';');
        for command in commands_table.chain(commands_index) {
            if !command.trim().is_empty() {
                let pool = self.get_pool()?;
                sqlx::query(command).execute(pool).await?;
            }
        }
        Ok(())
    }

    pub async fn is_postgres_require_init(
        &mut self,
        db_config: &RichIndexerConfig,
    ) -> Result<bool> {
        // Connect to the "postgres" database first
        let mut temp_config = db_config.clone();
        temp_config.db_name = "postgres".to_string();
        let uri = build_url_for_postgres(&temp_config);
        let connection_options =
            AnyConnectOptions::from_str(&uri)?.log_statements(LevelFilter::Trace);
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

fn build_url_for_sqlite(db_config: &RichIndexerConfig) -> String {
    db_config.db_type.to_string() + db_config.store.to_str().expect("get store path")
}

fn build_url_for_postgres(db_config: &RichIndexerConfig) -> String {
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

fn is_sqlite_require_init(db_config: &RichIndexerConfig) -> bool {
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
