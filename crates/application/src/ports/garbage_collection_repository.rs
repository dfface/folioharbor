use async_trait::async_trait;
use folioharbor_domain::{id::BlobId, imports::blob::StorageKey, time::OffsetDateTime};
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlobPurgeClaim {
    pub blob_id: BlobId,
    pub storage_key: StorageKey,
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("garbage collection persistence failed")]
pub struct GarbageCollectionRepositoryError;

#[async_trait]
pub trait GarbageCollectionRepository: Send + Sync {
    /// Detaches purge-eligible Item derivatives and schedules newly unreferenced Blobs.
    async fn prepare(
        &self,
        now: OffsetDateTime,
        limit: u32,
    ) -> Result<u64, GarbageCollectionRepositoryError>;

    /// Leases storage deletes whose independent 24-hour safety delay has elapsed.
    async fn claim(
        &self,
        worker: &str,
        now: OffsetDateTime,
        limit: u32,
    ) -> Result<Vec<BlobPurgeClaim>, GarbageCollectionRepositoryError>;

    async fn complete(
        &self,
        claim: &BlobPurgeClaim,
        worker: &str,
        now: OffsetDateTime,
    ) -> Result<bool, GarbageCollectionRepositoryError>;

    async fn release(
        &self,
        claim: &BlobPurgeClaim,
        worker: &str,
        now: OffsetDateTime,
    ) -> Result<bool, GarbageCollectionRepositoryError>;
}
