use async_trait::async_trait;
use folioharbor_application::ports::{
    AuthorizedUploadTransition, BeginUploadReceipt, BlobDisposition, ClaimUploadCleanup,
    CreateUploadRecord, ExpireUploads, FinalizeUploadReceipt, HeartbeatUploadReceipt,
    MarkUploadReceived, PrepareUploadPromotion, RecordPromotionDisposition, RecordUploadCleanup,
    UploadCleanup, UploadCleanupGuard, UploadReceiptAttempt, UploadRepository,
    UploadRepositoryError, WorkerUploadTransition,
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

struct PgUploadCleanupGuard {
    connection: Option<sqlx::pool::PoolConnection<sqlx::Postgres>>,
    cleanup: UploadCleanup,
}

impl Drop for PgUploadCleanupGuard {
    fn drop(&mut self) {
        if let Some(connection) = &mut self.connection {
            connection.close_on_drop();
        }
    }
}

#[async_trait]
impl UploadCleanupGuard for PgUploadCleanupGuard {
    fn cleanup(&self) -> &UploadCleanup {
        &self.cleanup
    }

    async fn complete(
        mut self: Box<Self>,
        now: folioharbor_domain::time::OffsetDateTime,
    ) -> Result<bool, UploadRepositoryError> {
        let connection = self
            .connection
            .as_mut()
            .ok_or(UploadRepositoryError::Persistence)?;
        let attempt = uuid::Uuid::parse_str(&self.cleanup.attempt_token)
            .map_err(|_| UploadRepositoryError::Persistence)?;
        let changed = sqlx::query("UPDATE folioharbor.upload_cleanups SET state='completed',lease_owner=NULL,lease_expires_at=NULL,completed_at=$3 WHERE upload_id=$1 AND attempt_token=$2 AND state='pending'")
            .bind(self.cleanup.upload_id.as_uuid()).bind(attempt).bind(now)
            .execute(&mut **connection).await.map_err(persistence_error)?.rows_affected()==1;
        sqlx::query("COMMIT")
            .execute(&mut **connection)
            .await
            .map_err(persistence_error)?;
        let _ = self.connection.take();
        Ok(changed)
    }

    async fn abandon(mut self: Box<Self>) -> Result<(), UploadRepositoryError> {
        let connection = self
            .connection
            .as_mut()
            .ok_or(UploadRepositoryError::Persistence)?;
        sqlx::query("ROLLBACK")
            .execute(&mut **connection)
            .await
            .map_err(persistence_error)?;
        let _ = self.connection.take();
        Ok(())
    }
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

fn attempt_uuid(value: &str) -> Result<uuid::Uuid, UploadRepositoryError> {
    value.parse().map_err(|_| UploadRepositoryError::Invalid)
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
        let scope = match record.dedup_scope {
            folioharbor_domain::imports::blob::DedupScope::Instance => "instance",
            folioharbor_domain::imports::blob::DedupScope::Library => "library",
            folioharbor_domain::imports::blob::DedupScope::Disabled => "disabled",
        };
        let outcome = sqlx::query_scalar!(
            r#"SELECT folioharbor.upload_create_authorized($1,$2,$3,$4,$5,$6,$7,$8,$9) AS "outcome!""#,
            record.upload_id.as_uuid(),
            record.library_id.as_uuid(),
            record.actor.as_uuid(),
            &record.file_name,
            &record.media_type,
            declared,
            scope,
            record.expires_at,
            record.now,
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
            item_id: None,
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
        let row = sqlx::query_as::<_, (String, String, i64, i64, String, Option<String>, Option<String>, Option<uuid::Uuid>)>("SELECT file_name,media_type,declared_bytes,received_bytes,state,storage_key,error_code,result_item_id FROM folioharbor.upload_sessions WHERE upload_id=$1 AND library_id=$2")
            .bind(upload.as_uuid())
            .bind(library.as_uuid())
            .fetch_optional(&mut *transaction)
            .await
            .map_err(persistence_error)?;
        transaction.commit().await.map_err(persistence_error)?;
        row.map(|row| {
            Ok(UploadSession {
                upload_id: upload,
                library_id: library,
                file_name: row.0,
                media_type: row.1,
                declared_bytes: ByteCount::new(
                    u64::try_from(row.2).map_err(|_| UploadRepositoryError::Persistence)?,
                ),
                received_bytes: ByteCount::new(
                    u64::try_from(row.3).map_err(|_| UploadRepositoryError::Persistence)?,
                ),
                state: UploadState::parse(&row.4).ok_or(UploadRepositoryError::Persistence)?,
                storage_key: row.5.map(StorageKey::from_opaque),
                error_code: row.6,
                item_id: row.7.map(folioharbor_domain::id::ItemId::from_uuid),
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
        let attempt = change
            .attempt_token
            .as_deref()
            .map(attempt_uuid)
            .transpose()?;
        let changed = sqlx::query_scalar!(
            r#"SELECT folioharbor.upload_transition_authorized($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) AS "changed!""#,
            change.upload_id.as_uuid(),
            change.library_id.as_uuid(),
            change.actor.as_uuid(),
            change.from.as_str(),
            change.to.as_str(),
            received,
            attempt,
            change.storage_key,
            change.error_code,
            change.now,
        )
        .fetch_one(&mut *transaction)
        .await
        .map_err(persistence_error)?;
        transaction.commit().await.map_err(persistence_error)?;
        Ok(changed)
    }

    async fn begin_receipt(
        &self,
        receipt: BeginUploadReceipt,
    ) -> Result<Option<UploadReceiptAttempt>, UploadRepositoryError> {
        let mut transaction = self
            .transaction(receipt.actor, receipt.library_id, receipt.request_id)
            .await?;
        let row = sqlx::query!(
            r#"SELECT attempt_token AS "attempt_token!",staging_key AS "staging_key!" FROM folioharbor.upload_begin_receipt_authorized($1,$2,$3,$4,$5)"#,
            receipt.upload_id.as_uuid(),
            receipt.library_id.as_uuid(),
            receipt.actor.as_uuid(),
            receipt.from.as_str(),
            receipt.now,
        )
        .fetch_optional(&mut *transaction)
        .await
        .map_err(persistence_error)?;
        transaction.commit().await.map_err(persistence_error)?;
        Ok(row.map(|row| UploadReceiptAttempt {
            attempt_token: row.attempt_token.to_string(),
            staging_key: row.staging_key,
        }))
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
        let request_id = receipt.request_id.as_ulid().to_string();
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
            &receipt.storage_key,
            receipt.staging_key.as_deref(),
            receipt.job_id.as_uuid(),
            receipt.now,
        )
        .fetch_one(&mut *transaction)
        .await
        .map_err(persistence_error)?;
        if changed {
            let job_id: Option<uuid::Uuid> = sqlx::query_scalar(
                "SELECT folioharbor.job_attach_upload_origin_authorized($1,$2,$3,$4,$5)",
            )
            .bind(receipt.upload_id.as_uuid())
            .bind(receipt.library_id.as_uuid())
            .bind(receipt.actor.as_uuid())
            .bind(&request_id)
            .bind(receipt.traceparent.as_deref())
            .fetch_one(&mut *transaction)
            .await
            .map_err(persistence_error)?;
            let job_id = job_id.ok_or(UploadRepositoryError::Persistence)?;
            let trace_id = receipt
                .traceparent
                .as_deref()
                .map(|traceparent| &traceparent[3..35]);
            tracing::info!(
                job_id = %job_id,
                request_id = %request_id,
                trace_id,
                "import job enqueued"
            );
        }
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
            sqlx::query_scalar("SELECT folioharbor.upload_heartbeat_authorized($1,$2,$3,$4,$5,$6)")
                .bind(receipt.upload_id.as_uuid())
                .bind(receipt.library_id.as_uuid())
                .bind(receipt.actor.as_uuid())
                .bind(attempt_uuid(&receipt.attempt_token)?)
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
            "SELECT folioharbor.upload_prepare_promotion_authorized($1,$2,$3,$4,$5,$6,$7,$8,$9)",
        )
        .bind(promotion.upload_id.as_uuid())
        .bind(promotion.library_id.as_uuid())
        .bind(promotion.actor.as_uuid())
        .bind(attempt_uuid(&promotion.attempt_token)?)
        .bind(promotion.staging_key)
        .bind(promotion.final_key)
        .bind(promotion.digest.as_bytes().to_vec())
        .bind(i64::try_from(promotion.received.get()).map_err(|_| UploadRepositoryError::Invalid)?)
        .bind(promotion.now)
        .fetch_one(&mut *transaction)
        .await
        .map_err(persistence_error)?;
        transaction.commit().await.map_err(persistence_error)?;
        Ok(changed)
    }

    async fn record_promotion_disposition(
        &self,
        promotion: RecordPromotionDisposition,
    ) -> Result<bool, UploadRepositoryError> {
        let disposition = match promotion.disposition {
            BlobDisposition::Installed => "installed",
            BlobDisposition::Reused => "reused",
        };
        let mut transaction = self
            .transaction(promotion.actor, promotion.library_id, promotion.request_id)
            .await?;
        let changed: bool = sqlx::query_scalar(
            "SELECT folioharbor.upload_record_promotion_disposition_authorized($1,$2,$3,$4,$5,$6,$7,$8)",
        )
        .bind(promotion.upload_id.as_uuid())
        .bind(promotion.library_id.as_uuid())
        .bind(promotion.actor.as_uuid())
        .bind(attempt_uuid(&promotion.attempt_token)?)
        .bind(promotion.staging_key)
        .bind(promotion.final_key)
        .bind(disposition)
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
            "SELECT folioharbor.upload_mark_received_authorized($1,$2,$3,$4,$5,$6,$7,$8)",
        )
        .bind(receipt.upload_id.as_uuid())
        .bind(receipt.library_id.as_uuid())
        .bind(receipt.actor.as_uuid())
        .bind(attempt_uuid(&receipt.attempt_token)?)
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
        sqlx::query(
            "SELECT folioharbor.upload_record_orphan_cleanup_authorized($1,$2,$3,$4,$5,$6)",
        )
        .bind(cleanup.upload_id.as_uuid())
        .bind(cleanup.library_id.as_uuid())
        .bind(cleanup.actor.as_uuid())
        .bind(attempt_uuid(&cleanup.attempt_token)?)
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

    async fn claim_cleanup(
        &self,
        request: ClaimUploadCleanup,
    ) -> Result<Option<Box<dyn UploadCleanupGuard>>, UploadRepositoryError> {
        let mut connection = self.pool.acquire().await.map_err(persistence_error)?;
        sqlx::query("BEGIN")
            .execute(&mut *connection)
            .await
            .map_err(persistence_error)?;
        if let Err(error) = PgTransactionContext::apply(
            &mut connection,
            &DatabaseContext::worker(request.request_id, None),
        )
        .await
        {
            connection.close_on_drop();
            return Err(persistence_error(error));
        }
        let row = sqlx::query_as::<_, (uuid::Uuid, uuid::Uuid, String, Option<String>, bool)>(
                "SELECT upload_id,attempt_token,staging_key,final_key,final_owned FROM folioharbor.upload_cleanups WHERE state='pending' ORDER BY created_at FOR UPDATE SKIP LOCKED LIMIT 1",
            )
            .fetch_optional(&mut *connection)
            .await;
        let row = match row {
            Ok(row) => row,
            Err(error) => {
                connection.close_on_drop();
                return Err(persistence_error(error));
            }
        };
        let Some(row) = row else {
            sqlx::query("ROLLBACK")
                .execute(&mut *connection)
                .await
                .map_err(persistence_error)?;
            return Ok(None);
        };
        let _ = (&request.owner, request.now);
        Ok(Some(Box::new(PgUploadCleanupGuard {
            connection: Some(connection),
            cleanup: UploadCleanup {
                upload_id: UploadId::from_uuid(row.0),
                attempt_token: row.1.to_string(),
                staging_key: row.2,
                final_key: row.3,
                final_owned: row.4,
            },
        })))
    }
}
