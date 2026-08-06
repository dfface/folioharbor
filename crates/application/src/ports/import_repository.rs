use async_trait::async_trait;
use folioharbor_domain::{
    id::{BlobId, LibraryId, RequestId, UploadId, UserId},
    imports::{blob::StorageKey, quota::ByteCount, upload::UploadState},
    time::OffsetDateTime,
};
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportWork {
    pub upload_id: UploadId,
    pub library_id: LibraryId,
    pub actor_id: UserId,
    pub blob_id: BlobId,
    pub logical_bytes: ByteCount,
    pub storage_key: StorageKey,
    pub state: UploadState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImportReconciliation {
    Work(ImportWork),
    Complete,
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ImportRepositoryError {
    #[error("import state is invalid")]
    InvalidState,
    #[error("import persistence is temporarily unavailable")]
    Unavailable,
    #[error("import schema is incompatible")]
    Schema,
}

#[async_trait]
pub trait ImportRepository: Send + Sync {
    async fn reconcile(
        &self,
        upload_id: UploadId,
        library_id: LibraryId,
        request_id: RequestId,
        now: OffsetDateTime,
    ) -> Result<ImportReconciliation, ImportRepositoryError>;
    async fn begin_catalog(
        &self,
        work: &ImportWork,
        request_id: RequestId,
        now: OffsetDateTime,
    ) -> Result<(), ImportRepositoryError>;
    async fn record_failure(
        &self,
        work: &ImportWork,
        to: UploadState,
        code: &'static str,
        request_id: RequestId,
        now: OffsetDateTime,
    ) -> Result<(), ImportRepositoryError>;
}
