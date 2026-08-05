use async_trait::async_trait;
use folioharbor_domain::imports::blob::{BlobIdentity, StorageKey};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BlobStoreError {
    #[error("invalid storage key")]
    InvalidKey,
    #[error("storage capacity floor would be violated")]
    InsufficientCapacity,
    #[error("blob content does not match its identity")]
    IdentityMismatch,
    #[error("blob range is invalid")]
    InvalidRange,
    #[error("blob storage operation failed")]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlobDisposition {
    Installed,
    Reused,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PromotedBlob {
    pub key: StorageKey,
    pub disposition: BlobDisposition,
}

#[async_trait]
pub trait BlobStore: Send + Sync {
    fn candidate_key(&self, identity: &BlobIdentity) -> StorageKey;
    async fn create_staging(&self) -> Result<StorageKey, BlobStoreError>;
    async fn append(&self, key: &StorageKey, bytes: &[u8]) -> Result<(), BlobStoreError>;
    async fn read_range(
        &self,
        key: &StorageKey,
        offset: u64,
        length: u64,
    ) -> Result<Vec<u8>, BlobStoreError>;
    async fn promote(
        &self,
        staging: &StorageKey,
        identity: &BlobIdentity,
    ) -> Result<PromotedBlob, BlobStoreError>;
    async fn delete(&self, key: &StorageKey) -> Result<(), BlobStoreError>;
    async fn free_bytes(&self) -> Result<u64, BlobStoreError>;
}
