use std::{
    io::{Seek, SeekFrom, Write},
    path::Path,
};

use cap_fs_ext::DirExt as _;
use cap_std::fs::Dir;
use folioharbor_application::ports::{
    BlobDisposition, BlobStoreError, BlobStoreInventory, PromotedBlob,
};
use folioharbor_domain::{
    id::RequestId,
    imports::blob::{BlobIdentity, StorageKey},
};

use super::{
    CapacityProbe, LocalBlobStore, MAX_RANGE_READ_BYTES, MIN_FREE_BYTES,
    file_ops::{
        append_options, open_optional, private_create_options, read_options, read_up_to,
        remove_named_file_if_present, verify_file,
    },
    paths,
    secure_fs::{SecureRoot, sync_dir, verify_private_dir},
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
        if bytes.len() > MAX_RANGE_READ_BYTES {
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
        if length > MAX_RANGE_READ_BYTES {
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

    pub(super) fn probe_write_sync(&self) -> Result<(), BlobStoreError> {
        let root = self.secure_root()?;
        root.verify_private()?;
        let staging = root.open_dir(Path::new("staging"), true)?;
        verify_private_dir(&staging)?;
        let health = root.open_dir(Path::new("staging/.health"), true)?;
        verify_private_dir(&health)?;
        let name = format!("{}.probe", RequestId::new().as_ulid());
        let mut created = false;
        let result = (|| {
            let mut file = health.open_with(&name, &private_create_options())?;
            created = true;
            file.write_all(b"ready")?;
            file.sync_all()?;
            drop(file);
            health.remove_file(&name)?;
            created = false;
            sync_dir(&health)?;
            Ok::<(), std::io::Error>(())
        })();
        if created {
            let _ = health.remove_file(&name);
            let _ = sync_dir(&health);
        }
        result.map_err(Into::into)
    }

    pub(super) fn inventory_sync(&self) -> Result<BlobStoreInventory, BlobStoreError> {
        let root = self.secure_root()?;
        let objects = match root.open_dir(Path::new("objects"), false) {
            Ok(directory) => directory,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(BlobStoreInventory::default());
            }
            Err(error) => return Err(error.into()),
        };
        let mut inventory = BlobStoreInventory::default();
        inventory_directory(&objects, 0, &mut Vec::new(), &mut inventory)?;
        inventory
            .keys
            .sort_by(|left, right| left.as_str().cmp(right.as_str()));
        Ok(inventory)
    }

    pub(super) fn open_publication_sync(
        &self,
        key: &StorageKey,
    ) -> Result<Box<dyn folioharbor_application::ports::PublicationSource>, BlobStoreError> {
        let root = self.secure_root()?;
        let relative = paths::stored_relative(key)?;
        let (directory, name) = root.open_parent(&relative, false)?;
        Ok(Box::new(directory.open_with(name, &read_options())?))
    }
}

fn inventory_directory(
    directory: &Dir,
    depth: usize,
    components: &mut Vec<String>,
    inventory: &mut BlobStoreInventory,
) -> Result<(), BlobStoreError> {
    for entry in directory.entries()? {
        let entry = entry?;
        let Ok(name) = entry.file_name().into_string() else {
            inventory.invalid_locations = inventory.invalid_locations.saturating_add(1);
            continue;
        };
        let file_type = entry.file_type()?;
        if depth < 3 {
            if !file_type.is_dir() || !valid_inventory_directory(depth, &name) {
                inventory.invalid_locations = inventory.invalid_locations.saturating_add(1);
                continue;
            }
            let Ok(child) = directory.open_dir_nofollow(Path::new(&name)) else {
                inventory.invalid_locations = inventory.invalid_locations.saturating_add(1);
                continue;
            };
            components.push(name);
            inventory_directory(&child, depth + 1, components, inventory)?;
            components.pop();
            continue;
        }
        if !file_type.is_file() {
            inventory.invalid_locations = inventory.invalid_locations.saturating_add(1);
            continue;
        }
        match physical_key(components, &name) {
            Some(key) => inventory.keys.push(key),
            None => {
                inventory.invalid_locations = inventory.invalid_locations.saturating_add(1);
            }
        }
    }
    Ok(())
}

fn valid_inventory_directory(depth: usize, name: &str) -> bool {
    match depth {
        0 => {
            !name.is_empty()
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        }
        1 | 2 => name.len() == 2 && name.bytes().all(is_lower_hex),
        _ => false,
    }
}

fn physical_key(components: &[String], file_name: &str) -> Option<StorageKey> {
    let [namespace, first, second] = components else {
        return None;
    };
    let (hash, size) = file_name.rsplit_once('-')?;
    if hash.len() != 64
        || !hash.bytes().all(is_lower_hex)
        || first != &hash[0..2]
        || second != &hash[2..4]
        || size.parse::<u64>().is_err()
    {
        return None;
    }
    Some(StorageKey::from_opaque(format!(
        "blob:{namespace}:{hash}:{size}"
    )))
}

fn is_lower_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
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
