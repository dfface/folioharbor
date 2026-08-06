use async_trait::async_trait;
use folioharbor_domain::{
    catalog::CatalogPublication,
    id::{
        BlobId, ItemId, LibraryId, ManifestationId, PublicationPackageId, RequestId, UploadId,
        UserId,
    },
    imports::blob::ByteCount,
    time::OffsetDateTime,
};
use thiserror::Error;

use crate::{
    catalog::{ImportCatalogCommand, ImportCatalogResult},
    error::AppError,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FinalizeCatalog {
    pub library_id: LibraryId,
    pub upload_id: UploadId,
    pub actor_id: UserId,
    pub original_blob_id: BlobId,
    pub logical_bytes: ByteCount,
    pub parser_profile_version: String,
    pub publication: CatalogPublication,
    pub request_id: RequestId,
    pub now: OffsetDateTime,
}

impl From<ImportCatalogCommand> for FinalizeCatalog {
    fn from(value: ImportCatalogCommand) -> Self {
        Self {
            library_id: value.library_id,
            upload_id: value.upload_id,
            actor_id: value.actor_id,
            original_blob_id: value.original_blob_id,
            logical_bytes: value.logical_bytes,
            parser_profile_version: value.parser_profile_version,
            publication: value.publication,
            request_id: value.request_id,
            now: value.now,
        }
    }
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum CatalogRepositoryError {
    #[error("catalog reservation is not active")]
    ReservationNotActive,
    #[error("catalog persistence failed")]
    Persistence,
}

impl From<CatalogRepositoryError> for AppError {
    fn from(value: CatalogRepositoryError) -> Self {
        match value {
            CatalogRepositoryError::ReservationNotActive => AppError::Conflict {
                code: "upload_not_importable",
            },
            CatalogRepositoryError::Persistence => AppError::DependencyUnavailable {
                code: "catalog_repository_unavailable",
            },
        }
    }
}

#[async_trait]
pub trait CatalogRepository: Send + Sync {
    /// Atomically writes catalog, consumes quota, advances the upload, and records success.
    async fn finalize(
        &self,
        command: FinalizeCatalog,
    ) -> Result<ImportCatalogResult, CatalogRepositoryError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VisibleCatalogItem {
    pub item_id: ItemId,
    pub manifestation_id: ManifestationId,
    pub package_id: PublicationPackageId,
    pub primary_title: String,
}

#[async_trait]
pub trait CatalogQueryRepository: Send + Sync {
    /// Resolves one Item through its visible Holding; global WEMI enumeration is not exposed.
    async fn find_visible_item(
        &self,
        actor_id: UserId,
        library_id: LibraryId,
        item_id: ItemId,
        membership_version: i64,
        request_id: RequestId,
    ) -> Result<Option<VisibleCatalogItem>, CatalogRepositoryError>;
}
