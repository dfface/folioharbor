#![forbid(unsafe_code)]

mod capacity;
mod file_ops;
mod paths;
mod secure_fs;

#[cfg(test)]
use std::sync::Arc;
use std::{
    fmt,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use async_trait::async_trait;
use file_ops::{
    append_options, open_optional, private_create_options, read_options,
    remove_named_file_if_present, verify_file,
};
use folioharbor_application::ports::{BlobStore, BlobStoreError};
use folioharbor_domain::imports::blob::{BlobIdentity, StorageKey};
use secure_fs::{SecureRoot, sync_dir};

pub use capacity::{CapacityProbe, SystemCapacityProbe};

pub const MIN_FREE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_IO_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone)]
pub struct LocalBlobStore<P = SystemCapacityProbe> {
    root: PathBuf,
    capacity: P,
    #[cfg(test)]
    hook: Option<Arc<dyn Fn(HookPoint) + Send + Sync>>,
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
    PromoteInstall,
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
            capacity,
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

impl<P: CapacityProbe> LocalBlobStore<P> {
    fn secure_root(&self) -> Result<SecureRoot, BlobStoreError> {
        SecureRoot::open(&self.root).map_err(Into::into)
    }

    fn require_capacity(&self, additional: u64) -> Result<(), BlobStoreError> {
        let free = self.capacity.free_bytes(&self.root)?;
        if free < MIN_FREE_BYTES.saturating_add(additional) {
            Err(BlobStoreError::InsufficientCapacity)
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl<P: CapacityProbe> BlobStore for LocalBlobStore<P> {
    async fn create_staging(&self) -> Result<StorageKey, BlobStoreError> {
        let root = self.secure_root()?;
        self.require_capacity(0)?;
        for _ in 0..8 {
            let mut random = [0_u8; 32];
            getrandom::fill(&mut random).map_err(std::io::Error::other)?;
            let key = StorageKey::from_opaque(format!("staging:{}", hex(&random)));
            let relative = paths::staging_relative(&key)?;
            let (directory, name) = root.open_parent(&relative, true)?;
            match directory.open_with(&name, &private_create_options()) {
                Ok(file) => {
                    file.sync_all()?;
                    sync_dir(&directory)?;
                    return Ok(key);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not allocate a unique staging key",
        )
        .into())
    }

    async fn append(&self, key: &StorageKey, bytes: &[u8]) -> Result<(), BlobStoreError> {
        if bytes.len() > MAX_IO_BYTES {
            return Err(BlobStoreError::InvalidRange);
        }
        let root = self.secure_root()?;
        self.require_capacity(u64::try_from(bytes.len()).map_err(std::io::Error::other)?)?;
        let relative = paths::staging_relative(key)?;
        let (directory, name) = root.open_parent(&relative, false)?;
        #[cfg(test)]
        self.run_test_hook(HookPoint::AppendOpen);
        directory
            .open_with(name, &append_options())?
            .write_all(bytes)?;
        Ok(())
    }

    async fn read_range(
        &self,
        key: &StorageKey,
        offset: u64,
        length: u64,
    ) -> Result<Vec<u8>, BlobStoreError> {
        let length = usize::try_from(length).map_err(|_| BlobStoreError::InvalidRange)?;
        if length > MAX_IO_BYTES {
            return Err(BlobStoreError::InvalidRange);
        }
        let root = self.secure_root()?;
        let relative = paths::stored_relative(key)?;
        let (directory, name) = root.open_parent(&relative, false)?;
        #[cfg(test)]
        self.run_test_hook(HookPoint::ReadOpen);
        let mut file = directory.open_with(name, &read_options())?;
        file.seek(SeekFrom::Start(offset))?;
        let mut output = vec![0; length];
        let count = file.read(&mut output)?;
        output.truncate(count);
        Ok(output)
    }

    async fn promote(
        &self,
        staging: &StorageKey,
        identity: &BlobIdentity,
    ) -> Result<StorageKey, BlobStoreError> {
        let root = self.secure_root()?;
        self.require_capacity(0)?;
        let source_relative = paths::staging_relative(staging)?;
        let destination_relative = paths::final_relative(identity);
        let final_key = paths::final_key(identity);
        let (destination_dir, destination_name) = root.open_parent(&destination_relative, true)?;
        if let Some(mut existing) = open_optional(&destination_dir, &destination_name)? {
            verify_file(&mut existing, identity)?;
            existing.sync_all()?;
            sync_dir(&destination_dir)?;
            remove_source_if_present(&root, &source_relative)?;
            return Ok(final_key);
        }
        let (source_dir, source_name) = match root.open_parent(&source_relative, false) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut existing = destination_dir.open_with(&destination_name, &read_options())?;
                verify_file(&mut existing, identity)?;
                return Ok(final_key);
            }
            Err(error) => return Err(error.into()),
        };
        let mut source = source_dir.open_with(&source_name, &read_options())?;
        verify_file(&mut source, identity)?;
        source.sync_all()?;
        #[cfg(test)]
        self.run_test_hook(HookPoint::PromoteInstall);
        let installed =
            match source_dir.hard_link(&source_name, &destination_dir, &destination_name) {
                Ok(()) => true,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => false,
                Err(error) => return Err(error.into()),
            };
        let mut destination = destination_dir.open_with(&destination_name, &read_options())?;
        if let Err(error) = verify_file(&mut destination, identity) {
            drop(destination);
            if installed {
                destination_dir.remove_file(&destination_name)?;
                sync_dir(&destination_dir)?;
            }
            return Err(error);
        }
        destination.sync_all()?;
        drop(destination);
        sync_dir(&destination_dir)?;
        remove_named_file_if_present(&source_dir, &source_name)?;
        sync_dir(&source_dir)?;
        Ok(final_key)
    }

    async fn delete(&self, key: &StorageKey) -> Result<(), BlobStoreError> {
        let root = self.secure_root()?;
        let relative = paths::stored_relative(key)?;
        let (directory, name) = match root.open_parent(&relative, false) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        match directory.symlink_metadata(&name) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(BlobStoreError::InvalidKey);
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        }
        #[cfg(test)]
        self.run_test_hook(HookPoint::Delete);
        remove_named_file_if_present(&directory, &name)?;
        sync_dir(&directory)?;
        Ok(())
    }

    async fn free_bytes(&self) -> Result<u64, BlobStoreError> {
        let root = self.secure_root()?;
        root.sync()?;
        Ok(self.capacity.free_bytes(&self.root)?)
    }
}

fn remove_source_if_present(root: &SecureRoot, relative: &Path) -> Result<(), BlobStoreError> {
    match root.open_parent(relative, false) {
        Ok((directory, name)) => {
            remove_named_file_if_present(&directory, &name)?;
            sync_dir(&directory)?;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

#[cfg(test)]
mod race_tests;
