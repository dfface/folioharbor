use crate::ports::BlobDisposition;
use async_trait::async_trait;
use folioharbor_domain::{
    id::{JobId, LibraryId, RequestId, UploadId, UserId},
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
    pub dedup_scope: folioharbor_domain::imports::blob::DedupScope,
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
    pub attempt_token: Option<String>,
    pub storage_key: Option<String>,
    pub error_code: Option<String>,
    pub request_id: RequestId,
    pub now: OffsetDateTime,
}
pub struct BeginUploadReceipt {
    pub actor: UserId,
    pub library_id: LibraryId,
    pub upload_id: UploadId,
    pub from: UploadState,
    pub request_id: RequestId,
    pub now: OffsetDateTime,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UploadReceiptAttempt {
    pub attempt_token: String,
    pub staging_key: String,
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
pub struct FinalizeUploadReceipt {
    pub actor: UserId,
    pub library_id: LibraryId,
    pub upload_id: UploadId,
    pub received: ByteCount,
    pub storage_key: String,
    pub staging_key: Option<String>,
    pub job_id: JobId,
    pub request_id: RequestId,
    pub now: OffsetDateTime,
}
pub struct HeartbeatUploadReceipt {
    pub actor: UserId,
    pub library_id: LibraryId,
    pub upload_id: UploadId,
    pub attempt_token: String,
    pub staging_key: String,
    pub request_id: RequestId,
    pub now: OffsetDateTime,
}
pub struct PrepareUploadPromotion {
    pub actor: UserId,
    pub library_id: LibraryId,
    pub upload_id: UploadId,
    pub attempt_token: String,
    pub staging_key: String,
    pub final_key: String,
    pub digest: folioharbor_domain::imports::blob::Sha256Digest,
    pub received: ByteCount,
    pub request_id: RequestId,
    pub now: OffsetDateTime,
}
pub struct RecordPromotionDisposition {
    pub actor: UserId,
    pub library_id: LibraryId,
    pub upload_id: UploadId,
    pub attempt_token: String,
    pub staging_key: String,
    pub final_key: String,
    pub disposition: BlobDisposition,
    pub request_id: RequestId,
    pub now: OffsetDateTime,
}
pub struct MarkUploadReceived {
    pub actor: UserId,
    pub library_id: LibraryId,
    pub upload_id: UploadId,
    pub attempt_token: String,
    pub staging_key: String,
    pub final_key: String,
    pub received: ByteCount,
    pub request_id: RequestId,
    pub now: OffsetDateTime,
}
pub struct RecordUploadCleanup {
    pub actor: UserId,
    pub library_id: LibraryId,
    pub upload_id: UploadId,
    pub attempt_token: String,
    pub staging_key: String,
    pub request_id: RequestId,
    pub now: OffsetDateTime,
}
pub struct ClaimUploadCleanup {
    pub owner: String,
    pub now: OffsetDateTime,
    pub request_id: RequestId,
}
#[derive(Clone, Debug)]
pub struct UploadCleanup {
    pub upload_id: UploadId,
    pub attempt_token: String,
    pub staging_key: String,
    pub final_key: Option<String>,
    pub final_owned: bool,
}

#[async_trait]
pub trait UploadCleanupGuard: Send {
    fn cleanup(&self) -> &UploadCleanup;
    async fn complete(self: Box<Self>, now: OffsetDateTime) -> Result<bool, UploadRepositoryError>;
    async fn abandon(self: Box<Self>) -> Result<(), UploadRepositoryError>;
}
pub struct ExpireUploads {
    pub now: OffsetDateTime,
    pub limit: u32,
    pub request_id: RequestId,
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
    async fn begin_receipt(
        &self,
        _: BeginUploadReceipt,
    ) -> Result<Option<UploadReceiptAttempt>, UploadRepositoryError> {
        Err(UploadRepositoryError::Persistence)
    }
    async fn transition_authorized(
        &self,
        transition: AuthorizedUploadTransition,
    ) -> Result<bool, UploadRepositoryError>;
    async fn finalize_authorized(
        &self,
        receipt: FinalizeUploadReceipt,
    ) -> Result<bool, UploadRepositoryError>;
    async fn heartbeat_receipt(
        &self,
        _: HeartbeatUploadReceipt,
    ) -> Result<bool, UploadRepositoryError> {
        Err(UploadRepositoryError::Persistence)
    }
    async fn prepare_promotion(
        &self,
        _: PrepareUploadPromotion,
    ) -> Result<bool, UploadRepositoryError> {
        Err(UploadRepositoryError::Persistence)
    }
    async fn record_promotion_disposition(
        &self,
        _: RecordPromotionDisposition,
    ) -> Result<bool, UploadRepositoryError> {
        Err(UploadRepositoryError::Persistence)
    }
    async fn mark_received(&self, _: MarkUploadReceived) -> Result<bool, UploadRepositoryError> {
        Err(UploadRepositoryError::Persistence)
    }
    async fn record_orphan_cleanup(
        &self,
        _: RecordUploadCleanup,
    ) -> Result<(), UploadRepositoryError> {
        Err(UploadRepositoryError::Persistence)
    }
    async fn transition_worker(
        &self,
        transition: WorkerUploadTransition,
    ) -> Result<bool, UploadRepositoryError>;
    async fn expire_worker(&self, request: ExpireUploads) -> Result<u64, UploadRepositoryError>;
    async fn claim_cleanup(
        &self,
        _: ClaimUploadCleanup,
    ) -> Result<Option<Box<dyn UploadCleanupGuard>>, UploadRepositoryError> {
        Err(UploadRepositoryError::Persistence)
    }
}
