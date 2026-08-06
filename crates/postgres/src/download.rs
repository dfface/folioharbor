use async_trait::async_trait;
use folioharbor_application::{
    actor::Actor,
    catalog::{
        DownloadAuthorization, DownloadRange, DownloadRepository, DownloadRepositoryError,
        DownloadSource,
    },
};
use folioharbor_domain::{
    id::{BlobId, ItemId, RequestId},
    imports::blob::StorageKey,
};
use sqlx::{PgPool, Row as _};

use crate::{DatabaseContext, PgTransactionContext};

#[derive(Clone, Debug)]
pub struct PgDownloadRepository {
    pool: PgPool,
}

impl PgDownloadRepository {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl DownloadRepository for PgDownloadRepository {
    async fn authorize_download(
        &self,
        actor: Actor,
        item_id: ItemId,
        request_id: RequestId,
    ) -> Result<DownloadAuthorization, DownloadRepositoryError> {
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| DownloadRepositoryError)?;
        PgTransactionContext::apply(
            &mut transaction,
            &DatabaseContext::api_without_library(actor.user_id, request_id),
        )
        .await
        .map_err(|_| DownloadRepositoryError)?;
        let row = sqlx::query(
            "SELECT outcome,library_id,item_id,blob_id,storage_key,byte_size,file_name FROM folioharbor.download_item_authorize($1,$2,$3)",
        )
        .bind(actor.user_id.as_uuid())
        .bind(item_id.as_uuid())
        .bind(request_id.as_ulid().to_string())
        .fetch_one(&mut *transaction)
        .await
        .map_err(|_| DownloadRepositoryError)?;
        transaction
            .commit()
            .await
            .map_err(|_| DownloadRepositoryError)?;
        match row.try_get::<String, _>("outcome").as_deref() {
            Ok("granted") => Ok(DownloadAuthorization::Granted(DownloadSource::new(
                BlobId::from_uuid(
                    row.try_get("blob_id")
                        .map_err(|_| DownloadRepositoryError)?,
                ),
                StorageKey::from_opaque(
                    row.try_get("storage_key")
                        .map_err(|_| DownloadRepositoryError)?,
                ),
                u64::try_from(
                    row.try_get::<i64, _>("byte_size")
                        .map_err(|_| DownloadRepositoryError)?,
                )
                .map_err(|_| DownloadRepositoryError)?,
                row.try_get("file_name")
                    .map_err(|_| DownloadRepositoryError)?,
            ))),
            Ok("forbidden") => Ok(DownloadAuthorization::Forbidden),
            Ok("not_found") => Ok(DownloadAuthorization::NotFound),
            _ => Err(DownloadRepositoryError),
        }
    }

    async fn record_download_start(
        &self,
        actor: Actor,
        item_id: ItemId,
        request_id: RequestId,
        range: DownloadRange,
    ) -> Result<bool, DownloadRepositoryError> {
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|_| DownloadRepositoryError)?;
        PgTransactionContext::apply(
            &mut transaction,
            &DatabaseContext::api_without_library(actor.user_id, request_id),
        )
        .await
        .map_err(|_| DownloadRepositoryError)?;
        let recorded: bool = sqlx::query_scalar(
            "SELECT folioharbor.download_record_start($1,$2,$3,$4,$5,clock_timestamp())",
        )
        .bind(actor.user_id.as_uuid())
        .bind(item_id.as_uuid())
        .bind(request_id.as_ulid().to_string())
        .bind(i64::try_from(range.start).map_err(|_| DownloadRepositoryError)?)
        .bind(i64::try_from(range.end).map_err(|_| DownloadRepositoryError)?)
        .fetch_one(&mut *transaction)
        .await
        .map_err(|_| DownloadRepositoryError)?;
        transaction
            .commit()
            .await
            .map_err(|_| DownloadRepositoryError)?;
        Ok(recorded)
    }
}
