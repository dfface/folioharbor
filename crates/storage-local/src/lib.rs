#![forbid(unsafe_code)]

mod capacity;
mod file_ops;
mod operations;
mod paths;
mod secure_fs;

use std::{fmt, path::PathBuf, sync::Arc};

use async_trait::async_trait;
use folioharbor_application::ports::{BlobStore, BlobStoreError, PromotedBlob};
use folioharbor_domain::imports::blob::{BlobIdentity, StorageKey};

pub use capacity::{CapacityProbe, SystemCapacityProbe};

pub const MIN_FREE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_IO_BYTES: usize = 8 * 1024 * 1024;

pub struct LocalBlobStore<P = SystemCapacityProbe> {
    root: PathBuf,
    capacity: Arc<P>,
    #[cfg(test)]
    hook: Option<Arc<dyn Fn(HookPoint) + Send + Sync>>,
}

impl<P> Clone for LocalBlobStore<P> {
    fn clone(&self) -> Self {
        Self {
            root: self.root.clone(),
            capacity: Arc::clone(&self.capacity),
            #[cfg(test)]
            hook: self.hook.clone(),
        }
    }
}

impl<P: fmt::Debug> fmt::Debug for LocalBlobStore<P> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalBlobStore")
            .field("root", &self.root)
            .field("capacity", &self.capacity)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HookPoint {
    AppendOpen,
    ReadOpen,
    PromoteSourceOpen,
    PromoteInstall,
    PromoteRecoveryDestinationSynced,
    PromoteRecoveryParentSynced,
    Delete,
}

impl LocalBlobStore<SystemCapacityProbe> {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self::with_capacity(root, SystemCapacityProbe)
    }
}

impl<P> LocalBlobStore<P> {
    #[must_use]
    pub fn with_capacity(root: impl Into<PathBuf>, capacity: P) -> Self {
        Self {
            root: root.into(),
            capacity: Arc::new(capacity),
            #[cfg(test)]
            hook: None,
        }
    }

    #[cfg(test)]
    fn with_test_hook(mut self, hook: Arc<dyn Fn(HookPoint) + Send + Sync>) -> Self {
        self.hook = Some(hook);
        self
    }

    #[cfg(test)]
    fn run_test_hook(&self, point: HookPoint) {
        if let Some(hook) = &self.hook {
            hook(point);
        }
    }
}

#[async_trait]
impl<P: CapacityProbe + 'static> BlobStore for LocalBlobStore<P> {
    fn candidate_key(&self, identity: &BlobIdentity) -> StorageKey {
        paths::final_key(identity)
    }

    async fn create_staging_for(&self, key: &StorageKey) -> Result<(), BlobStoreError> {
        let store = self.clone();
        let key = key.clone();
        run_blocking(move || store.create_staging_sync(&key)).await
    }

    async fn append(&self, key: &StorageKey, bytes: &[u8]) -> Result<(), BlobStoreError> {
        if bytes.len() > MAX_IO_BYTES {
            return Err(BlobStoreError::InvalidRange);
        }
        let store = self.clone();
        let key = key.clone();
        let bytes = bytes.to_vec();
        run_blocking(move || store.append_sync(&key, &bytes)).await
    }

    async fn read_range(
        &self,
        key: &StorageKey,
        offset: u64,
        length: u64,
    ) -> Result<Vec<u8>, BlobStoreError> {
        if length > MAX_IO_BYTES as u64 {
            return Err(BlobStoreError::InvalidRange);
        }
        let store = self.clone();
        let key = key.clone();
        run_blocking(move || store.read_range_sync(&key, offset, length)).await
    }

    async fn promote(
        &self,
        staging: &StorageKey,
        identity: &BlobIdentity,
    ) -> Result<PromotedBlob, BlobStoreError> {
        let store = self.clone();
        let staging = staging.clone();
        let identity = identity.clone();
        run_blocking(move || store.promote_sync(&staging, &identity)).await
    }

    async fn delete(&self, key: &StorageKey) -> Result<(), BlobStoreError> {
        let store = self.clone();
        let key = key.clone();
        run_blocking(move || store.delete_sync(&key)).await
    }

    async fn free_bytes(&self) -> Result<u64, BlobStoreError> {
        let store = self.clone();
        run_blocking(move || store.free_bytes_sync()).await
    }

    async fn open_publication(
        &self,
        key: &StorageKey,
    ) -> Result<Box<dyn folioharbor_application::ports::PublicationSource>, BlobStoreError> {
        let store = self.clone();
        let key = key.clone();
        run_blocking(move || store.open_publication_sync(&key)).await
    }
}

async fn run_blocking<T>(
    operation: impl FnOnce() -> Result<T, BlobStoreError> + Send + 'static,
) -> Result<T, BlobStoreError>
where
    T: Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(std::io::Error::other)?
}

#[cfg(test)]
mod race_tests;

#[cfg(test)]
mod quality_tests;
