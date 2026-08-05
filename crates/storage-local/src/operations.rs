use std::{
    io::{Seek, SeekFrom, Write},
    path::Path,
};

use folioharbor_application::ports::{BlobDisposition, BlobStoreError, PromotedBlob};
use folioharbor_domain::imports::blob::{BlobIdentity, StorageKey};

use super::{
    CapacityProbe, LocalBlobStore, MAX_IO_BYTES, MIN_FREE_BYTES,
    file_ops::{
        append_options, open_optional, private_create_options, read_options, read_up_to,
        remove_named_file_if_present, verify_file,
    },
    paths,
    secure_fs::{SecureRoot, sync_dir},
};

#[cfg(test)]
use super::HookPoint;

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

    pub(super) fn create_staging_sync(&self, key: &StorageKey) -> Result<(), BlobStoreError> {
        let root = self.secure_root()?;
        self.require_capacity(0)?;
        let relative = paths::staging_relative(key)?;
        let (directory, name) = root.open_parent(&relative, true)?;
        let file = directory.open_with(&name, &private_create_options())?;
        file.sync_all()?;
        sync_dir(&directory)?;
        Ok(())
    }

    pub(super) fn append_sync(&self, key: &StorageKey, bytes: &[u8]) -> Result<(), BlobStoreError> {
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

    pub(super) fn read_range_sync(
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
        Ok(read_up_to(&mut file, length)?)
    }

    pub(super) fn promote_sync(
        &self,
        staging: &StorageKey,
        identity: &BlobIdentity,
    ) -> Result<PromotedBlob, BlobStoreError> {
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
            return Ok(PromotedBlob {
                key: final_key,
                disposition: BlobDisposition::Reused,
            });
        }
        #[cfg(test)]
        self.run_test_hook(HookPoint::PromoteSourceOpen);
        let (source_dir, source_name) = match root.open_parent(&source_relative, false) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut existing = destination_dir.open_with(&destination_name, &read_options())?;
                verify_file(&mut existing, identity)?;
                existing.sync_all()?;
                #[cfg(test)]
                self.run_test_hook(HookPoint::PromoteRecoveryDestinationSynced);
                sync_dir(&destination_dir)?;
                #[cfg(test)]
                self.run_test_hook(HookPoint::PromoteRecoveryParentSynced);
                return Ok(PromotedBlob {
                    key: final_key,
                    disposition: BlobDisposition::Reused,
                });
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
        Ok(PromotedBlob {
            key: final_key,
            disposition: if installed {
                BlobDisposition::Installed
            } else {
                BlobDisposition::Reused
            },
        })
    }

    pub(super) fn delete_sync(&self, key: &StorageKey) -> Result<(), BlobStoreError> {
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

    pub(super) fn free_bytes_sync(&self) -> Result<u64, BlobStoreError> {
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
