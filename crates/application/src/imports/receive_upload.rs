use crate::{
    error::AppError,
    ports::{
        AuthorizedUploadTransition, BlobStoreError, FinalizeUploadReceipt, HeartbeatUploadReceipt,
        MarkUploadReceived, PrepareUploadPromotion, RecordUploadCleanup,
    },
};
use async_trait::async_trait;
use folioharbor_domain::{
    id::{JobId, UploadId},
    imports::{
        blob::{BlobIdentity, DedupScope, Sha256Digest, StorageNamespace},
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
            UploadState::Queued => return Ok(current),
            UploadState::Received => return self.finalize_received(&current, request).await,
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
            if self.blobs.delete(&staging).await.is_err() {
                let _ = self
                    .uploads
                    .record_orphan_cleanup(RecordUploadCleanup {
                        actor: request.actor,
                        library_id: request.library_id,
                        upload_id: request.upload_id,
                        staging_key: staging.as_str().to_owned(),
                        request_id: request.request_id,
                        now: self.clock.now(),
                    })
                    .await;
            }
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
        let marked = self
            .uploads
            .mark_received(MarkUploadReceived {
                actor: context.actor,
                library_id: context.library_id,
                upload_id: context.upload_id,
                staging_key: staging.as_str().to_owned(),
                final_key: stored.as_str().to_owned(),
                received: ByteCount::new(received),
                request_id: context.request_id,
                now: self.clock.now(),
            })
            .await
            .map_err(|_| dependency())?;
        if !marked {
            return Err(AppError::Conflict {
                code: "upload_state_conflict",
            });
        }
        self.finalize(context, received, stored.as_str().to_owned(), None)
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
    async fn finalize_received(
        &self,
        current: &UploadSession,
        request: ReceiveUploadRequest,
    ) -> Result<UploadSession, AppError> {
        let storage = current.storage_key.as_ref().ok_or_else(dependency)?;
        self.finalize(
            ReceiptContext {
                actor: request.actor,
                library_id: request.library_id,
                upload_id: request.upload_id,
                request_id: request.request_id,
            },
            current.received_bytes.get(),
            storage.as_str().to_owned(),
            None,
        )
        .await?;
        self.get_upload(GetUploadRequest {
            actor: request.actor,
            request_id: request.request_id,
            library_id: request.library_id,
            upload_id: request.upload_id,
        })
        .await
    }

    async fn finalize(
        &self,
        context: ReceiptContext,
        received: u64,
        storage_key: String,
        staging_key: Option<String>,
    ) -> Result<(), AppError> {
        match self
            .uploads
            .finalize_authorized(FinalizeUploadReceipt {
                actor: context.actor,
                library_id: context.library_id,
                upload_id: context.upload_id,
                received: ByteCount::new(received),
                storage_key,
                staging_key,
                job_id: JobId::new(),
                request_id: context.request_id,
                now: self.clock.now(),
            })
            .await
        {
            Ok(true) => Ok(()),
            Ok(false) => Err(AppError::Conflict {
                code: "upload_state_conflict",
            }),
            Err(_) => Err(dependency()),
        }
    }

    async fn promote(
        &self,
        context: ReceiptContext,
        staging: &folioharbor_domain::imports::blob::StorageKey,
        received: u64,
        digest: Sha256Digest,
    ) -> Result<folioharbor_domain::imports::blob::StorageKey, AppError> {
        let identity = BlobIdentity::new(
            StorageNamespace::for_scope(self.dedup_scope, context.library_id, context.upload_id),
            digest,
            ByteCount::new(received),
        );
        let candidate = self.blobs.candidate_key(&identity);
        let prepared = self
            .uploads
            .prepare_promotion(PrepareUploadPromotion {
                actor: context.actor,
                library_id: context.library_id,
                upload_id: context.upload_id,
                staging_key: staging.as_str().to_owned(),
                final_key: candidate.as_str().to_owned(),
                final_owned: self.dedup_scope == DedupScope::Disabled,
                request_id: context.request_id,
                now: self.clock.now(),
            })
            .await
            .map_err(|_| dependency())?;
        if !prepared {
            return Err(AppError::Conflict {
                code: "upload_state_conflict",
            });
        }
        match self.blobs.promote(staging, &identity).await {
            Ok(value) if value == candidate => Ok(value),
            Ok(_) => {
                let _ = self
                    .abort(context, staging, received, "upload_storage_failed")
                    .await;
                Err(dependency())
            }
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
                let alive = self
                    .uploads
                    .heartbeat_receipt(HeartbeatUploadReceipt {
                        actor: context.actor,
                        library_id: context.library_id,
                        upload_id: context.upload_id,
                        staging_key: staging.as_str().to_owned(),
                        request_id: context.request_id,
                        now: self.clock.now(),
                    })
                    .await
                    .map_err(|_| dependency())?;
                if !alive {
                    return Err(AppError::Conflict {
                        code: "upload_receipt_lease_lost",
                    });
                }
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
        if self.blobs.delete(staging).await.is_err() {
            return dependency();
        }
        let transition = self
            .uploads
            .transition_authorized(AuthorizedUploadTransition {
                actor: context.actor,
                library_id: context.library_id,
                upload_id: context.upload_id,
                from: UploadState::Receiving,
                to: UploadState::Failed,
                received: ByteCount::new(received),
                storage_key: Some(staging.as_str().to_owned()),
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
