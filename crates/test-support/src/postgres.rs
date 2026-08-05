use std::env;

use sqlx::{PgPool, postgres::PgPoolOptions};
use thiserror::Error;
use ulid::Ulid;
use url::Url;

const ADMIN_URL_ENV: &str = "FOLIOHARBOR_TEST_DATABASE_URL";
const ROLES_SQL: &str = include_str!("../../../deploy/postgres/init/001-roles.sql");

#[derive(Debug)]
pub struct TestPostgres {
    admin_url: String,
    database_name: String,
}

#[derive(Debug, Error)]
pub enum TestPostgresError {
    #[error("missing PostgreSQL test configuration")]
    Environment(#[from] env::VarError),
    #[error("PostgreSQL test operation failed")]
    Database(#[from] sqlx::Error),
    #[error("refusing to operate on a database outside the test namespace")]
    UnsafeDatabaseName,
}

impl TestPostgres {
    /// Creates an isolated database owned by `folioharbor_owner`.
    ///
    /// The admin URL must target a disposable `PostgreSQL` 18 test cluster.
    ///
    /// # Errors
    ///
    /// Returns an error when the environment is absent, the URL is invalid,
    /// role bootstrap fails, or the unique database cannot be created.
    pub async fn provision() -> Result<Self, TestPostgresError> {
        let admin_url = env::var(ADMIN_URL_ENV)?;
        let database_name = format!(
            "folioharbor_test_{}",
            Ulid::new().to_string().to_lowercase()
        );
        validate_database_name(&database_name)?;
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect(&admin_url)
            .await?;
        sqlx::raw_sql(ROLES_SQL).execute(&admin).await?;
        sqlx::raw_sql(&format!(
            "CREATE DATABASE \"{database_name}\" OWNER folioharbor_owner"
        ))
        .execute(&admin)
        .await?;
        admin.close().await;
        Ok(Self {
            admin_url,
            database_name,
        })
    }

    /// Builds the owner-role URL for this test database.
    ///
    /// # Errors
    ///
    /// Returns an error when the configured admin URL is invalid.
    pub fn owner_url(&self) -> Result<String, url::ParseError> {
        self.role_url("folioharbor_owner")
    }

    /// Builds the API-role URL for this test database.
    ///
    /// # Errors
    ///
    /// Returns an error when the configured admin URL is invalid.
    pub fn api_url(&self) -> Result<String, url::ParseError> {
        self.role_url("folioharbor_api")
    }

    /// Builds the worker-role URL for this test database.
    ///
    /// # Errors
    ///
    /// Returns an error when the configured admin URL is invalid.
    pub fn worker_url(&self) -> Result<String, url::ParseError> {
        self.role_url("folioharbor_worker")
    }

    /// Drops only the uniquely named database created by [`Self::provision`].
    ///
    /// # Errors
    ///
    /// Returns an error if validation, connection, or database removal fails.
    pub async fn cleanup(self) -> Result<(), TestPostgresError> {
        validate_database_name(&self.database_name)?;
        let admin = PgPool::connect(&self.admin_url).await?;
        sqlx::raw_sql(&format!(
            "DROP DATABASE \"{}\" WITH (FORCE)",
            self.database_name
        ))
        .execute(&admin)
        .await?;
        admin.close().await;
        Ok(())
    }

    fn role_url(&self, role: &str) -> Result<String, url::ParseError> {
        let mut url = Url::parse(&self.admin_url)?;
        let _ = url.set_username(role);
        let _ = url.set_password(None);
        url.set_path(&format!("/{}", self.database_name));
        Ok(url.into())
    }
}

fn validate_database_name(name: &str) -> Result<(), TestPostgresError> {
    if name.starts_with("folioharbor_test_")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        Ok(())
    } else {
        Err(TestPostgresError::UnsafeDatabaseName)
    }
}
