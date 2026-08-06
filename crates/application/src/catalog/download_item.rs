use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use folioharbor_domain::{
    id::{BlobId, ItemId, LibraryId, RequestId},
    imports::blob::StorageKey,
};
use sha2::{Digest as _, Sha256};
use std::fmt;
use thiserror::Error;

use crate::{actor::Actor, error::AppError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DownloadRange {
    pub start: u64,
    pub end: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DownloadSource {
    pub library_id: LibraryId,
    pub item_id: ItemId,
    pub blob_id: BlobId,
    pub storage_identity: StorageKey,
    pub byte_size: u64,
    pub file_name: String,
}

#[derive(Debug, Error)]
#[error("download repository failed")]
pub struct DownloadRepositoryError;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DownloadAuthorization {
    Granted(DownloadSource),
    Forbidden,
    NotFound,
}

#[async_trait]
pub trait DownloadRepository: Send + Sync {
    async fn authorize_download(
        &self,
        actor: Actor,
        item_id: ItemId,
        request_id: RequestId,
    ) -> Result<DownloadAuthorization, DownloadRepositoryError>;

    async fn record_download_start(
        &self,
        _actor: Actor,
        _item_id: ItemId,
        _request_id: RequestId,
        _range: DownloadRange,
    ) -> Result<bool, DownloadRepositoryError> {
        Err(DownloadRepositoryError)
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
        let authorization = self
            .repository
            .authorize_download(actor, item_id, request_id)
            .await
            .map_err(|_| AppError::DependencyUnavailable {
                code: "download_repository_unavailable",
            })?;
        let source = match authorization {
            DownloadAuthorization::Granted(source) => source,
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
        };
        Ok(DownloadGrant {
            storage_identity: source.storage_identity,
            byte_size: source.byte_size,
            safe_file_name: safe_file_name(&source.file_name),
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

fn safe_file_name(value: &str) -> String {
    let leaf = value.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned = leaf
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>();
    if cleaned.trim().is_empty() {
        "publication.epub".to_owned()
    } else {
        cleaned
    }
}
