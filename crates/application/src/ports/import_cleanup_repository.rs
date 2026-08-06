use async_trait::async_trait;
use folioharbor_domain::{id::UploadId, imports::blob::StorageKey};
use thiserror::Error;

use crate::imports::CleanupCursor;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FailedUploadPurge {
    pub upload_id: UploadId,
    pub storage_key: StorageKey,
    pub delete_file: bool,
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("import cleanup persistence failed")]
pub struct ImportCleanupRepositoryError;

#[async_trait]
pub trait ImportCleanupRepository: Send + Sync {
    async fn expire_abandoned(
        &self,
        cursor: CleanupCursor,
    ) -> Result<u64, ImportCleanupRepositoryError>;
    async fn claim_failed_purges(
        &self,
        owner: &str,
        cursor: CleanupCursor,
    ) -> Result<Vec<FailedUploadPurge>, ImportCleanupRepositoryError>;
    async fn complete_failed_purge(
        &self,
        upload_id: UploadId,
        owner: &str,
        cursor: CleanupCursor,
    ) -> Result<bool, ImportCleanupRepositoryError>;
}
