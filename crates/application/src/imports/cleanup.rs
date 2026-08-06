use std::sync::Arc;

use folioharbor_domain::{imports::job::JobKind, time::OffsetDateTime};

use crate::ports::{BlobStore, ImportCleanupRepository, ImportCleanupRepositoryError};

pub type CleanupJobKind = JobKind;

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
    pub stable: bool,
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
        Ok(CleanupOutcome {
            expired,
            purged,
            stable: true,
        })
    }

    /// Resumes and drains one durable cleanup kind before advancing its cutoff.
    ///
    /// # Errors
    /// Returns when the persisted pass cannot be resumed, a batch fails, or
    /// physical deletion prevents the pass from reaching a stable boundary.
    pub async fn run_kind(
        &self,
        owner: &str,
        kind: CleanupJobKind,
        now: OffsetDateTime,
        limit: u32,
    ) -> Result<CleanupOutcome, ImportCleanupRepositoryError> {
        let cursor = self.repository.begin_pass(kind, owner, now, limit).await?;
        let mut outcome = CleanupOutcome {
            expired: 0,
            purged: 0,
            stable: false,
        };
        match kind {
            JobKind::ExpireUploadsAndReservations => loop {
                let count = self.repository.expire_abandoned(cursor).await?;
                outcome.expired += count;
                if count < u64::from(cursor.limit()) {
                    break;
                }
            },
            JobKind::PurgeFailedUploads => loop {
                let claims = self.repository.claim_failed_purges(owner, cursor).await?;
                let short_batch = claims.len() < cursor.limit() as usize;
                for claim in claims {
                    if claim.delete_file && self.blobs.delete(&claim.storage_key).await.is_err() {
                        self.repository
                            .release_failed_purge(claim.upload_id, owner, now)
                            .await?;
                        return Err(ImportCleanupRepositoryError);
                    }
                    if self
                        .repository
                        .complete_failed_purge(claim.upload_id, owner, cursor)
                        .await?
                    {
                        outcome.purged += 1;
                    }
                }
                if short_batch {
                    break;
                }
            },
            JobKind::CollectBlobsLater => {}
            JobKind::ImportEpub => return Err(ImportCleanupRepositoryError),
        }
        if self.repository.has_pending(kind, cursor).await? {
            return Err(ImportCleanupRepositoryError);
        }
        self.repository.complete_pass(kind, owner, cursor).await?;
        outcome.stable = true;
        Ok(outcome)
    }
}
