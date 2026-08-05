use std::sync::Arc;

use crate::ports::{
    BlobStore, ClaimUploadCleanup, ExpireUploads, UploadCleanupGuard, UploadRepository,
    UploadRepositoryError,
};
use folioharbor_domain::{imports::blob::StorageKey, time::OffsetDateTime};

pub struct UploadRecoveryService {
    uploads: Arc<dyn UploadRepository>,
    blobs: Arc<dyn BlobStore>,
}

impl UploadRecoveryService {
    #[must_use]
    pub fn new(uploads: Arc<dyn UploadRepository>, blobs: Arc<dyn BlobStore>) -> Self {
        Self { uploads, blobs }
    }

    /// Expires stale receipts and removes the blobs owned by leased cleanup records.
    ///
    /// # Errors
    ///
    /// Returns an error when the durable recovery repository is unavailable. Blob deletion
    /// failures leave the cleanup leased so another worker can retry it after lease expiry.
    pub async fn reconcile(
        &self,
        owner: &str,
        now: OffsetDateTime,
        limit: u32,
    ) -> Result<u64, UploadRepositoryError> {
        let request_id = folioharbor_domain::id::RequestId::new();
        self.uploads
            .expire_worker(ExpireUploads {
                now,
                limit,
                request_id,
            })
            .await?;
        let mut completed = 0_u64;
        for _ in 0..limit {
            let Some(guard) = self
                .uploads
                .claim_cleanup(ClaimUploadCleanup {
                    owner: owner.to_owned(),
                    now,
                    request_id,
                })
                .await?
            else {
                break;
            };
            let blobs = Arc::clone(&self.blobs);
            let changed = tokio::spawn(async move { clean_claim(blobs, guard, now).await })
                .await
                .map_err(|_| UploadRepositoryError::Persistence)??;
            if changed {
                completed += 1;
            }
        }
        Ok(completed)
    }
}

async fn clean_claim(
    blobs: Arc<dyn BlobStore>,
    guard: Box<dyn UploadCleanupGuard>,
    now: OffsetDateTime,
) -> Result<bool, UploadRepositoryError> {
    let cleanup = guard.cleanup().clone();
    if blobs
        .delete(&StorageKey::from_opaque(cleanup.staging_key))
        .await
        .is_err()
    {
        guard.abandon().await?;
        return Ok(false);
    }
    if cleanup.final_owned
        && let Some(final_key) = cleanup.final_key
        && blobs
            .delete(&StorageKey::from_opaque(final_key))
            .await
            .is_err()
    {
        guard.abandon().await?;
        return Ok(false);
    }
    guard.complete(now).await
}
