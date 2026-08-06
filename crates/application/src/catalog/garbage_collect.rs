use std::sync::Arc;

use folioharbor_domain::time::OffsetDateTime;
use thiserror::Error;

use crate::ports::{BlobStore, GarbageCollectionRepository};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GarbageCollectionOutcome {
    pub purged_items: u64,
    pub purged_blobs: u64,
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum GarbageCollectionError {
    #[error("garbage collection persistence is unavailable")]
    Repository,
    #[error("garbage collection storage deletion failed and remains retryable")]
    Storage,
}

pub struct CollectGarbage {
    repository: Arc<dyn GarbageCollectionRepository>,
    blobs: Arc<dyn BlobStore>,
    worker: String,
    limit: u32,
}

impl CollectGarbage {
    #[must_use]
    pub fn new<R, B>(repository: Arc<R>, blobs: Arc<B>, worker: String, limit: u32) -> Option<Self>
    where
        R: GarbageCollectionRepository + 'static,
        B: BlobStore + 'static,
    {
        (limit > 0 && limit <= 1_000 && !worker.is_empty() && worker.len() <= 128).then_some(Self {
            repository,
            blobs,
            worker,
            limit,
        })
    }

    /// Advances one bounded two-phase collection batch.
    ///
    /// Storage is touched only after the database has committed a purge claim. A failed delete
    /// releases that durable claim so a later worker pass can retry it.
    ///
    /// # Errors
    /// Returns repository or retryable storage errors.
    pub async fn execute(
        &self,
        now: OffsetDateTime,
    ) -> Result<GarbageCollectionOutcome, GarbageCollectionError> {
        let purged_items = self
            .repository
            .prepare(now, self.limit)
            .await
            .map_err(|_| GarbageCollectionError::Repository)?;
        let claims = self
            .repository
            .claim(&self.worker, now, self.limit)
            .await
            .map_err(|_| GarbageCollectionError::Repository)?;
        let mut purged_blobs = 0_u64;
        for claim in claims {
            if self.blobs.delete(&claim.storage_key).await.is_err() {
                self.repository
                    .release(&claim, &self.worker, now)
                    .await
                    .map_err(|_| GarbageCollectionError::Repository)?;
                return Err(GarbageCollectionError::Storage);
            }
            if !self
                .repository
                .complete(&claim, &self.worker, now)
                .await
                .map_err(|_| GarbageCollectionError::Repository)?
            {
                return Err(GarbageCollectionError::Repository);
            }
            purged_blobs += 1;
        }
        Ok(GarbageCollectionOutcome {
            purged_items,
            purged_blobs,
        })
    }
}
