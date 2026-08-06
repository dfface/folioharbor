use async_trait::async_trait;
use folioharbor_application::imports::CleanupCursor;
use folioharbor_application::ports::{
    FailedUploadPurge, ImportCleanupRepository, ImportCleanupRepositoryError, ImportReconciliation,
    ImportRepository, ImportRepositoryError, ImportWork,
};
use folioharbor_domain::{
    id::{BlobId, LibraryId, RequestId, UploadId, UserId},
    imports::{blob::StorageKey, job::JobKind, quota::ByteCount, upload::UploadState},
    time::OffsetDateTime,
};
use sqlx::PgPool;

use crate::{DatabaseContext, PgTransactionContext};

#[derive(Clone, Debug)]
pub struct PgImportRepository {
    pool: PgPool,
}

#[derive(Clone, Debug)]
pub struct PgImportCleanupRepository {
    pool: PgPool,
}

impl PgImportCleanupRepository {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ImportCleanupRepository for PgImportCleanupRepository {
    async fn begin_pass(
        &self,
        kind: JobKind,
        owner: &str,
        now: OffsetDateTime,
        limit: u32,
    ) -> Result<CleanupCursor, ImportCleanupRepositoryError> {
        let mut transaction = cleanup_transaction(&self.pool, RequestId::new()).await?;
        let cutoff = sqlx::query_scalar!(
            "SELECT folioharbor.import_begin_cleanup_worker($1,$2,$3)",
            kind.as_str(),
            owner,
            now,
        )
        .fetch_one(&mut *transaction)
        .await
        .map_err(cleanup_error)?
        .ok_or(ImportCleanupRepositoryError)?;
        transaction.commit().await.map_err(cleanup_error)?;
        CleanupCursor::new(cutoff, limit).ok_or(ImportCleanupRepositoryError)
    }

    async fn complete_pass(
        &self,
        kind: JobKind,
        owner: &str,
        cursor: CleanupCursor,
    ) -> Result<(), ImportCleanupRepositoryError> {
        let mut transaction = cleanup_transaction(&self.pool, RequestId::new()).await?;
        let completed = sqlx::query_scalar!(
            r#"SELECT folioharbor.import_complete_cleanup_worker($1,$2,$3,$4) AS "completed!""#,
            kind.as_str(),
            owner,
            cursor.not_after(),
            OffsetDateTime::now_utc(),
        )
        .fetch_one(&mut *transaction)
        .await
        .map_err(cleanup_error)?;
        transaction.commit().await.map_err(cleanup_error)?;
        completed.then_some(()).ok_or(ImportCleanupRepositoryError)
    }

    async fn has_pending(
        &self,
        kind: JobKind,
        cursor: CleanupCursor,
    ) -> Result<bool, ImportCleanupRepositoryError> {
        let mut transaction = cleanup_transaction(&self.pool, RequestId::new()).await?;
        let pending = sqlx::query_scalar!(
            r#"SELECT folioharbor.import_cleanup_pending_worker($1,$2) AS "pending!""#,
            kind.as_str(),
            cursor.not_after(),
        )
        .fetch_one(&mut *transaction)
        .await
        .map_err(cleanup_error)?;
        transaction.commit().await.map_err(cleanup_error)?;
        Ok(pending)
    }
    async fn expire_abandoned(
        &self,
        cursor: CleanupCursor,
    ) -> Result<u64, ImportCleanupRepositoryError> {
        let mut transaction = cleanup_transaction(&self.pool, RequestId::new()).await?;
        let expired = sqlx::query_scalar!(
            r#"SELECT folioharbor.import_expire_abandoned_worker($1,$2) AS "expired!""#,
            cursor.not_after(),
            i64::from(cursor.limit()),
        )
        .fetch_one(&mut *transaction)
        .await
        .map_err(cleanup_error)?;
        transaction.commit().await.map_err(cleanup_error)?;
        u64::try_from(expired).map_err(|_| ImportCleanupRepositoryError)
    }

