use async_trait::async_trait;
use folioharbor_domain::imports::blob::{BlobIdentity, StorageKey};
use std::io::{Read, Seek};
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

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BlobStoreInventory {
    pub keys: Vec<StorageKey>,
    pub invalid_locations: u64,
}

pub trait PublicationSource: Read + Seek + Send {}
impl<T: Read + Seek + Send> PublicationSource for T {}

#[async_trait]
pub trait BlobStore: Send + Sync {
    fn candidate_key(&self, identity: &BlobIdentity) -> StorageKey;
    async fn create_staging_for(&self, key: &StorageKey) -> Result<(), BlobStoreError>;
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
    async fn probe_write(&self) -> Result<(), BlobStoreError> {
        Err(BlobStoreError::InvalidKey)
    }
    async fn inventory(&self) -> Result<BlobStoreInventory, BlobStoreError> {
        Err(BlobStoreError::InvalidKey)
    }
    async fn open_publication(
        &self,
        _key: &StorageKey,
    ) -> Result<Box<dyn PublicationSource>, BlobStoreError> {
        Err(BlobStoreError::InvalidKey)
    }
}
