use std::collections::BTreeSet;

use super::{
    CatalogMetadata, CatalogValueError, SpineEntry, TocEntry, content_unit::normalized_href,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicationResource {
    href: String,
    media_type: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogPublication {
    metadata: CatalogMetadata,
    resources: Vec<PublicationResource>,
    spine: Vec<SpineEntry>,
    toc: Vec<TocEntry>,
    cover_href: Option<String>,
}

impl PublicationResource {
    /// # Errors
    /// Returns an error for a non-internal href or invalid media type.
    pub fn new(
        href: impl Into<String>,
        media_type: impl Into<String>,
    ) -> Result<Self, CatalogValueError> {
        let media_type = media_type.into();
        if media_type.is_empty()
            || media_type.len() > 255
            || !media_type.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'+' | b'-' | b'.')
            })
        {
            return Err(CatalogValueError::InvalidMetadata);
        }
        Ok(Self {
            href: normalized_href(href.into())?,
            media_type,
        })
    }

    #[must_use]
    pub fn href(&self) -> &str {
        &self.href
    }

    #[must_use]
    pub fn media_type(&self) -> &str {
        &self.media_type
    }
}

impl CatalogPublication {
    /// Constructs persistence-neutral catalog data from a completed parser result.
    ///
    /// # Errors
    /// Returns an error for duplicate resources or locators outside the manifest.
    pub fn from_parser(
        metadata: CatalogMetadata,
        resources: Vec<PublicationResource>,
        spine: Vec<SpineEntry>,
        toc: Vec<TocEntry>,
        cover_href: Option<String>,
    ) -> Result<Self, CatalogValueError> {
        let hrefs: BTreeSet<&str> = resources.iter().map(PublicationResource::href).collect();
        if resources.is_empty() || hrefs.len() != resources.len() || spine.is_empty() {
            return Err(CatalogValueError::InvalidMetadata);
        }
        if !spine
            .iter()
            .all(|entry| hrefs.contains(entry.href().split('#').next().unwrap_or_default()))
            || !toc
                .iter()
                .all(|entry| hrefs.contains(entry.href().split('#').next().unwrap_or_default()))
        {
            return Err(CatalogValueError::InvalidMetadata);
        }
        let cover_href = cover_href.map(normalized_href).transpose()?;
        if cover_href
            .as_deref()
            .is_some_and(|cover| !hrefs.contains(cover))
        {
            return Err(CatalogValueError::InvalidMetadata);
        }
        Ok(Self {
            metadata,
            resources,
            spine,
            toc,
            cover_href,
        })
    }

    #[must_use]
    pub const fn metadata(&self) -> &CatalogMetadata {
        &self.metadata
    }
    #[must_use]
    pub fn resources(&self) -> &[PublicationResource] {
        &self.resources
    }
    #[must_use]
    pub fn spine(&self) -> &[SpineEntry] {
        &self.spine
    }
    #[must_use]
    pub fn toc(&self) -> &[TocEntry] {
        &self.toc
    }
    #[must_use]
    pub fn cover_href(&self) -> Option<&str> {
        self.cover_href.as_deref()
    }
}