    async fn claim_failed_purges(
        &self,
        owner: &str,
        cursor: CleanupCursor,
        claim_now: OffsetDateTime,
    ) -> Result<Vec<FailedUploadPurge>, ImportCleanupRepositoryError> {
        let mut transaction = cleanup_transaction(&self.pool, RequestId::new()).await?;
        let rows = sqlx::query!(
            r#"SELECT upload_id AS "upload_id!",storage_key AS "storage_key!",delete_file AS "delete_file!" FROM folioharbor.import_claim_failed_purges_worker($1,$2,$3,$4)"#,
            owner,
            cursor.not_after(),
            claim_now,
            i64::from(cursor.limit()),
        )
        .fetch_all(&mut *transaction)
        .await
        .map_err(cleanup_error)?;
        transaction.commit().await.map_err(cleanup_error)?;
        Ok(rows
            .into_iter()
            .map(|row| FailedUploadPurge {
                upload_id: UploadId::from_uuid(row.upload_id),
                storage_key: StorageKey::from_opaque(row.storage_key),
                delete_file: row.delete_file,
            })
            .collect())
    }

    async fn complete_failed_purge(
        &self,
        upload_id: UploadId,
        owner: &str,
        cursor: CleanupCursor,
    ) -> Result<bool, ImportCleanupRepositoryError> {
        let mut transaction = cleanup_transaction(&self.pool, RequestId::new()).await?;
        let completed = sqlx::query_scalar!(
            r#"SELECT folioharbor.import_complete_failed_purge_worker($1,$2,$3) AS "completed!""#,
            upload_id.as_uuid(),
            owner,
            cursor.not_after(),
        )
        .fetch_one(&mut *transaction)
        .await
        .map_err(cleanup_error)?;
        transaction.commit().await.map_err(cleanup_error)?;
        Ok(completed)
    }

    async fn release_failed_purge(
        &self,
        upload_id: UploadId,
        owner: &str,
        now: OffsetDateTime,
    ) -> Result<(), ImportCleanupRepositoryError> {
        let mut transaction = cleanup_transaction(&self.pool, RequestId::new()).await?;
        let released = sqlx::query_scalar!(
            r#"SELECT folioharbor.import_release_failed_purge_worker($1,$2,$3) AS "released!""#,
            upload_id.as_uuid(),
            owner,
            now,
        )
        .fetch_one(&mut *transaction)
        .await
        .map_err(cleanup_error)?;
        transaction.commit().await.map_err(cleanup_error)?;
        released.then_some(()).ok_or(ImportCleanupRepositoryError)
    }
}

async fn cleanup_transaction(
    pool: &PgPool,
    request_id: RequestId,
) -> Result<sqlx::Transaction<'_, sqlx::Postgres>, ImportCleanupRepositoryError> {
    let mut transaction = pool.begin().await.map_err(cleanup_error)?;
    PgTransactionContext::apply(&mut transaction, &DatabaseContext::worker(request_id, None))
        .await
        .map_err(cleanup_error)?;
    Ok(transaction)
}

fn cleanup_error(_: sqlx::Error) -> ImportCleanupRepositoryError {
    ImportCleanupRepositoryError
}

impl PgImportRepository {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ImportRepository for PgImportRepository {
    async fn reconcile(
        &self,
        upload_id: UploadId,
        library_id: LibraryId,
        request_id: RequestId,
        now: OffsetDateTime,
    ) -> Result<ImportReconciliation, ImportRepositoryError> {
        let mut transaction = self.pool.begin().await.map_err(unavailable)?;
        PgTransactionContext::apply(
            &mut transaction,
            &DatabaseContext::worker(request_id, Some(library_id)),
        )
        .await
        .map_err(unavailable)?;
        let request = request_id.as_ulid().to_string();
        let row = sqlx::query!(
            r#"SELECT outcome AS "outcome!",actor_id AS "actor_id!",blob_id,logical_bytes AS "logical_bytes!",storage_key,upload_state AS "upload_state!",error_code FROM folioharbor.import_reconcile_worker($1,$2,$3,$4,$5)"#,
            upload_id.as_uuid(),
            library_id.as_uuid(),
            BlobId::new().as_uuid(),
            request,
            now,
        )
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|error| classify(&error))?;
        transaction.commit().await.map_err(unavailable)?;
        let Some(row) = row else {
            return Err(ImportRepositoryError::InvalidState);
        };
        if row.outcome == "complete" {
            return Ok(ImportReconciliation::Complete);
        }
        if row.outcome == "failed" {
            return Ok(ImportReconciliation::TerminalFailure {
                code: row.error_code.unwrap_or_else(|| "import_failed".to_owned()),
            });
        }
        Ok(ImportReconciliation::Work(ImportWork {
            upload_id,
            library_id,
            actor_id: UserId::from_uuid(row.actor_id),
            blob_id: BlobId::from_uuid(row.blob_id.ok_or(ImportRepositoryError::InvalidState)?),
            logical_bytes: ByteCount::new(
                u64::try_from(row.logical_bytes).map_err(|_| ImportRepositoryError::Schema)?,
            ),
            storage_key: StorageKey::from_opaque(
                row.storage_key.ok_or(ImportRepositoryError::InvalidState)?,
            ),
            state: UploadState::parse(&row.upload_state).ok_or(ImportRepositoryError::Schema)?,
        }))
    }

