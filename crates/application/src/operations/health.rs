use std::sync::Arc;

use async_trait::async_trait;

use crate::ports::BlobStore;

pub const COMPATIBLE_SCHEMA_VERSION: i64 = 27;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HealthStatus {
    Ready,
    BootstrapRequired,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegistrationGate {
    Available,
    BootstrapRequired,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DatabaseHealth {
    pub schema_version: i64,
    pub system_administrator_exists: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("operations health dependency failed")]
pub struct HealthRepositoryError;

#[async_trait]
pub trait HealthRepository: Send + Sync {
    async fn database_health(&self) -> Result<DatabaseHealth, HealthRepositoryError>;
}

#[async_trait]
pub trait OperationsApi: Send + Sync {
    async fn readiness(&self) -> HealthStatus;
    async fn registration_gate(&self) -> RegistrationGate;
}

pub struct HealthService {
    database: Arc<dyn HealthRepository>,
    storage: Arc<dyn BlobStore>,
    free_reserve_bytes: u64,
    required_configuration_ready: bool,
}

impl HealthService {
    #[must_use]
    pub const fn new(
        database: Arc<dyn HealthRepository>,
        storage: Arc<dyn BlobStore>,
        free_reserve_bytes: u64,
        required_configuration_ready: bool,
    ) -> Self {
        Self {
            database,
            storage,
            free_reserve_bytes,
            required_configuration_ready,
        }
    }
}

#[async_trait]
impl OperationsApi for HealthService {
    async fn readiness(&self) -> HealthStatus {
        let (database, free_bytes) =
            tokio::join!(self.database.database_health(), self.storage.free_bytes());
        let Ok(database) = database else {
            return HealthStatus::Unavailable;
        };
        if database.schema_version != COMPATIBLE_SCHEMA_VERSION {
            return HealthStatus::Unavailable;
        }
        if !database.system_administrator_exists {
            return HealthStatus::BootstrapRequired;
        }
        if !self.required_configuration_ready
            || free_bytes.is_err()
            || free_bytes.is_ok_and(|bytes| bytes < self.free_reserve_bytes)
        {
            return HealthStatus::Unavailable;
        }
        HealthStatus::Ready
    }

    async fn registration_gate(&self) -> RegistrationGate {
        match self.database.database_health().await {
            Ok(database) if database.schema_version != COMPATIBLE_SCHEMA_VERSION => {
                RegistrationGate::Unavailable
            }
            Ok(database) if database.system_administrator_exists => RegistrationGate::Available,
            Ok(_) => RegistrationGate::BootstrapRequired,
            Err(_) => RegistrationGate::Unavailable,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ReadyOperations;

#[async_trait]
impl OperationsApi for ReadyOperations {
    async fn readiness(&self) -> HealthStatus {
        HealthStatus::Ready
    }

    async fn registration_gate(&self) -> RegistrationGate {
        RegistrationGate::Available
    }
}
