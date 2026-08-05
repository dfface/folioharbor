use async_trait::async_trait;
use folioharbor_domain::{
    id::{LibraryId, RequestId, UploadId, UserId},
    imports::{
        quota::ByteCount,
        upload::{UploadSession, UploadState},
    },
    time::OffsetDateTime,
};
use thiserror::Error;

pub struct CreateUploadRecord {
    pub upload_id: UploadId,
    pub library_id: LibraryId,
    pub actor: UserId,
    pub request_id: RequestId,
    pub file_name: String,
    pub media_type: String,
    pub declared_bytes: ByteCount,
    pub expires_at: OffsetDateTime,
    pub now: OffsetDateTime,
}
pub struct AuthorizedUploadTransition {
    pub actor: UserId,
    pub library_id: LibraryId,
    pub upload_id: UploadId,
    pub from: UploadState,
    pub to: UploadState,
    pub received: ByteCount,
    pub storage_key: Option<String>,
    pub error_code: Option<String>,
    pub request_id: RequestId,
    pub now: OffsetDateTime,
}
pub struct WorkerUploadTransition {
    pub library_id: LibraryId,
    pub upload_id: UploadId,
    pub from: UploadState,
    pub to: UploadState,
    pub error_code: Option<String>,
    pub request_id: RequestId,
    pub now: OffsetDateTime,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CreateUploadOutcome {
    Created,
    Forbidden,
    NotFound,
    QuotaExceeded,
    Conflict,
}

#[derive(Clone, Copy, Debug, Error)]
pub enum UploadRepositoryError {
    #[error("upload persistence failed")]
    Persistence,
    #[error("upload quota exceeded")]
    QuotaExceeded,
    #[error("upload already exists")]
    Conflict,
    #[error("upload request invalid")]
    Invalid,
    #[error("upload forbidden")]
    Forbidden,
    #[error("upload not found")]
    NotFound,
}

#[async_trait]
pub trait UploadRepository: Send + Sync {
    async fn create_authorized(
        &self,
        record: CreateUploadRecord,
    ) -> Result<UploadSession, UploadRepositoryError>;
    async fn find_authorized(
        &self,
        actor: UserId,
        library: LibraryId,
        upload: UploadId,
        request: RequestId,
    ) -> Result<Option<UploadSession>, UploadRepositoryError>;
    async fn transition_authorized(
        &self,
        transition: AuthorizedUploadTransition,
    ) -> Result<bool, UploadRepositoryError>;
    async fn transition_worker(
        &self,
        transition: WorkerUploadTransition,
    ) -> Result<bool, UploadRepositoryError>;
}
