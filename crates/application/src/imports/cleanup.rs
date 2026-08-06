use std::sync::Arc;

use folioharbor_domain::time::OffsetDateTime;

use crate::ports::{BlobStore, ImportCleanupRepository, ImportCleanupRepositoryError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CleanupJobKind {
    ExpireUploadsAndReservations,
    PurgeFailedUploads,
    CollectBlobsLater,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CleanupCursor {
    not_after: OffsetDateTime,
    limit: u32,
}

impl CleanupCursor {
    #[must_use]
    pub const fn new(not_after: OffsetDateTime, limit: u32) -> Option<Self> {
        if limit == 0 || limit > 1_000 {
            None
        } else {
            Some(Self { not_after, limit })
        }
    }

    #[must_use]
    pub const fn not_after(self) -> OffsetDateTime {
        self.not_after
    }

    #[must_use]
    pub const fn limit(self) -> u32 {
        self.limit
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CleanupOutcome {
    pub expired: u64,
    pub purged: u64,
}

pub struct CleanupImports {
    repository: Arc<dyn ImportCleanupRepository>,
    blobs: Arc<dyn BlobStore>,
}

impl CleanupImports {
    #[must_use]
    pub fn new(repository: Arc<dyn ImportCleanupRepository>, blobs: Arc<dyn BlobStore>) -> Self {
        Self { repository, blobs }
    }

    /// Runs one bounded cleanup pass at an immutable persisted time boundary.
    ///
    /// # Errors
    /// Returns when durable claims cannot be acquired or completed. Failed storage
    /// deletion leaves the claim leased for recovery by a later pass.
    pub async fn run(
        &self,
        owner: &str,
        cursor: CleanupCursor,
    ) -> Result<CleanupOutcome, ImportCleanupRepositoryError> {
        let expired = self.repository.expire_abandoned(cursor).await?;
        let claims = self.repository.claim_failed_purges(owner, cursor).await?;
        let mut purged = 0_u64;
        for claim in claims {
            if claim.delete_file && self.blobs.delete(&claim.storage_key).await.is_err() {
                continue;
            }
            if self
                .repository
                .complete_failed_purge(claim.upload_id, owner, cursor)
                .await?
            {
                purged += 1;
            }
        }
        Ok(CleanupOutcome { expired, purged })
    }
}
