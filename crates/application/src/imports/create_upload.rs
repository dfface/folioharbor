use std::{pin::Pin, sync::Arc};

use async_trait::async_trait;
use bytes::Bytes;
use folioharbor_domain::{
    id::{LibraryId, RequestId, UploadId, UserId},
    imports::{blob::DedupScope, quota::ByteCount, upload::UploadSession},
};
use futures_util::Stream;
use time::Duration;

use crate::{
    authorization::{Action, Authorization, ResourceRef},
    error::{AppError, FieldViolation},
    ports::{
        AuthorizationRepository, BlobStore, Clock, CreateUploadRecord, UploadRepository,
        UploadRepositoryError,
    },
};

#[derive(Clone, Debug)]
pub struct CreateUploadRequest {
    pub actor: UserId,
    pub request_id: RequestId,
    pub library_id: LibraryId,
    pub file_name: String,
    pub media_type: String,
    pub declared_bytes: u64,
}
#[derive(Clone, Copy, Debug)]
pub struct GetUploadRequest {
    pub actor: UserId,
    pub request_id: RequestId,
    pub library_id: LibraryId,
    pub upload_id: UploadId,
}
pub struct ReceiveUploadRequest {
    pub actor: UserId,
    pub request_id: RequestId,
    pub library_id: LibraryId,
    pub upload_id: UploadId,
    pub bytes: UploadByteStream,
}

#[derive(Debug)]
pub struct UploadStreamError;
pub type UploadByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, UploadStreamError>> + Send>>;

#[async_trait]
pub trait UploadApi: Send + Sync {
    async fn create_upload(&self, request: CreateUploadRequest) -> Result<UploadSession, AppError>;
    async fn receive_upload(
        &self,
        request: ReceiveUploadRequest,
    ) -> Result<UploadSession, AppError>;
    async fn get_upload(&self, request: GetUploadRequest) -> Result<UploadSession, AppError>;
}

pub struct UploadService {
    pub(crate) uploads: Arc<dyn UploadRepository>,
    pub(crate) authorization: Arc<dyn AuthorizationRepository>,
    pub(crate) blobs: Arc<dyn BlobStore>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) dedup_scope: DedupScope,
    pub(crate) receipt_heartbeat_interval: std::time::Duration,
}

impl UploadService {
    #[must_use]
    pub fn new(
        uploads: Arc<dyn UploadRepository>,
        authorization: Arc<dyn AuthorizationRepository>,
        blobs: Arc<dyn BlobStore>,
        clock: Arc<dyn Clock>,
        dedup_scope: DedupScope,
    ) -> Self {
        Self {
            uploads,
            authorization,
            blobs,
            clock,
            dedup_scope,
            receipt_heartbeat_interval: std::time::Duration::from_secs(60),
        }
    }

    /// Overrides how often an active receipt lease is renewed while waiting for body data.
    ///
    /// # Panics
    ///
    /// Panics when `interval` is zero because a zero-duration Tokio interval is invalid.
    #[must_use]
    pub fn with_receipt_heartbeat_interval(mut self, interval: std::time::Duration) -> Self {
        assert!(
            !interval.is_zero(),
            "receipt heartbeat interval must be positive"
        );
        self.receipt_heartbeat_interval = interval;
        self
    }

    pub(crate) async fn create(
        &self,
        request: CreateUploadRequest,
    ) -> Result<UploadSession, AppError> {
        if !request.file_name.to_ascii_lowercase().ends_with(".epub") {
            return Err(invalid("file_name", "epub_filename_required"));
        }
        if !matches!(
            request.media_type.as_str(),
            "application/epub+zip" | "application/octet-stream"
        ) {
            return Err(invalid("media_type", "unsupported_upload_media_type"));
        }
        let bytes = declared_size(request.declared_bytes)?;
        Authorization::new(self.authorization.as_ref())
            .require(
                request.actor,
                Action::CreateUpload,
                ResourceRef::Library(request.library_id),
            )
            .await?;
        let now = self.clock.now();
        self.uploads
            .create_authorized(CreateUploadRecord {
                upload_id: UploadId::new(),
                library_id: request.library_id,
                actor: request.actor,
                request_id: request.request_id,
                file_name: request.file_name,
                media_type: request.media_type,
                declared_bytes: bytes,
                dedup_scope: self.dedup_scope,
                expires_at: now + Duration::hours(24),
                now,
            })
            .await
            .map_err(create_error)
    }

    pub(crate) async fn get(&self, request: GetUploadRequest) -> Result<UploadSession, AppError> {
        Authorization::new(self.authorization.as_ref())
            .require(
                request.actor,
                Action::InspectUpload,
                ResourceRef::Library(request.library_id),
            )
            .await?;
        self.uploads
            .find_authorized(
                request.actor,
                request.library_id,
                request.upload_id,
                request.request_id,
            )
            .await
            .map_err(|_| dependency())?
            .ok_or(AppError::NotFound {
                code: "upload_not_found",
            })
    }
}

pub struct UnavailableUploadApi;
#[async_trait]
impl UploadApi for UnavailableUploadApi {
    async fn create_upload(&self, _: CreateUploadRequest) -> Result<UploadSession, AppError> {
        unavailable()
    }
    async fn receive_upload(&self, _: ReceiveUploadRequest) -> Result<UploadSession, AppError> {
        unavailable()
    }
    async fn get_upload(&self, _: GetUploadRequest) -> Result<UploadSession, AppError> {
        unavailable()
    }
}
fn unavailable<T>() -> Result<T, AppError> {
    Err(AppError::DependencyUnavailable {
        code: "upload_service_unavailable",
    })
}

pub const MAX_UPLOAD_BYTES: u64 = 1024 * 1024 * 1024;
pub(crate) fn declared_size(value: u64) -> Result<ByteCount, AppError> {
    if value == 0 || value > MAX_UPLOAD_BYTES {
        return Err(AppError::PayloadTooLarge);
    }
    Ok(ByteCount::new(value))
}
fn invalid(field: &'static str, code: &'static str) -> AppError {
    AppError::Invalid {
        code,
        fields: vec![FieldViolation { field, code }],
    }
}
fn dependency() -> AppError {
    AppError::DependencyUnavailable {
        code: "upload_repository_unavailable",
    }
}
fn create_error(error: UploadRepositoryError) -> AppError {
    match error {
        UploadRepositoryError::QuotaExceeded => AppError::Conflict {
            code: "library_quota_exceeded",
        },
        UploadRepositoryError::Conflict => AppError::Conflict {
            code: "upload_already_exists",
        },
        UploadRepositoryError::Invalid => invalid("upload", "invalid_upload"),
        UploadRepositoryError::Forbidden => AppError::Forbidden {
            code: "library_action_forbidden",
        },
        UploadRepositoryError::NotFound => AppError::NotFound {
            code: "library_not_found",
        },
        UploadRepositoryError::Persistence => dependency(),
    }
}
