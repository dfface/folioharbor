use async_trait::async_trait;
use folioharbor_domain::{
    catalog::CatalogPublication,
    id::{
        BlobId, HoldingId, ItemId, LibraryId, ManifestationId, PublicationPackageId, RequestId,
        UploadId, UserId,
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
    pub holding_id: HoldingId,
    pub item_id: ItemId,
    pub manifestation_id: ManifestationId,
    pub package_id: PublicationPackageId,
    pub primary_title: String,
    pub authors: Vec<String>,
    pub languages: Vec<String>,
    pub identifiers: Vec<String>,
    pub media_type: String,
}

#[async_trait]
pub trait CatalogQueryRepository: Send + Sync {
    /// Lists bounded visible rows from Holding inward with one SQL query.
    async fn list_visible_items(
        &self,
        grant: crate::authorization::AuthorizationGrant,
        library_id: LibraryId,
        after: Option<HoldingId>,
        limit: u32,
        request_id: RequestId,
    ) -> Result<Vec<VisibleCatalogItem>, CatalogRepositoryError>;

    /// Resolves one Item through its visible Holding; global WEMI enumeration is not exposed.
    async fn find_visible_item(
        &self,
        grant: crate::authorization::AuthorizationGrant,
        library_id: LibraryId,
        item_id: ItemId,
        request_id: RequestId,
    ) -> Result<Option<VisibleCatalogItem>, CatalogRepositoryError>;
}
