use std::str::FromStr;

use secrecy::{ExposeSecret, SecretString};
use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions},
};

const OWNER_ROLE: &str = "folioharbor_owner";
const API_ROLE: &str = "folioharbor_api";
const WORKER_ROLE: &str = "folioharbor_worker";

#[derive(Debug)]
pub struct PgPools {
    pub owner: PgPool,
    pub api: PgPool,
    pub worker: PgPool,
}

impl PgPools {
    /// Connects all three role-specific pools for real-PostgreSQL tests.
    ///
    /// # Errors
    ///
    /// Returns a `SQLx` error when a URL is invalid, a connection fails, or the
    /// URL authenticates as a role other than the required role.
    pub async fn connect_for_tests(
        owner_url: &str,
        api_url: &str,
        worker_url: &str,
    ) -> Result<Self, sqlx::Error> {
        Ok(Self {
            owner: connect_with_role(owner_url, OWNER_ROLE, 1).await?,
            api: connect_with_role(api_url, API_ROLE, 1).await?,
            worker: connect_with_role(worker_url, WORKER_ROLE, 1).await?,
        })
    }

    pub async fn close(&self) {
        self.owner.close().await;
        self.api.close().await;
        self.worker.close().await;
    }
}

/// Connects an API-only `PostgreSQL` pool.
///
/// # Errors
///
/// Returns a `SQLx` error if the connection fails or credentials do not select
/// `folioharbor_api`.
pub async fn connect_api(url: &SecretString) -> Result<PgPool, sqlx::Error> {
    connect_with_role(url.expose_secret(), API_ROLE, 10).await
}

/// Connects a worker-only `PostgreSQL` pool.
///
/// # Errors
///
/// Returns a `SQLx` error if the connection fails or credentials do not select
/// `folioharbor_worker`.
pub async fn connect_worker(url: &SecretString) -> Result<PgPool, sqlx::Error> {
    connect_with_role(url.expose_secret(), WORKER_ROLE, 10).await
}

/// Connects the owner pool used exclusively by explicit migration tooling.
///
/// # Errors
///
/// Returns a `SQLx` error if the connection fails or credentials do not select
/// `folioharbor_owner`.
pub async fn connect_owner(url: &SecretString) -> Result<PgPool, sqlx::Error> {
    connect_with_role(url.expose_secret(), OWNER_ROLE, 1).await
}

async fn connect_with_role(
    url: &str,
    role: &str,
    max_connections: u32,
) -> Result<PgPool, sqlx::Error> {
    let options = PgConnectOptions::from_str(url)?;
    let pool = PgPoolOptions::new()
        .max_connections(max_connections)
        .connect_with(options)
        .await?;
    let actual: String = sqlx::query_scalar("SELECT current_user")
        .fetch_one(&pool)
        .await?;
    if actual != role {
        pool.close().await;
        return Err(sqlx::Error::Configuration(
            format!("database URL authenticated as `{actual}`, expected `{role}`").into(),
        ));
    }
    Ok(pool)
}
