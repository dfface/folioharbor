use async_trait::async_trait;
use folioharbor_application::ports::{QuotaOutcome, QuotaRepository, QuotaRepositoryError};
use folioharbor_domain::{
    id::{LibraryId, RequestId, UploadId},
    imports::quota::ByteCount,
    time::OffsetDateTime,
};
use sqlx::PgPool;

use crate::{DatabaseContext, PgTransactionContext};

#[derive(Clone, Debug)]
pub struct PgQuotaRepository {
    pool: PgPool,
}

impl PgQuotaRepository {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn mutate(
        &self,
        library_id: LibraryId,
        operation: QuotaOperation,
    ) -> Result<QuotaOutcome, QuotaRepositoryError> {
        let mut transaction = self.pool.begin().await.map_err(persistence_error)?;
        PgTransactionContext::apply(
            &mut transaction,
            &DatabaseContext::worker(RequestId::new(), Some(library_id)),
        )
        .await
        .map_err(persistence_error)?;
        let outcome: String = match operation {
            QuotaOperation::Reserve(upload, bytes, expires) => {
                sqlx::query_scalar!(
                    r#"SELECT folioharbor.quota_reserve($1,$2,$3,$4) AS "outcome!""#,
                    library_id.as_uuid(),
                    upload.as_uuid(),
                    to_i64(bytes)?,
                    expires,
                )
                .fetch_one(&mut *transaction)
                .await
            }
            QuotaOperation::Resize(upload, bytes) => {
                sqlx::query_scalar!(
                    r#"SELECT folioharbor.quota_resize($1,$2,$3) AS "outcome!""#,
                    library_id.as_uuid(),
                    upload.as_uuid(),
                    to_i64(bytes)?,
                )
                .fetch_one(&mut *transaction)
                .await
            }
            QuotaOperation::Consume(upload) => {
                sqlx::query_scalar!(
                    r#"SELECT folioharbor.quota_consume($1,$2) AS "outcome!""#,
                    library_id.as_uuid(),
                    upload.as_uuid(),
                )
                .fetch_one(&mut *transaction)
                .await
            }
            QuotaOperation::Release(upload) => {
                sqlx::query_scalar!(
                    r#"SELECT folioharbor.quota_release($1,$2) AS "outcome!""#,
                    library_id.as_uuid(),
                    upload.as_uuid(),
                )
                .fetch_one(&mut *transaction)
                .await
            }
        }
        .map_err(persistence_error)?;
        transaction.commit().await.map_err(persistence_error)?;
        match outcome.as_str() {
            "applied" => Ok(QuotaOutcome::Applied),
            "exceeded" => Ok(QuotaOutcome::Exceeded),
            "not_active" => Ok(QuotaOutcome::NotActive),
            _ => Err(QuotaRepositoryError),
        }
    }
}

enum QuotaOperation {
    Reserve(UploadId, ByteCount, OffsetDateTime),
    Resize(UploadId, ByteCount),
    Consume(UploadId),
    Release(UploadId),
}

fn to_i64(bytes: ByteCount) -> Result<i64, QuotaRepositoryError> {
    i64::try_from(bytes.get()).map_err(|_| QuotaRepositoryError)
}

fn persistence_error(_: sqlx::Error) -> QuotaRepositoryError {
    QuotaRepositoryError
}

#[async_trait]
impl QuotaRepository for PgQuotaRepository {
    async fn reserve(
        &self,
        library_id: LibraryId,
        upload_id: UploadId,
        bytes: ByteCount,
        expires_at: OffsetDateTime,
    ) -> Result<QuotaOutcome, QuotaRepositoryError> {
        self.mutate(
            library_id,
            QuotaOperation::Reserve(upload_id, bytes, expires_at),
        )
        .await
    }

    async fn resize_reservation(
        &self,
        library_id: LibraryId,
        upload_id: UploadId,
        bytes: ByteCount,
    ) -> Result<QuotaOutcome, QuotaRepositoryError> {
        self.mutate(library_id, QuotaOperation::Resize(upload_id, bytes))
            .await
    }

    async fn consume(
        &self,
        library_id: LibraryId,
        upload_id: UploadId,
    ) -> Result<QuotaOutcome, QuotaRepositoryError> {
        self.mutate(library_id, QuotaOperation::Consume(upload_id))
            .await
    }

    async fn release(
        &self,
        library_id: LibraryId,
        upload_id: UploadId,
    ) -> Result<QuotaOutcome, QuotaRepositoryError> {
        self.mutate(library_id, QuotaOperation::Release(upload_id))
            .await
    }
}
