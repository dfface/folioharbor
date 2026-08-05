use crate::{
    error::AppError,
    ports::{AuthorizedUploadTransition, BlobStoreError, JobRepositoryError},
};
use async_trait::async_trait;
use folioharbor_domain::{
    id::{JobId, UploadId},
    imports::{
        blob::{BlobIdentity, DedupScope, Sha256Digest, StorageNamespace},
        job::{JobInput, JobKind},
        quota::ByteCount,
        upload::{UploadSession, UploadState},
    },
};
use futures_util::StreamExt as _;
use sha2::{Digest as _, Sha256};

use super::{
    CreateUploadRequest, GetUploadRequest, ReceiveUploadRequest, UploadApi, UploadService,
};

const MAX_APPEND_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy)]
struct ReceiptContext {
    actor: folioharbor_domain::id::UserId,
    library_id: folioharbor_domain::id::LibraryId,
    upload_id: UploadId,
    request_id: folioharbor_domain::id::RequestId,
}

#[async_trait]
impl UploadApi for UploadService {
    async fn create_upload(&self, request: CreateUploadRequest) -> Result<UploadSession, AppError> {
        self.create(request).await
    }

    async fn get_upload(&self, request: GetUploadRequest) -> Result<UploadSession, AppError> {
        self.get(request).await
    }

    async fn receive_upload(
        &self,
        mut request: ReceiveUploadRequest,
    ) -> Result<UploadSession, AppError> {
        let current = self
            .get_upload(GetUploadRequest {
                actor: request.actor,
                request_id: request.request_id,
                library_id: request.library_id,
                upload_id: request.upload_id,
            })
            .await?;
        let from = match current.state {
            UploadState::Created | UploadState::Failed => current.state,
            _ => {
                return Err(AppError::Conflict {
                    code: "upload_state_conflict",
                });
            }
        };
        let staging = self
            .blobs
            .create_staging()
            .await
            .map_err(|error| storage_error(&error))?;
        if let Err(error) = self
            .apply_transition(AuthorizedUploadTransition {
                actor: request.actor,
                library_id: request.library_id,
                upload_id: request.upload_id,
                from,
                to: UploadState::Receiving,
                received: ByteCount::new(0),
                storage_key: Some(staging.as_str().to_owned()),
                error_code: None,
                request_id: request.request_id,
                now: self.clock.now(),
            })
            .await
        {
            let _ = self.blobs.delete(&staging).await;
            return Err(error);
        }
        let context = ReceiptContext {
            actor: request.actor,
            library_id: request.library_id,
            upload_id: request.upload_id,
            request_id: request.request_id,
        };
        let (received, digest) = self
            .stream_content(&mut request, context, &staging, current.declared_bytes)
            .await?;
        let stored = self.promote(context, &staging, received, digest).await?;
        self.apply_transition(AuthorizedUploadTransition {
            actor: request.actor,
            library_id: request.library_id,
            upload_id: request.upload_id,
            from: UploadState::Receiving,
            to: UploadState::Received,
            received: ByteCount::new(received),
            storage_key: Some(stored.as_str().to_owned()),
            error_code: None,
            request_id: request.request_id,
            now: self.clock.now(),
        })
        .await?;
        self.jobs
            .enqueue(
                JobId::new(),
                request.library_id,
                JobKind::ImportEpub,
                JobInput::upload_v1(request.upload_id.as_uuid().to_string()),
                &format!("import:{}", request.upload_id.as_uuid()),
                self.clock.now(),
            )
            .await
            .map_err(job_error)?;
        self.apply_transition(AuthorizedUploadTransition {
            actor: request.actor,
            library_id: request.library_id,
            upload_id: request.upload_id,
            from: UploadState::Received,
            to: UploadState::Queued,
            received: ByteCount::new(received),
            storage_key: Some(stored.as_str().to_owned()),
            error_code: None,
            request_id: request.request_id,
            now: self.clock.now(),
        })
        .await?;
        self.get_upload(GetUploadRequest {
            actor: request.actor,
            request_id: request.request_id,
            library_id: request.library_id,
            upload_id: request.upload_id,
        })
        .await
    }
}

