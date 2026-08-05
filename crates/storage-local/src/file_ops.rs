use std::ffi::OsStr;

use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, File, OpenOptions};
use folioharbor_application::ports::BlobStoreError;
use folioharbor_domain::imports::blob::BlobIdentity;
use sha2::{Digest, Sha256};

pub(crate) fn verify_file(file: &mut File, identity: &BlobIdentity) -> Result<(), BlobStoreError> {
    let mut hasher = Sha256::new();
    let copied = std::io::copy(file, &mut hasher)?;
    let digest: [u8; 32] = hasher.finalize().into();
    if copied != identity.byte_size().get() || digest != identity.sha256().as_bytes() {
        return Err(BlobStoreError::IdentityMismatch);
    }
    Ok(())
}

pub(crate) fn open_optional(directory: &Dir, name: &OsStr) -> Result<Option<File>, BlobStoreError> {
    match directory.open_with(name, &read_options()) {
        Ok(file) => Ok(Some(file)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn remove_named_file_if_present(
    directory: &Dir,
    name: &OsStr,
) -> Result<(), BlobStoreError> {
    match directory.remove_file(name) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn read_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    options
}

pub(crate) fn append_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.append(true).follow(FollowSymlinks::No);
    options
}

pub(crate) fn private_create_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}