    async fn begin_catalog(
        &self,
        work: &ImportWork,
        request_id: RequestId,
        now: OffsetDateTime,
    ) -> Result<(), ImportRepositoryError> {
        if work.state == UploadState::Importing {
            return Ok(());
        }
        transition_from(
            &self.pool,
            work,
            work.state,
            UploadState::Importing,
            None,
            request_id,
            now,
        )
        .await?
        .then_some(())
        .ok_or(ImportRepositoryError::InvalidState)
    }

    async fn record_failure(
        &self,
        work: &ImportWork,
        to: UploadState,
        code: &'static str,
        request_id: RequestId,
        now: OffsetDateTime,
    ) -> Result<(), ImportRepositoryError> {
        let mut transaction = self.pool.begin().await.map_err(unavailable)?;
        PgTransactionContext::apply(
            &mut transaction,
            &DatabaseContext::worker(request_id, Some(work.library_id)),
        )
        .await
        .map_err(unavailable)?;
        let changed = sqlx::query_scalar!(
            r#"SELECT folioharbor.import_record_failure_worker($1,$2,$3,$4,$5) AS "changed!""#,
            work.upload_id.as_uuid(),
            work.library_id.as_uuid(),
            to.as_str(),
            code,
            now,
        )
        .fetch_one(&mut *transaction)
        .await
        .map_err(|error| classify(&error))?;
        transaction.commit().await.map_err(unavailable)?;
        changed
            .then_some(())
            .ok_or(ImportRepositoryError::InvalidState)
    }
}

#[allow(clippy::too_many_arguments)]
async fn transition_from(
    pool: &PgPool,
    work: &ImportWork,
    from: UploadState,
    to: UploadState,
    code: Option<&str>,
    request_id: RequestId,
    now: OffsetDateTime,
) -> Result<bool, ImportRepositoryError> {
    let mut transaction = pool.begin().await.map_err(unavailable)?;
    PgTransactionContext::apply(
        &mut transaction,
        &DatabaseContext::worker(request_id, Some(work.library_id)),
    )
    .await
    .map_err(unavailable)?;
    let changed = sqlx::query_scalar!(
        r#"SELECT folioharbor.upload_transition_worker($1,$2,$3,$4,$5,$6) AS "changed!""#,
        work.upload_id.as_uuid(),
        work.library_id.as_uuid(),
        from.as_str(),
        to.as_str(),
        code,
        now,
    )
    .fetch_one(&mut *transaction)
    .await
    .map_err(|error| classify(&error))?;
    transaction.commit().await.map_err(unavailable)?;
    Ok(changed)
}

fn unavailable(_: sqlx::Error) -> ImportRepositoryError {
    ImportRepositoryError::Unavailable
}

fn classify(error: &sqlx::Error) -> ImportRepositoryError {
    match error {
        sqlx::Error::Database(database)
            if matches!(
                database.code().as_deref(),
                Some("42P01" | "42883" | "42703")
            ) =>
        {
            ImportRepositoryError::Schema
        }
        _ => ImportRepositoryError::Unavailable,
    }
}