impl UploadService {
    async fn promote(
        &self,
        context: ReceiptContext,
        staging: &folioharbor_domain::imports::blob::StorageKey,
        received: u64,
        digest: Sha256Digest,
    ) -> Result<folioharbor_domain::imports::blob::StorageKey, AppError> {
        let identity = BlobIdentity::new(
            StorageNamespace::for_scope(
                DedupScope::Instance,
                context.library_id,
                context.upload_id,
            ),
            digest,
            ByteCount::new(received),
        );
        match self.blobs.promote(staging, &identity).await {
            Ok(value) => Ok(value),
            Err(error) => {
                let _ = self
                    .abort(context, staging, received, "upload_storage_failed")
                    .await;
                Err(storage_error(&error))
            }
        }
    }

    async fn apply_transition(
        &self,
        transition: AuthorizedUploadTransition,
    ) -> Result<(), AppError> {
        match self.uploads.transition_authorized(transition).await {
            Ok(true) => Ok(()),
            Ok(false) => Err(AppError::Conflict {
                code: "upload_state_conflict",
            }),
            Err(_) => Err(dependency()),
        }
    }
    async fn stream_content(
        &self,
        request: &mut ReceiveUploadRequest,
        context: ReceiptContext,
        staging: &folioharbor_domain::imports::blob::StorageKey,
        declared: ByteCount,
    ) -> Result<(u64, Sha256Digest), AppError> {
        let mut received = 0_u64;
        let mut digest = Sha256::new();
        while let Some(next) = request.bytes.next().await {
            let Ok(bytes) = next else {
                return Err(self
                    .abort(context, staging, received, "upload_interrupted")
                    .await);
            };
            for chunk in bytes.chunks(MAX_APPEND_BYTES) {
                let Some(total) =
                    received.checked_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX))
                else {
                    return Err(self
                        .abort(context, staging, received, "upload_exceeds_declared_size")
                        .await);
                };
                if total > declared.get() {
                    return Err(self
                        .abort(context, staging, received, "upload_exceeds_declared_size")
                        .await);
                }
                received = total;
                if let Err(error) = self.blobs.append(staging, chunk).await {
                    let _ = self
                        .abort(context, staging, received, "upload_storage_failed")
                        .await;
                    return Err(storage_error(&error));
                }
                digest.update(chunk);
            }
        }
        Ok((received, Sha256Digest::from_bytes(digest.finalize().into())))
    }

    async fn abort(
        &self,
        context: ReceiptContext,
        staging: &folioharbor_domain::imports::blob::StorageKey,
        received: u64,
        code: &'static str,
    ) -> AppError {
        let _ = self.blobs.delete(staging).await;
        let transition = self
            .uploads
            .transition_authorized(AuthorizedUploadTransition {
                actor: context.actor,
                library_id: context.library_id,
                upload_id: context.upload_id,
                from: UploadState::Receiving,
                to: UploadState::Failed,
                received: ByteCount::new(received),
                storage_key: None,
                error_code: Some(code.to_owned()),
                request_id: context.request_id,
                now: self.clock.now(),
            })
            .await
            .map_err(|_| dependency());
        match transition {
            Ok(_) => AppError::Invalid {
                code,
                fields: Vec::new(),
            },
            Err(error) => error,
        }
    }
}

fn dependency() -> AppError {
    AppError::DependencyUnavailable {
        code: "upload_repository_unavailable",
    }
}
fn job_error(_: JobRepositoryError) -> AppError {
    AppError::DependencyUnavailable {
        code: "job_repository_unavailable",
    }
}
fn storage_error(error: &BlobStoreError) -> AppError {
    match error {
        BlobStoreError::InsufficientCapacity => AppError::StorageExhausted,
        BlobStoreError::InvalidKey
        | BlobStoreError::IdentityMismatch
        | BlobStoreError::InvalidRange
        | BlobStoreError::Io(_) => AppError::DependencyUnavailable {
            code: "blob_store_unavailable",
        },
    }
}
