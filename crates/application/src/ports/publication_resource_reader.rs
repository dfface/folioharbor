use async_trait::async_trait;
use folioharbor_domain::{
    id::{BlobId, ItemId, LibraryId, ManifestationId, PublicationPackageId, RequestId, UserId},
    imports::blob::StorageKey,
};
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReaderResource {
    pub normalized_href: String,
    pub media_type: String,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReaderSpineEntry {
    pub normalized_href: String,
    pub linear: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReaderTocEntry {
    pub label: String,
    pub normalized_href: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReaderPublication {
    pub library_id: LibraryId,
    pub item_id: ItemId,
    pub manifestation_id: ManifestationId,
    pub package_id: PublicationPackageId,
    pub blob_id: BlobId,
    pub storage_key: StorageKey,
    pub parser_profile_version: String,
    pub primary_title: String,
    pub authors: Vec<String>,
    pub languages: Vec<String>,
    pub resources: Vec<ReaderResource>,
    pub reading_order: Vec<ReaderSpineEntry>,
    pub toc: Vec<ReaderTocEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceReadRequest {
    pub item_id: ItemId,
    pub blob_id: BlobId,
    pub storage_key: StorageKey,
    pub package_id: PublicationPackageId,
    pub normalized_href: String,
    pub media_type: String,
    pub resources: Vec<ReaderResource>,
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("reader catalog persistence failed")]
pub struct ReaderCatalogError;

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ResourceReaderError {
    #[error("publication resource is malformed")]
    Malformed,
    #[error("publication resource is unavailable")]
    Unavailable,
}

#[async_trait]
pub trait ReaderCatalogRepository: Send + Sync {
    /// Resolves an active package only when the actor can read it on this request.
    async fn find_readable_publication(
        &self,
        actor: UserId,
        item_id: ItemId,
        request_id: RequestId,
    ) -> Result<Option<ReaderPublication>, ReaderCatalogError>;
}

#[async_trait]
pub trait PublicationResourceReader: Send + Sync {
    async fn read(&self, request: ResourceReadRequest) -> Result<Vec<u8>, ResourceReaderError>;
}
