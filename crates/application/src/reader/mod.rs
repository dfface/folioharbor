mod get_manifest;
mod get_resource;

use async_trait::async_trait;
use folioharbor_domain::id::{ItemId, RequestId, UserId};

use crate::{
    error::AppError,
    ports::{PublicationResourceReader, ReaderCatalogRepository},
};

pub use get_manifest::{
    GetPublicationManifest, ManifestLink, ManifestMetadata, PublicationManifest,
};
pub use get_resource::{GetPublicationResource, ResourceId, ResourceIdError, ResourceResponse};

#[async_trait]
pub trait ReaderApi: Send + Sync {
    async fn get_manifest(
        &self,
        actor: UserId,
        item_id: ItemId,
        request_id: RequestId,
    ) -> Result<PublicationManifest, AppError>;
    async fn get_resource(
        &self,
        actor: UserId,
        item_id: ItemId,
        resource_id: ResourceId,
        request_id: RequestId,
    ) -> Result<ResourceResponse, AppError>;
}

pub struct UnavailableReaderApi;

#[async_trait]
impl ReaderApi for UnavailableReaderApi {
    async fn get_manifest(
        &self,
        _: UserId,
        _: ItemId,
        _: RequestId,
    ) -> Result<PublicationManifest, AppError> {
        Err(AppError::DependencyUnavailable {
            code: "reader_catalog_unavailable",
        })
    }
    async fn get_resource(
        &self,
        _: UserId,
        _: ItemId,
        _: ResourceId,
        _: RequestId,
    ) -> Result<ResourceResponse, AppError> {
        Err(AppError::DependencyUnavailable {
            code: "publication_resource_unavailable",
        })
    }
}

pub struct ReaderService<C, R> {
    catalog: C,
    resources: R,
}
impl<C, R> ReaderService<C, R> {
    #[must_use]
    pub const fn new(catalog: C, resources: R) -> Self {
        Self { catalog, resources }
    }
}

#[async_trait]
impl<C: ReaderCatalogRepository, R: PublicationResourceReader> ReaderApi for ReaderService<C, R> {
    async fn get_manifest(
        &self,
        actor: UserId,
        item_id: ItemId,
        request_id: RequestId,
    ) -> Result<PublicationManifest, AppError> {
        GetPublicationManifest::new(&self.catalog)
            .execute(actor, item_id, request_id)
            .await
    }
    async fn get_resource(
        &self,
        actor: UserId,
        item_id: ItemId,
        resource_id: ResourceId,
        request_id: RequestId,
    ) -> Result<ResourceResponse, AppError> {
        GetPublicationResource::new(&self.catalog, &self.resources)
            .execute(actor, item_id, resource_id, request_id)
            .await
    }
}
