use async_trait::async_trait;
use folioharbor_application::ports::{
    AuthorizedUploadTransition, CreateUploadRecord, UploadRepository, UploadRepositoryError,
    WorkerUploadTransition,
};
use folioharbor_domain::{
    id::{LibraryId, RequestId, UploadId, UserId},
    imports::{
        blob::StorageKey,
        quota::ByteCount,
        upload::{UploadSession, UploadState},
    },
};
use sqlx::PgPool;

use crate::{DatabaseContext, PgTransactionContext};

#[derive(Clone, Debug)]
pub struct PgUploadRepository {
    pool: PgPool,
}

impl PgUploadRepository {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn transaction(
        &self,
        actor: UserId,
        library: LibraryId,
        request: RequestId,
    ) -> Result<sqlx::Transaction<'_, sqlx::Postgres>, UploadRepositoryError> {
        let mut transaction = self.pool.begin().await.map_err(persistence_error)?;
        PgTransactionContext::apply(
            &mut transaction,
            &DatabaseContext::api(actor, library, request),
        )
        .await
        .map_err(persistence_error)?;
        Ok(transaction)
    }
}

fn persistence_error(_: sqlx::Error) -> UploadRepositoryError {
    UploadRepositoryError::Persistence
}

#[async_trait]
impl UploadRepository for PgUploadRepository {
    async fn create_authorized(
        &self,
        record: CreateUploadRecord,
    ) -> Result<UploadSession, UploadRepositoryError> {
        let mut transaction = self
            .transaction(record.actor, record.library_id, record.request_id)
            .await?;
        let declared = i64::try_from(record.declared_bytes.get())
            .map_err(|_| UploadRepositoryError::Invalid)?;
        let outcome = sqlx::query_scalar!(
            r#"SELECT folioharbor.upload_create_authorized($1,$2,$3,$4,$5,$6,$7,$8) AS "outcome!""#,
            record.upload_id.as_uuid(),
            record.library_id.as_uuid(),
            record.actor.as_uuid(),
            &record.file_name,
            &record.media_type,
            declared,
            record.expires_at,
            record.now
        )
        .fetch_one(&mut *transaction)
        .await
        .map_err(persistence_error)?;
        if outcome != "created" {
            return Err(match outcome.as_str() {
                "quota_exceeded" => UploadRepositoryError::QuotaExceeded,
                "conflict" => UploadRepositoryError::Conflict,
                "invalid" => UploadRepositoryError::Invalid,
                "forbidden" => UploadRepositoryError::Forbidden,
                "not_found" => UploadRepositoryError::NotFound,
                _ => UploadRepositoryError::Persistence,
            });
        }
        transaction.commit().await.map_err(persistence_error)?;
        Ok(UploadSession {
            upload_id: record.upload_id,
            library_id: record.library_id,
            file_name: record.file_name,
            media_type: record.media_type,
            declared_bytes: record.declared_bytes,
            received_bytes: ByteCount::new(0),
            state: UploadState::Created,
            storage_key: None,
            error_code: None,
        })
    }

    async fn find_authorized(
        &self,
        actor: UserId,
        library: LibraryId,
        upload: UploadId,
        request: RequestId,
    ) -> Result<Option<UploadSession>, UploadRepositoryError> {
        let mut transaction = self.transaction(actor, library, request).await?;
        let row = sqlx::query!("SELECT file_name,media_type,declared_bytes,received_bytes,state,storage_key,error_code FROM folioharbor.upload_sessions WHERE upload_id=$1 AND library_id=$2",upload.as_uuid(),library.as_uuid()).fetch_optional(&mut *transaction).await.map_err(persistence_error)?;
        transaction.commit().await.map_err(persistence_error)?;
        row.map(|row| {
            Ok(UploadSession {
                upload_id: upload,
                library_id: library,
                file_name: row.file_name,
                media_type: row.media_type,
                declared_bytes: ByteCount::new(
                    u64::try_from(row.declared_bytes)
                        .map_err(|_| UploadRepositoryError::Persistence)?,
                ),
                received_bytes: ByteCount::new(
                    u64::try_from(row.received_bytes)
                        .map_err(|_| UploadRepositoryError::Persistence)?,
                ),
                state: UploadState::parse(&row.state).ok_or(UploadRepositoryError::Persistence)?,
                storage_key: row.storage_key.map(StorageKey::from_opaque),
                error_code: row.error_code,
            })
        })
        .transpose()
    }

    async fn transition_authorized(
        &self,
        change: AuthorizedUploadTransition,
    ) -> Result<bool, UploadRepositoryError> {
        let mut transaction = self
            .transaction(change.actor, change.library_id, change.request_id)
            .await?;
        let received =
            i64::try_from(change.received.get()).map_err(|_| UploadRepositoryError::Invalid)?;
        let changed = sqlx::query_scalar!(r#"SELECT folioharbor.upload_transition_authorized($1,$2,$3,$4,$5,$6,$7,$8,$9) AS "changed!""#,
            change.upload_id.as_uuid(),change.library_id.as_uuid(),change.actor.as_uuid(),change.from.as_str(),change.to.as_str(),received,change.storage_key,change.error_code,change.now)
        .fetch_one(&mut *transaction)
        .await
        .map_err(persistence_error)?;
        transaction.commit().await.map_err(persistence_error)?;
        Ok(changed)
    }

    async fn transition_worker(
        &self,
        change: WorkerUploadTransition,
    ) -> Result<bool, UploadRepositoryError> {
        let mut transaction = self.pool.begin().await.map_err(persistence_error)?;
        PgTransactionContext::apply(
            &mut transaction,
            &DatabaseContext::worker(change.request_id, Some(change.library_id)),
        )
        .await
        .map_err(persistence_error)?;
        let changed = sqlx::query_scalar!(
            r#"SELECT folioharbor.upload_transition_worker($1,$2,$3,$4,$5,$6) AS "changed!""#,
            change.upload_id.as_uuid(),
            change.library_id.as_uuid(),
            change.from.as_str(),
            change.to.as_str(),
            change.error_code,
            change.now
        )
        .fetch_one(&mut *transaction)
        .await
        .map_err(persistence_error)?;
        transaction.commit().await.map_err(persistence_error)?;
        Ok(changed)
    }
}
