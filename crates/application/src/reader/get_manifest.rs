use crate::{
    error::AppError,
    ports::{ReaderCatalogRepository, ReaderPublication},
};
use folioharbor_domain::id::{ItemId, ManifestationId, RequestId, UserId};
use std::collections::{HashMap, HashSet};

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
        Ok(project(&publication))
    }
}

fn project(publication: &ReaderPublication) -> PublicationManifest {
    let item = publication.item_id.as_uuid();
    let resources_by_href = publication
        .resources
        .iter()
        .map(|resource| (resource.normalized_href.as_str(), resource))
        .collect::<HashMap<_, _>>();
    let reading_hrefs = publication
        .reading_order
        .iter()
        .map(|entry| entry.normalized_href.as_str())
        .collect::<HashSet<_>>();
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
            resources_by_href
                .get(entry.normalized_href.as_str())
                .map(|resource| make_link(&resource.normalized_href, &resource.media_type, None))
        })
        .collect();
    let resources = publication
        .resources
        .iter()
        .filter(|resource| !reading_hrefs.contains(resource.normalized_href.as_str()))
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
            resources_by_href.get(href).map(|resource| {
                let mut link = make_link(href, &resource.media_type, Some(entry.label.clone()));
                if let Some(fragment) = fragment {
                    link.href.push('#');
                    link.href.push_str(&encode_fragment(fragment));
                }
                link
            })
        })
        .collect();
    PublicationManifest {
        metadata: ManifestMetadata {
            title: publication.primary_title.clone(),
            authors: publication.authors.to_vec(),
            languages: publication.languages.to_vec(),
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

fn encode_fragment(fragment: &str) -> String {
    let mut encoded = String::with_capacity(fragment.len());
    for byte in fragment.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

fn resource_url(item: uuid::Uuid, resource: &ResourceId) -> String {
    format!("/api/v1/items/{item}/resources/{}", resource.as_str())
}
