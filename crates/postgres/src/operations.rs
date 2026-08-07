use async_trait::async_trait;
use folioharbor_application::operations::{
    BlobInventoryEntry, BootstrapAdminOutcome, BootstrapAdminRepository,
    BootstrapAdminRepositoryError, ConsistencyRepository, ConsistencyRepositoryError,
    DatabaseHealth, HealthRepository, HealthRepositoryError, NewSystemAdministrator,
};
use folioharbor_domain::imports::blob::StorageKey;
use sqlx::{PgPool, Row as _};

#[derive(Clone, Debug)]
pub struct PgOperationsRepository {
    pool: PgPool,
}

impl PgOperationsRepository {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl HealthRepository for PgOperationsRepository {
    async fn database_health(&self) -> Result<DatabaseHealth, HealthRepositoryError> {
        let (schema_version, system_administrator_exists): (i64, bool) =
            sqlx::query_as("SELECT schema_version, system_administrator_exists FROM folioharbor.operations_health()")
                .fetch_one(&self.pool)
                .await
                .map_err(|_| HealthRepositoryError)?;
        Ok(DatabaseHealth {
            schema_version,
            system_administrator_exists,
        })
    }
}

#[async_trait]
impl BootstrapAdminRepository for PgOperationsRepository {
    async fn bootstrap_admin(
        &self,
        administrator: NewSystemAdministrator,
    ) -> Result<BootstrapAdminOutcome, BootstrapAdminRepositoryError> {
        let outcome: String =
            sqlx::query_scalar("SELECT folioharbor.operations_bootstrap_admin($1,$2,$3,$4,$5)")
                .bind(administrator.user_id.as_uuid())
                .bind(administrator.normalized_email.as_str())
                .bind(administrator.display_email)
                .bind(administrator.password_hash)
                .bind(administrator.created_at)
                .fetch_one(&self.pool)
                .await
                .map_err(|_| BootstrapAdminRepositoryError)?;
        match outcome.as_str() {
            "created" => Ok(BootstrapAdminOutcome::Created),
            "already_administrator" => Ok(BootstrapAdminOutcome::AlreadyAdministrator),
            _ => Err(BootstrapAdminRepositoryError),
        }
    }
}

#[async_trait]
impl ConsistencyRepository for PgOperationsRepository {
    async fn ready_blob_inventory(
        &self,
    ) -> Result<Vec<BlobInventoryEntry>, ConsistencyRepositoryError> {
        let rows = sqlx::query(
            "SELECT location.storage_key, blob.storage_namespace, blob.sha256, \
                    encode(blob.sha256, 'hex') AS sha256_hex, blob.byte_size \
             FROM folioharbor.blob_locations AS location \
             JOIN folioharbor.blobs AS blob USING (blob_id) \
             WHERE location.state = 'ready' \
             ORDER BY location.blob_id, location.storage_key",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|_| ConsistencyRepositoryError)?;
        rows.into_iter()
            .map(|row| {
                let digest = row
                    .try_get::<Vec<u8>, _>("sha256")
                    .map_err(|_| ConsistencyRepositoryError)?;
                let expected_sha256: [u8; 32] =
                    digest.try_into().map_err(|_| ConsistencyRepositoryError)?;
                let byte_size = row
                    .try_get::<i64, _>("byte_size")
                    .map_err(|_| ConsistencyRepositoryError)?;
                let expected_byte_size =
                    u64::try_from(byte_size).map_err(|_| ConsistencyRepositoryError)?;
                let storage_key: String = row
                    .try_get("storage_key")
                    .map_err(|_| ConsistencyRepositoryError)?;
                let namespace: String = row
                    .try_get("storage_namespace")
                    .map_err(|_| ConsistencyRepositoryError)?;
                let sha256_hex: String = row
                    .try_get("sha256_hex")
                    .map_err(|_| ConsistencyRepositoryError)?;
                let location_is_canonical =
                    storage_key == format!("blob:{namespace}:{sha256_hex}:{expected_byte_size}");
                Ok(BlobInventoryEntry {
                    storage_key: StorageKey::from_opaque(storage_key),
                    location_is_canonical,
                    expected_sha256,
                    expected_byte_size,
                })
            })
            .collect()
    }
}
