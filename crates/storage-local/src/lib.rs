#![forbid(unsafe_code)]

mod capacity;
mod paths;

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use async_trait::async_trait;
use folioharbor_application::ports::{BlobStore, BlobStoreError};
use folioharbor_domain::imports::blob::{BlobIdentity, StorageKey};
use sha2::{Digest, Sha256};

pub use capacity::{CapacityProbe, SystemCapacityProbe};

pub const MIN_FREE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_IO_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct LocalBlobStore<P = SystemCapacityProbe> {
    root: PathBuf,
    capacity: P,
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
        }
    }
}

impl<P: CapacityProbe> LocalBlobStore<P> {
    fn prepare_root(&self) -> Result<(), BlobStoreError> {
        fs::create_dir_all(&self.root)?;
        if fs::symlink_metadata(&self.root)?.file_type().is_symlink() {
            return Err(BlobStoreError::InvalidKey);
        }
        Ok(())
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
        self.prepare_root()?;
        self.require_capacity(0)?;
        let directory = self.root.join("staging");
        ensure_directory(&self.root, &directory)?;
        for _ in 0..8 {
            let mut random = [0_u8; 32];
            getrandom::fill(&mut random).map_err(std::io::Error::other)?;
            let token = hex(&random);
            let key = StorageKey::from_opaque(format!("staging:{token}"));
            let path = paths::staging_path(&self.root, &key)?;
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(path) {
                Ok(file) => {
                    file.sync_all()?;
                    sync_directory(&directory)?;
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
        self.prepare_root()?;
        self.require_capacity(u64::try_from(bytes.len()).map_err(std::io::Error::other)?)?;
        let path = paths::staging_path(&self.root, key)?;
        reject_symlink_below(&self.root, &path)?;
        OpenOptions::new()
            .append(true)
            .open(path)?
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
        let path = paths::stored_path(&self.root, key)?;
        reject_symlink_below(&self.root, &path)?;
        let mut file = File::open(path)?;
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
        self.prepare_root()?;
        self.require_capacity(0)?;
        let source = paths::staging_path(&self.root, staging)?;
        let destination = paths::final_path(&self.root, identity);
        let final_key = paths::final_key(identity);
        reject_symlink_below(&self.root, &destination)?;
        if destination.exists() {
            verify_identity(&self.root, &destination, identity)?;
            reject_symlink_below(&self.root, &source)?;
            if source.exists() {
                fs::remove_file(source)?;
            }
            return Ok(final_key);
        }
        reject_symlink_below(&self.root, &source)?;
        verify_identity(&self.root, &source, identity)?;
        let parent = destination.parent().ok_or(BlobStoreError::InvalidKey)?;
        ensure_directory(&self.root, parent)?;
        File::open(&source)?.sync_all()?;
        fs::rename(&source, &destination)?;
        File::open(&destination)?.sync_all()?;
        sync_directory(parent)?;
        Ok(final_key)
    }

    async fn delete(&self, key: &StorageKey) -> Result<(), BlobStoreError> {
        let path = paths::stored_path(&self.root, key)?;
        reject_symlink_below(&self.root, &path)?;
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => Err(BlobStoreError::InvalidKey),
            Ok(_) => {
                fs::remove_file(path)?;
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    async fn free_bytes(&self) -> Result<u64, BlobStoreError> {
        self.prepare_root()?;
        Ok(self.capacity.free_bytes(&self.root)?)
    }
}

fn verify_identity(
    root: &Path,
    path: &Path,
    identity: &BlobIdentity,
) -> Result<(), BlobStoreError> {
    reject_symlink_below(root, path)?;
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let copied = std::io::copy(&mut file, &mut hasher)?;
    let digest: [u8; 32] = hasher.finalize().into();
    if copied != identity.byte_size().get() || digest != identity.sha256().as_bytes() {
        return Err(BlobStoreError::IdentityMismatch);
    }
    Ok(())
}

fn ensure_directory(root: &Path, path: &Path) -> Result<(), BlobStoreError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| BlobStoreError::InvalidKey)?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(BlobStoreError::InvalidKey);
            }
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => return Err(BlobStoreError::InvalidKey),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                create_private_directory(&current)?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn create_private_directory(path: &Path) -> Result<(), BlobStoreError> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    Ok(())
}

fn reject_symlink_below(root: &Path, path: &Path) -> Result<(), BlobStoreError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| BlobStoreError::InvalidKey)?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(BlobStoreError::InvalidKey);
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), BlobStoreError> {
    File::open(path)?.sync_all()?;
    Ok(())
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
