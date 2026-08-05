use std::path::PathBuf;

use folioharbor_domain::imports::blob::{BlobIdentity, StorageKey};

use folioharbor_application::ports::BlobStoreError;

pub(crate) fn staging_relative(key: &StorageKey) -> Result<PathBuf, BlobStoreError> {
    let token = key
        .as_str()
        .strip_prefix("staging:")
        .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or(BlobStoreError::InvalidKey)?;
    Ok(PathBuf::from("staging").join(token))
}

pub(crate) fn final_key(identity: &BlobIdentity) -> StorageKey {
    StorageKey::from_opaque(format!(
        "blob:{}:{}:{}",
        identity.namespace().as_str(),
        identity.sha256().to_hex(),
        identity.byte_size().get()
    ))
}

pub(crate) fn final_relative(identity: &BlobIdentity) -> PathBuf {
    let hash = identity.sha256().to_hex();
    PathBuf::from("objects")
        .join(identity.namespace().as_str())
        .join(&hash[0..2])
        .join(&hash[2..4])
        .join(format!("{hash}-{}", identity.byte_size().get()))
}

pub(crate) fn stored_relative(key: &StorageKey) -> Result<PathBuf, BlobStoreError> {
    if key.as_str().starts_with("staging:") {
        return staging_relative(key);
    }
    let mut parts = key.as_str().split(':');
    if parts.next() != Some("blob") {
        return Err(BlobStoreError::InvalidKey);
    }
    let namespace = parts.next().ok_or(BlobStoreError::InvalidKey)?;
    let hash = parts.next().ok_or(BlobStoreError::InvalidKey)?;
    let size = parts.next().ok_or(BlobStoreError::InvalidKey)?;
    if parts.next().is_some()
        || namespace.is_empty()
        || !namespace
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        || hash.len() != 64
        || !hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        || size.parse::<u64>().is_err()
    {
        return Err(BlobStoreError::InvalidKey);
    }
    Ok(PathBuf::from("objects")
        .join(namespace)
        .join(&hash[0..2])
        .join(&hash[2..4])
        .join(format!("{hash}-{size}")))
}
