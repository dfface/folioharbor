use std::{fmt, str::FromStr};

use crate::id::{LibraryId, UploadId};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct ByteCount(u64);

impl ByteCount {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    #[must_use]
    pub fn checked_add(self, other: Self) -> Option<Self> {
        self.0.checked_add(other.0).map(Self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DedupScope {
    Instance,
    Library,
    Disabled,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct StorageNamespace(String);

impl StorageNamespace {
    #[must_use]
    pub fn for_scope(scope: DedupScope, library_id: LibraryId, upload_id: UploadId) -> Self {
        match scope {
            DedupScope::Instance => Self("instance-v1".to_owned()),
            DedupScope::Library => Self(format!("library-{}", library_id.as_uuid().simple())),
            DedupScope::Disabled => Self(format!("upload-{}", upload_id.as_uuid().simple())),
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Sha256Digest([u8; 32]);

impl Sha256Digest {
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(self) -> [u8; 32] {
        self.0
    }

    #[must_use]
    pub fn to_hex(self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut output = String::with_capacity(64);
        for byte in self.0 {
            output.push(char::from(HEX[usize::from(byte >> 4)]));
            output.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        output
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlobIdentity {
    namespace: StorageNamespace,
    sha256: Sha256Digest,
    byte_size: ByteCount,
}

impl BlobIdentity {
    #[must_use]
    pub const fn new(
        namespace: StorageNamespace,
        sha256: Sha256Digest,
        byte_size: ByteCount,
    ) -> Self {
        Self {
            namespace,
            sha256,
            byte_size,
        }
    }

    #[must_use]
    pub const fn namespace(&self) -> &StorageNamespace {
        &self.namespace
    }

    #[must_use]
    pub const fn sha256(&self) -> Sha256Digest {
        self.sha256
    }

    #[must_use]
    pub const fn byte_size(&self) -> ByteCount {
        self.byte_size
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct StorageKey(String);

impl StorageKey {
    #[must_use]
    pub fn from_opaque(value: String) -> Self {
        Self(value)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for StorageKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Error)]
#[error("storage key is empty")]
pub struct StorageKeyParseError;

impl FromStr for StorageKey {
    type Err = StorageKeyParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.is_empty() {
            Err(StorageKeyParseError)
        } else {
            Ok(Self(value.to_owned()))
        }
    }
}
