use sqlx::{PgPool, migrate::Migrator};
use thiserror::Error;

static MIGRATOR: Migrator = sqlx::migrate!("../../migrations");
const MIGRATION_LOCK_ID: i64 = 5_066_353_826_641_225_812;

#[derive(Debug, Error)]
pub enum MigrationError {
    #[error("migration database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("schema migration failed")]
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error("migrations require folioharbor_owner credentials, got {0}")]
    WrongRole(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationReport {
    pub versions: Vec<i64>,
}

/// Runs embedded migrations while holding `FolioHarbor`'s fixed advisory lock.
///
/// `SQLx` rejects dirty, checksum-mismatched, or source-missing/newer migration
/// histories. This entry point also refuses runtime-role credentials.
///
/// # Errors
///
/// Returns [`MigrationError`] for role, locking, migration, or reporting errors.
pub async fn run_migrations(pool: &PgPool) -> Result<MigrationReport, MigrationError> {
    // Advisory locks are session-scoped. Detaching ensures every early return,
    // cancellation, or unwind closes the locked backend instead of pooling it.
    let mut connection = pool.acquire().await?.detach();
    let role: String = sqlx::query_scalar("SELECT current_user")
        .fetch_one(&mut connection)
        .await?;
    if role != "folioharbor_owner" {
        return Err(MigrationError::WrongRole(role));
    }

    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(MIGRATION_LOCK_ID)
        .execute(&mut connection)
        .await?;
    let migration_result = MIGRATOR.run_direct(&mut connection).await;
    let unlock_result = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(MIGRATION_LOCK_ID)
        .execute(&mut connection)
        .await;

    migration_result?;
    unlock_result?;

    let versions =
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE success ORDER BY version")
            .fetch_all(&mut connection)
            .await?;
    Ok(MigrationReport { versions })
}
