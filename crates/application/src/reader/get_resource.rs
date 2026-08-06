use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use folioharbor_domain::id::{ItemId, PublicationPackageId, RequestId, UserId};
use sha2::{Digest as _, Sha256};
use thiserror::Error;

use crate::{
    error::AppError,
    ports::{
        PublicationResourceReader, ReaderCatalogRepository, ResourceReadRequest,
        ResourceReaderError,
    },
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ResourceId(String);

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
#[error("resource identifier is invalid")]
pub struct ResourceIdError;

impl ResourceId {
    #[must_use]
    pub fn for_resource(package: PublicationPackageId, normalized_href: &str) -> Self {
        let mut digest = Sha256::new();
        digest.update(package.as_uuid().as_bytes());
        digest.update([0]);
        digest.update(normalized_href.as_bytes());
        Self(URL_SAFE_NO_PAD.encode(digest.finalize()))
    }
    /// # Errors
    /// Rejects empty, oversized, or non URL-safe identifiers.
    pub fn parse(value: &str) -> Result<Self, ResourceIdError> {
        if value.is_empty()
            || value.len() > 128
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(ResourceIdError);
        }
        Ok(Self(value.to_owned()))
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceResponse {
    pub bytes: Vec<u8>,
    pub media_type: String,
    pub etag: String,
}

pub struct GetPublicationResource<'a, C: ?Sized, R: ?Sized> {
    catalog: &'a C,
    reader: &'a R,
}
impl<'a, C: ?Sized, R: ?Sized> GetPublicationResource<'a, C, R> {
    #[must_use]
    pub const fn new(catalog: &'a C, reader: &'a R) -> Self {
        Self { catalog, reader }
    }
}
impl<C: ReaderCatalogRepository + ?Sized, R: PublicationResourceReader + ?Sized>
    GetPublicationResource<'_, C, R>
{
    /// # Errors
    /// Returns not-found after fresh authorization, or stable transformation/dependency errors.
    pub async fn execute(
        &self,
        actor: UserId,
        item_id: ItemId,
        resource_id: ResourceId,
        request_id: RequestId,
    ) -> Result<ResourceResponse, AppError> {
        let publication = self
            .catalog
            .find_readable_publication(actor, item_id, request_id)
            .await
            .map_err(|_| AppError::DependencyUnavailable {
                code: "reader_catalog_unavailable",
            })?
            .ok_or(AppError::NotFound {
                code: "item_not_found",
            })?;
        let resource = publication
            .resources
            .iter()
            .find(|resource| {
                ResourceId::for_resource(publication.package_id, &resource.normalized_href)
                    == resource_id
            })
            .cloned()
            .ok_or(AppError::NotFound {
                code: "resource_not_found",
            })?;
        let bytes = self
            .reader
            .read(ResourceReadRequest {
                item_id: publication.item_id,
                blob_id: publication.blob_id,
                storage_key: publication.storage_key,
                package_id: publication.package_id,
                normalized_href: resource.normalized_href,
                media_type: resource.media_type.clone(),
                resources: publication.resources,
            })
            .await
            .map_err(map_reader_error)?;
        Ok(ResourceResponse {
            bytes,
            media_type: resource.media_type,
            etag: format!(
                "\"resource-{}-{}-{}-sanitizer-v2\"",
                publication.item_id.as_uuid(),
                publication.package_id.as_uuid(),
                resource_id.as_str()
            ),
        })
    }
}

fn map_reader_error(error: ResourceReaderError) -> AppError {
    match error {
        ResourceReaderError::Malformed => AppError::Invalid {
            code: "publication_resource_malformed",
            fields: Vec::new(),
        },
        ResourceReaderError::Unavailable => AppError::DependencyUnavailable {
            code: "publication_resource_unavailable",
        },
    }
}
