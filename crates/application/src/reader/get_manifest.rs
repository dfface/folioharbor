use crate::{
    error::AppError,
    ports::{ReaderCatalogRepository, ReaderPublication},
};
use folioharbor_domain::id::{ItemId, ManifestationId, RequestId, UserId};

use super::ResourceId;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestMetadata {
    pub title: String,
    pub authors: Vec<String>,
    pub languages: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestLink {
    pub href: String,
    pub media_type: String,
    pub relation: String,
    pub title: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicationManifest {
    pub metadata: ManifestMetadata,
    pub manifestation_id: ManifestationId,
    pub reading_order: Vec<ManifestLink>,
    pub resources: Vec<ManifestLink>,
    pub toc: Vec<ManifestLink>,
    pub links: Vec<ManifestLink>,
    pub etag: String,
}

pub struct GetPublicationManifest<'a, R: ?Sized> {
    repository: &'a R,
}
impl<'a, R: ?Sized> GetPublicationManifest<'a, R> {
    #[must_use]
    pub const fn new(repository: &'a R) -> Self {
        Self { repository }
    }
}

impl<R: ReaderCatalogRepository + ?Sized> GetPublicationManifest<'_, R> {
    /// # Errors
    /// Returns anti-enumerating not-found or a stable dependency error.
    pub async fn execute(
        &self,
        actor: UserId,
        item_id: ItemId,
        request_id: RequestId,
    ) -> Result<PublicationManifest, AppError> {
        let publication = self
            .repository
            .find_readable_publication(actor, item_id, request_id)
            .await
            .map_err(|_| AppError::DependencyUnavailable {
                code: "reader_catalog_unavailable",
            })?
            .ok_or(AppError::NotFound {
                code: "item_not_found",
            })?;
        Ok(project(publication))
    }
}

fn project(publication: ReaderPublication) -> PublicationManifest {
    let item = publication.item_id.as_uuid();
    let make_link = |href: &str, media_type: &str, title: Option<String>| ManifestLink {
        href: resource_url(
            item,
            &ResourceId::for_resource(publication.package_id, href),
        ),
        media_type: media_type.to_owned(),
        relation: String::new(),
        title,
    };
    let reading_order = publication
        .reading_order
        .iter()
        .filter_map(|entry| {
            publication
                .resources
                .iter()
                .find(|resource| resource.normalized_href == entry.normalized_href)
                .map(|resource| make_link(&resource.normalized_href, &resource.media_type, None))
        })
        .collect();
    let resources = publication
        .resources
        .iter()
        .filter(|resource| {
            !publication
                .reading_order
                .iter()
                .any(|entry| entry.normalized_href == resource.normalized_href)
        })
        .map(|resource| make_link(&resource.normalized_href, &resource.media_type, None))
        .collect();
    let toc = publication
        .toc
        .iter()
        .filter_map(|entry| {
            let (href, fragment) = entry.normalized_href.split_once('#').map_or(
                (entry.normalized_href.as_str(), None),
                |(href, fragment)| (href, Some(fragment)),
            );
            publication
                .resources
                .iter()
                .find(|resource| resource.normalized_href == href)
                .map(|resource| {
                    let mut link = make_link(href, &resource.media_type, Some(entry.label.clone()));
                    if let Some(fragment) = fragment {
                        link.href.push('#');
                        link.href.push_str(fragment);
                    }
                    link
                })
        })
        .collect();
    PublicationManifest {
        metadata: ManifestMetadata {
            title: publication.primary_title,
            authors: publication.authors,
            languages: publication.languages,
        },
        manifestation_id: publication.manifestation_id,
        reading_order,
        resources,
        toc,
        links: vec![ManifestLink {
            href: format!("/api/v1/items/{item}/manifest"),
            media_type: "application/webpub+json".to_owned(),
            relation: "self".to_owned(),
            title: None,
        }],
        etag: format!(
            "\"package-{}-{}\"",
            publication.package_id.as_uuid(),
            publication.parser_profile_version
        ),
    }
}

fn resource_url(item: uuid::Uuid, resource: &ResourceId) -> String {
    format!("/api/v1/items/{item}/resources/{}", resource.as_str())
}
