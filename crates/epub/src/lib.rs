mod archive;
mod container;
mod error;
mod navigation;
mod package;
mod sanitize;

use std::{
    io::{Read, Seek},
    time::Duration,
};

pub use error::{EpubError, EpubErrorCode};
pub use package::{Metadata, ParsedPublication, PublicationResource, SpineItem, TocEntry};
pub use sanitize::{ContentSanitizer, ResourceResolver, SanitizedContent};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct EpubPath(String);

impl EpubPath {
    /// Validates and normalizes an EPUB-internal path.
    ///
    /// # Errors
    ///
    /// Returns an error when the path is absolute, escaping, or otherwise unsafe.
    pub fn new(path: &str) -> Result<Self, EpubError> {
        archive::normalize_path(path, false)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn from_normalized(path: String) -> Self {
        Self(path)
    }

    fn resolve_from(base: &str, reference: &str) -> Result<Self, EpubError> {
        archive::resolve_path(base, reference)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ParserLimits {
    pub max_entries: usize,
    pub max_total_uncompressed_bytes: u64,
    pub max_compression_ratio: u64,
    pub max_path_depth: usize,
    pub max_xml_depth: usize,
    pub max_resource_bytes: u64,
    pub deadline: Duration,
}

impl Default for ParserLimits {
    fn default() -> Self {
        Self {
            max_entries: 4_096,
            max_total_uncompressed_bytes: 512 * 1024 * 1024,
            max_compression_ratio: 200,
            max_path_depth: 32,
            max_xml_depth: 64,
            max_resource_bytes: 64 * 1024 * 1024,
            deadline: Duration::from_secs(30),
        }
    }
}

pub struct EpubParser;

impl EpubParser {
    /// Inspects a bounded EPUB archive and returns format-neutral publication data.
    ///
    /// # Errors
    ///
    /// Returns a stable EPUB error when the archive exceeds limits or is malformed.
    pub fn inspect<R: Read + Seek>(
        source: &mut R,
        limits: ParserLimits,
    ) -> Result<ParsedPublication, EpubError> {
        let archive = archive::BoundedArchive::read(source, limits)?;
        let package_path = container::package_path(&archive)?;
        package::parse(&archive, &package_path)
    }
}
