use std::sync::Arc;

use folioharbor_domain::{imports::blob::StorageKey, time::OffsetDateTime};
use time::Duration;

use crate::ports::{
    BlobStore, ExpireUploads, LeaseUploadCleanups, UploadRepository, UploadRepositoryError,
};

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
        let cleanups = self
            .uploads
            .lease_cleanups(LeaseUploadCleanups {
                owner: owner.to_owned(),
                now,
                lease_for: Duration::minutes(5),
                limit,
                request_id,
            })
            .await?;
        let mut completed = 0_u64;
        for cleanup in cleanups {
            if self
                .blobs
                .delete(&StorageKey::from_opaque(cleanup.staging_key))
                .await
                .is_err()
            {
                continue;
            }
            if cleanup.final_owned
                && let Some(final_key) = cleanup.final_key
                && self
                    .blobs
                    .delete(&StorageKey::from_opaque(final_key))
                    .await
                    .is_err()
            {
                continue;
            }
            if self
                .uploads
                .complete_cleanup(
                    cleanup.upload_id,
                    &cleanup.attempt_token,
                    owner,
                    now,
                    request_id,
                )
                .await?
            {
                completed += 1;
            }
        }
        Ok(completed)
    }
}
