use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use folioharbor_domain::{
    id::{BlobId, ItemId, RequestId},
    imports::blob::StorageKey,
};
use sha2::{Digest as _, Sha256};
use std::fmt;
use unicode_properties::{GeneralCategory, UnicodeGeneralCategory as _};

use crate::{
    actor::Actor,
    error::AppError,
    ports::{DownloadAuthorization, DownloadRange, DownloadRepository, DownloadSourceReceiver},
};

struct DownloadSource {
    blob_id: BlobId,
    storage_identity: StorageKey,
    byte_size: u64,
    file_name: String,
}

#[derive(Default)]
struct DownloadSourceCollector(DownloadSourceCollection);

#[derive(Default)]
enum DownloadSourceCollection {
    #[default]
    Missing,
    One(DownloadSource),
    Invalid,
}

impl DownloadSourceReceiver for DownloadSourceCollector {
    fn receive(
        &mut self,
        blob_id: BlobId,
        storage_identity: StorageKey,
        byte_size: u64,
        file_name: String,
    ) {
        let source = DownloadSource {
            blob_id,
            storage_identity,
            byte_size,
            file_name,
        };
        self.0 = match std::mem::replace(&mut self.0, DownloadSourceCollection::Invalid) {
            DownloadSourceCollection::Missing => DownloadSourceCollection::One(source),
            DownloadSourceCollection::One(_) | DownloadSourceCollection::Invalid => {
                DownloadSourceCollection::Invalid
            }
        };
    }
}

impl fmt::Debug for DownloadSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DownloadSource")
            .field("byte_size", &self.byte_size)
            .finish_non_exhaustive()
    }
}

#[async_trait]
pub trait DownloadApi: Send + Sync {
    async fn authorize(
        &self,
        actor: Actor,
        item_id: ItemId,
        request_id: RequestId,
    ) -> Result<DownloadGrant, AppError>;
    async fn record_start(
        &self,
        actor: Actor,
        item_id: ItemId,
        request_id: RequestId,
        range: DownloadRange,
    ) -> Result<(), AppError>;
}

pub struct DownloadService<R> {
    repository: R,
}

impl<R> DownloadService<R> {
    #[must_use]
    pub const fn new(repository: R) -> Self {
        Self { repository }
    }
}

#[async_trait]
impl<R: DownloadRepository> DownloadApi for DownloadService<R> {
    async fn authorize(
        &self,
        actor: Actor,
        item_id: ItemId,
        request_id: RequestId,
    ) -> Result<DownloadGrant, AppError> {
        DownloadItem::new(&self.repository)
            .authorize(actor, item_id, request_id)
            .await
    }

    async fn record_start(
        &self,
        actor: Actor,
        item_id: ItemId,
        request_id: RequestId,
        range: DownloadRange,
    ) -> Result<(), AppError> {
        let recorded = self
            .repository
            .record_download_start(actor, item_id, request_id, range)
            .await
            .map_err(|_| AppError::DependencyUnavailable {
                code: "download_repository_unavailable",
            })?;
        if recorded {
            Ok(())
        } else {
            Err(AppError::NotFound {
                code: "item_not_found",
            })
        }
    }
}

pub struct UnavailableDownloadApi;

