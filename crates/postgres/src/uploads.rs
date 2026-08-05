use async_trait::async_trait;
use folioharbor_application::ports::{
    AuthorizedUploadTransition, CreateUploadRecord, ExpireUploads, FinalizeUploadReceipt,
    HeartbeatUploadReceipt, LeaseUploadCleanups, MarkUploadReceived, PrepareUploadPromotion,
    RecordUploadCleanup, UploadCleanup, UploadRepository, UploadRepositoryError,
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

    async fn finalize_authorized(
        &self,
        receipt: FinalizeUploadReceipt,
    ) -> Result<bool, UploadRepositoryError> {
        let mut transaction = self
            .transaction(receipt.actor, receipt.library_id, receipt.request_id)
            .await?;
        let received =
            i64::try_from(receipt.received.get()).map_err(|_| UploadRepositoryError::Invalid)?;
        let changed = sqlx::query_scalar!(
            r#"SELECT folioharbor.upload_finalize_authorized($1,$2,$3,$4,$5,$6,$7,$8) AS "changed!""#,
            receipt.upload_id.as_uuid(),
            receipt.library_id.as_uuid(),
            receipt.actor.as_uuid(),
            received,
            receipt.storage_key,
            receipt.staging_key,
            receipt.job_id.as_uuid(),
            receipt.now,
        )
        .fetch_one(&mut *transaction)
        .await
        .map_err(persistence_error)?;
        transaction.commit().await.map_err(persistence_error)?;
        Ok(changed)
    }

    async fn heartbeat_receipt(
        &self,
        receipt: HeartbeatUploadReceipt,
    ) -> Result<bool, UploadRepositoryError> {
        let mut transaction = self
            .transaction(receipt.actor, receipt.library_id, receipt.request_id)
            .await?;
        let changed: bool =
            sqlx::query_scalar("SELECT folioharbor.upload_heartbeat_authorized($1,$2,$3,$4,$5)")
                .bind(receipt.upload_id.as_uuid())
                .bind(receipt.library_id.as_uuid())
                .bind(receipt.actor.as_uuid())
                .bind(receipt.staging_key)
                .bind(receipt.now)
                .fetch_one(&mut *transaction)
                .await
                .map_err(persistence_error)?;
        transaction.commit().await.map_err(persistence_error)?;
        Ok(changed)
    }

    async fn prepare_promotion(
        &self,
        promotion: PrepareUploadPromotion,
    ) -> Result<bool, UploadRepositoryError> {
        let mut transaction = self
            .transaction(promotion.actor, promotion.library_id, promotion.request_id)
            .await?;
        let changed: bool = sqlx::query_scalar(
            "SELECT folioharbor.upload_prepare_promotion_authorized($1,$2,$3,$4,$5,$6,$7)",
        )
        .bind(promotion.upload_id.as_uuid())
        .bind(promotion.library_id.as_uuid())
        .bind(promotion.actor.as_uuid())
        .bind(promotion.staging_key)
        .bind(promotion.final_key)
        .bind(promotion.final_owned)
        .bind(promotion.now)
        .fetch_one(&mut *transaction)
        .await
        .map_err(persistence_error)?;
        transaction.commit().await.map_err(persistence_error)?;
        Ok(changed)
    }

    async fn mark_received(
        &self,
        receipt: MarkUploadReceived,
    ) -> Result<bool, UploadRepositoryError> {
        let received =
            i64::try_from(receipt.received.get()).map_err(|_| UploadRepositoryError::Invalid)?;
        let mut transaction = self
            .transaction(receipt.actor, receipt.library_id, receipt.request_id)
            .await?;
        let changed: bool = sqlx::query_scalar(
            "SELECT folioharbor.upload_mark_received_authorized($1,$2,$3,$4,$5,$6,$7)",
        )
        .bind(receipt.upload_id.as_uuid())
        .bind(receipt.library_id.as_uuid())
        .bind(receipt.actor.as_uuid())
        .bind(receipt.staging_key)
        .bind(receipt.final_key)
        .bind(received)
        .bind(receipt.now)
        .fetch_one(&mut *transaction)
        .await
        .map_err(persistence_error)?;
        transaction.commit().await.map_err(persistence_error)?;
        Ok(changed)
    }

    async fn record_orphan_cleanup(
        &self,
        cleanup: RecordUploadCleanup,
    ) -> Result<(), UploadRepositoryError> {
        let mut transaction = self
            .transaction(cleanup.actor, cleanup.library_id, cleanup.request_id)
            .await?;
        sqlx::query("SELECT folioharbor.upload_record_orphan_cleanup_authorized($1,$2,$3,$4,$5)")
            .bind(cleanup.upload_id.as_uuid())
            .bind(cleanup.library_id.as_uuid())
            .bind(cleanup.actor.as_uuid())
            .bind(cleanup.staging_key)
            .bind(cleanup.now)
            .execute(&mut *transaction)
            .await
            .map_err(persistence_error)?;
        transaction.commit().await.map_err(persistence_error)
    }

    async fn expire_worker(&self, request: ExpireUploads) -> Result<u64, UploadRepositoryError> {
        let mut transaction = self.pool.begin().await.map_err(persistence_error)?;
        PgTransactionContext::apply(
            &mut transaction,
            &DatabaseContext::worker(request.request_id, None),
        )
        .await
        .map_err(persistence_error)?;
        let limit = i64::from(request.limit);
        let expired = sqlx::query_scalar!(
            r#"SELECT folioharbor.upload_expire_worker($1,$2) AS "expired!""#,
            request.now,
            limit,
        )
        .fetch_one(&mut *transaction)
        .await
        .map_err(persistence_error)?;
        transaction.commit().await.map_err(persistence_error)?;
        u64::try_from(expired).map_err(|_| UploadRepositoryError::Persistence)
    }

    async fn lease_cleanups(
        &self,
        request: LeaseUploadCleanups,
    ) -> Result<Vec<UploadCleanup>, UploadRepositoryError> {
        let mut transaction = self.pool.begin().await.map_err(persistence_error)?;
        PgTransactionContext::apply(
            &mut transaction,
            &DatabaseContext::worker(request.request_id, None),
        )
        .await
        .map_err(persistence_error)?;
        let expires = request.now + request.lease_for;
        let rows: Vec<(uuid::Uuid, uuid::Uuid, String, Option<String>, bool)> = sqlx::query_as(
            r"WITH candidates AS (
                 SELECT upload_id,attempt_token FROM folioharbor.upload_cleanups
                 WHERE state='pending' OR (state='leased' AND lease_expires_at<=$1)
                 ORDER BY created_at FOR UPDATE SKIP LOCKED LIMIT $2
               )
               UPDATE folioharbor.upload_cleanups c SET state='leased',lease_owner=$3,lease_expires_at=$4
               FROM candidates x WHERE c.upload_id=x.upload_id AND c.attempt_token=x.attempt_token
               RETURNING c.upload_id,c.attempt_token,c.staging_key,c.final_key,c.final_owned",
        ).bind(request.now).bind(i64::from(request.limit)).bind(&request.owner).bind(expires)
         .fetch_all(&mut *transaction).await.map_err(persistence_error)?;
        transaction.commit().await.map_err(persistence_error)?;
        Ok(rows
            .into_iter()
            .map(|row| UploadCleanup {
                upload_id: UploadId::from_uuid(row.0),
                attempt_token: row.1.to_string(),
                staging_key: row.2,
                final_key: row.3,
                final_owned: row.4,
            })
            .collect())
    }

    async fn complete_cleanup(
        &self,
        upload: UploadId,
        attempt: &str,
        owner: &str,
        now: folioharbor_domain::time::OffsetDateTime,
        request: RequestId,
    ) -> Result<bool, UploadRepositoryError> {
        let attempt = uuid::Uuid::parse_str(attempt).map_err(|_| UploadRepositoryError::Invalid)?;
        let mut transaction = self.pool.begin().await.map_err(persistence_error)?;
        PgTransactionContext::apply(&mut transaction, &DatabaseContext::worker(request, None))
            .await
            .map_err(persistence_error)?;
        let changed = sqlx::query("UPDATE folioharbor.upload_cleanups SET state='completed',lease_owner=NULL,lease_expires_at=NULL,completed_at=$4 WHERE upload_id=$1 AND attempt_token=$2 AND state='leased' AND lease_owner=$3 AND lease_expires_at>$4")
            .bind(upload.as_uuid()).bind(attempt).bind(owner).bind(now)
            .execute(&mut *transaction).await.map_err(persistence_error)?.rows_affected()==1;
        transaction.commit().await.map_err(persistence_error)?;
        Ok(changed)
    }
}
