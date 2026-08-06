use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};

use async_trait::async_trait;
use folioharbor_domain::{
    id::{BlobId, ItemId, LibraryId, ManifestationId, PublicationPackageId, RequestId, UserId},
    imports::blob::StorageKey,
};
use thiserror::Error;

use crate::reader::ResourceId;

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
    pub authors: Arc<[String]>,
    pub languages: Arc<[String]>,
    pub resources: Arc<[ReaderResource]>,
    pub reading_order: Arc<[ReaderSpineEntry]>,
    pub toc: Arc<[ReaderTocEntry]>,
    indexes: Arc<OnceLock<ReaderPublicationIndexes>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ReaderPublicationIndexes {
    by_id: HashMap<ResourceId, usize>,
    routes: Arc<HashMap<String, String>>,
}

impl ReaderPublication {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        library_id: LibraryId,
        item_id: ItemId,
        manifestation_id: ManifestationId,
        package_id: PublicationPackageId,
        blob_id: BlobId,
        storage_key: StorageKey,
        parser_profile_version: String,
        primary_title: String,
        authors: Vec<String>,
        languages: Vec<String>,
        resources: Vec<ReaderResource>,
        reading_order: Vec<ReaderSpineEntry>,
        toc: Vec<ReaderTocEntry>,
    ) -> Self {
        Self {
            library_id,
            item_id,
            manifestation_id,
            package_id,
            blob_id,
            storage_key,
            parser_profile_version,
            primary_title,
            authors: authors.into(),
            languages: languages.into(),
            resources: resources.into(),
            reading_order: reading_order.into(),
            toc: toc.into(),
            indexes: Arc::new(OnceLock::new()),
        }
    }

    fn indexes(&self) -> &ReaderPublicationIndexes {
        self.indexes.get_or_init(|| {
            let mut by_id = HashMap::with_capacity(self.resources.len());
            let mut routes = HashMap::with_capacity(self.resources.len());
            for (index, resource) in self.resources.iter().enumerate() {
                let id = ResourceId::for_resource(self.package_id, &resource.normalized_href);
                routes.insert(resource.normalized_href.clone(), id.as_str().to_owned());
                by_id.insert(id, index);
            }
            ReaderPublicationIndexes {
                by_id,
                routes: Arc::new(routes),
            }
        })
    }

    #[must_use]
    pub fn resource_by_id(&self, id: &ResourceId) -> Option<&ReaderResource> {
        self.indexes()
            .by_id
            .get(id)
            .map(|index| &self.resources[*index])
    }

    #[must_use]
    pub fn resource_routes(&self) -> Arc<HashMap<String, String>> {
        self.indexes().routes.clone()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceReadRequest {
    pub item_id: ItemId,
    pub blob_id: BlobId,
    pub storage_key: StorageKey,
    pub package_id: PublicationPackageId,
    pub normalized_href: String,
    pub media_type: String,
    pub resource_routes: Arc<HashMap<String, String>>,
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
    ) -> Result<Option<Arc<ReaderPublication>>, ReaderCatalogError>;
}

#[async_trait]
pub trait PublicationResourceReader: Send + Sync {
    async fn read(&self, request: ResourceReadRequest) -> Result<Vec<u8>, ResourceReaderError>;
}