#[async_trait]
impl DownloadApi for UnavailableDownloadApi {
    async fn authorize(
        &self,
        _: Actor,
        _: ItemId,
        _: RequestId,
    ) -> Result<DownloadGrant, AppError> {
        Err(AppError::DependencyUnavailable {
            code: "download_repository_unavailable",
        })
    }
    async fn record_start(
        &self,
        _: Actor,
        _: ItemId,
        _: RequestId,
        _: DownloadRange,
    ) -> Result<(), AppError> {
        Err(AppError::DependencyUnavailable {
            code: "download_repository_unavailable",
        })
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct DownloadGrant {
    storage_identity: StorageKey,
    byte_size: u64,
    safe_file_name: String,
    etag: String,
}

impl fmt::Debug for DownloadGrant {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DownloadGrant")
            .field("byte_size", &self.byte_size)
            .field("media_type", &self.media_type())
            .field("etag", &self.etag)
            .finish_non_exhaustive()
    }
}

impl DownloadGrant {
    #[must_use]
    pub const fn storage_identity(&self) -> &StorageKey {
        &self.storage_identity
    }
    #[must_use]
    pub const fn byte_size(&self) -> u64 {
        self.byte_size
    }
    #[must_use]
    pub const fn media_type(&self) -> &'static str {
        "application/epub+zip"
    }
    #[must_use]
    pub fn safe_file_name(&self) -> &str {
        &self.safe_file_name
    }
    #[must_use]
    pub fn etag(&self) -> &str {
        &self.etag
    }
}

pub struct DownloadItem<'a, R: ?Sized> {
    repository: &'a R,
}

impl<'a, R: ?Sized> DownloadItem<'a, R> {
    #[must_use]
    pub const fn new(repository: &'a R) -> Self {
        Self { repository }
    }
}

impl<R: DownloadRepository + ?Sized> DownloadItem<'_, R> {
    /// Authorizes an original-file download and returns only streaming metadata.
    ///
    /// # Errors
    /// Returns anti-enumerating not-found or a stable dependency error.
    pub async fn authorize(
        &self,
        actor: Actor,
        item_id: ItemId,
        request_id: RequestId,
    ) -> Result<DownloadGrant, AppError> {
        let mut source = DownloadSourceCollector::default();
        let authorization = self
            .repository
            .authorize_download(actor, item_id, request_id, &mut source)
            .await
            .map_err(|_| AppError::DependencyUnavailable {
                code: "download_repository_unavailable",
            })?;
        match authorization {
            DownloadAuthorization::Granted => {}
            DownloadAuthorization::Forbidden => {
                return Err(AppError::Forbidden {
                    code: "item_download_forbidden",
                });
            }
            DownloadAuthorization::NotFound => {
                return Err(AppError::NotFound {
                    code: "item_not_found",
                });
            }
        }
        let DownloadSourceCollection::One(source) = source.0 else {
            return Err(AppError::DependencyUnavailable {
                code: "download_repository_unavailable",
            });
        };
        Ok(DownloadGrant {
            storage_identity: source.storage_identity,
            byte_size: source.byte_size,
            safe_file_name: sanitize_download_file_name(&source.file_name),
            etag: opaque_etag(source.blob_id),
        })
    }
}

fn opaque_etag(blob: BlobId) -> String {
    let mut digest = Sha256::new();
    digest.update(b"folioharbor-download-etag-v1\0");
    digest.update(blob.as_uuid().as_bytes());
    format!("\"{}\"", URL_SAFE_NO_PAD.encode(digest.finalize()))
}

#[must_use]
pub fn sanitize_download_file_name(value: &str) -> String {
    let leaf = value.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned = leaf
        .chars()
        .filter(|character| !is_unsafe_filename_character(*character))
        .collect::<String>();
    if cleaned.trim().is_empty() {
        "publication.epub".to_owned()
    } else {
        cleaned
    }
}

fn is_unsafe_filename_character(character: char) -> bool {
    character.is_control() || character.general_category() == GeneralCategory::Format
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_download_source_debug_is_redacted() {
        let blob_id = BlobId::new();
        let source = DownloadSource {
            blob_id,
            storage_identity: StorageKey::from_opaque("blob:secret-debug-location".to_owned()),
            byte_size: 16,
            file_name: "secret-debug-name.epub".to_owned(),
        };

        let debug = format!("{source:?}");
        assert!(!debug.contains(&blob_id.as_uuid().to_string()));
        assert!(!debug.contains("secret-debug-location"));
        assert!(!debug.contains("secret-debug-name"));
        assert!(debug.contains("byte_size: 16"));
    }
}
