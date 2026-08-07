use async_trait::async_trait;
use folioharbor_domain::imports::blob::StorageKey;
use sha2::{Digest as _, Sha256};

use crate::ports::{BlobStore, BlobStoreError};

#[derive(Clone, Debug)]
pub struct BlobInventoryEntry {
    pub storage_key: StorageKey,
    pub location_is_canonical: bool,
    pub expected_sha256: [u8; 32],
    pub expected_byte_size: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("blob inventory could not be read")]
pub struct ConsistencyRepositoryError;

#[async_trait]
pub trait ConsistencyRepository: Send + Sync {
    async fn ready_blob_inventory(
        &self,
    ) -> Result<Vec<BlobInventoryEntry>, ConsistencyRepositoryError>;
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ConsistencyReport {
    pub checked: u64,
    pub missing_blobs: u64,
    pub orphan_locations: u64,
    pub hash_mismatches: u64,
}

impl ConsistencyReport {
    #[must_use]
    pub const fn is_clean(self) -> bool {
        self.missing_blobs == 0 && self.orphan_locations == 0 && self.hash_mismatches == 0
    }
}

#[derive(Debug, thiserror::Error)]
#[error("storage consistency check could not complete")]
pub struct ConsistencyCheckError;

pub struct ConsistencyCheck<'a, R, B> {
    repository: &'a R,
    blobs: &'a B,
}

impl<'a, R, B> ConsistencyCheck<'a, R, B> {
    #[must_use]
    pub const fn new(repository: &'a R, blobs: &'a B) -> Self {
        Self { repository, blobs }
    }
}

impl<R: ConsistencyRepository, B: BlobStore> ConsistencyCheck<'_, R, B> {
    /// Compares every ready database location with the bytes in Blob storage.
    ///
    /// # Errors
    ///
    /// Returns an error for repository or storage failures that cannot be classified safely.
    pub async fn execute(&self) -> Result<ConsistencyReport, ConsistencyCheckError> {
        let inventory = self
            .repository
            .ready_blob_inventory()
            .await
            .map_err(|_| ConsistencyCheckError)?;
        let mut report = ConsistencyReport::default();
        for entry in inventory {
            report.checked = report.checked.saturating_add(1);
            if !entry.location_is_canonical {
                report.orphan_locations = report.orphan_locations.saturating_add(1);
                continue;
            }
            let mut source = match self.blobs.open_publication(&entry.storage_key).await {
                Ok(source) => source,
                Err(BlobStoreError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                    report.missing_blobs = report.missing_blobs.saturating_add(1);
                    continue;
                }
                Err(BlobStoreError::InvalidKey) => {
                    report.orphan_locations = report.orphan_locations.saturating_add(1);
                    continue;
                }
                Err(_) => return Err(ConsistencyCheckError),
            };
            let mut hasher = Sha256::new();
            let copied =
                std::io::copy(&mut source, &mut hasher).map_err(|_| ConsistencyCheckError)?;
            let digest: [u8; 32] = hasher.finalize().into();
            if copied != entry.expected_byte_size || digest != entry.expected_sha256 {
                report.hash_mismatches = report.hash_mismatches.saturating_add(1);
            }
        }
        Ok(report)
    }
}
