#[path = "../../../tests/fixtures/epub/generate-fixtures.rs"]
mod fixtures;

use std::{io::Cursor, sync::Arc};

use async_trait::async_trait;
use fixtures::{FixtureEntry, epub};
use folioharbor_application::ports::{BlobStore, BlobStoreError, PromotedBlob, PublicationParser};
use folioharbor_domain::imports::blob::{BlobIdentity, StorageKey};
use folioharbor_epub::{EpubParser, EpubPath, EpubPublicationParser, ParserLimits};
use zip::CompressionMethod::Stored;

struct ArchiveBlobs(Vec<u8>);

#[async_trait]
impl BlobStore for ArchiveBlobs {
    fn candidate_key(&self, _: &BlobIdentity) -> StorageKey {
        unreachable!()
    }
    async fn create_staging_for(&self, _: &StorageKey) -> Result<(), BlobStoreError> {
        unreachable!()
    }
    async fn append(&self, _: &StorageKey, _: &[u8]) -> Result<(), BlobStoreError> {
        unreachable!()
    }
    async fn read_range(&self, _: &StorageKey, _: u64, _: u64) -> Result<Vec<u8>, BlobStoreError> {
        unreachable!()
    }
    async fn promote(
        &self,
        _: &StorageKey,
        _: &BlobIdentity,
    ) -> Result<PromotedBlob, BlobStoreError> {
        unreachable!()
    }
    async fn delete(&self, _: &StorageKey) -> Result<(), BlobStoreError> {
        unreachable!()
    }
    async fn free_bytes(&self) -> Result<u64, BlobStoreError> {
        unreachable!()
    }
    async fn open_publication(
        &self,
        _: &StorageKey,
    ) -> Result<Box<dyn folioharbor_application::ports::PublicationSource>, BlobStoreError> {
        Ok(Box::new(Cursor::new(self.0.clone())))
    }
}

#[test]
fn parses_epub_three_into_neutral_publication_data() -> anyhow::Result<()> {
    let source = fixtures::valid_epub()?;
    let before = sha256(&source);
    let publication = EpubParser::inspect(&mut Cursor::new(&source), ParserLimits::default())?;

    assert_eq!(publication.metadata.titles, ["Small Book"]);
    assert_eq!(publication.metadata.authors, ["Ada Author"]);
    assert_eq!(publication.metadata.languages, ["en"]);
    assert_eq!(publication.metadata.identifiers, ["urn:fixture:1"]);
    assert_eq!(
        publication.spine[0].href.as_str(),
        "EPUB/text/chapter.xhtml"
    );
    assert_eq!(publication.toc[0].label, "Chapter");
    assert_eq!(
        publication.toc[0].href.as_str(),
        "EPUB/text/chapter.xhtml#start"
    );
    assert_eq!(
        publication.cover.as_ref().map(EpubPath::as_str),
        Some("EPUB/images/cover.png")
    );
    assert_eq!(publication.resources.len(), 4);
    assert!(
        publication
            .resources
            .iter()
            .any(|resource| resource.href.as_str() == "EPUB/styles/book.css"
                && resource.media_type == "text/css")
    );
    assert!(
        publication
            .resources
            .iter()
            .any(|resource| resource.href.as_str() == "EPUB/images/cover.png"
                && resource.media_type == "image/png")
    );
    assert!(
        publication
            .warnings
            .iter()
            .any(|warning| warning.contains("fixture:unknown"))
    );
    assert!(
        publication
            .warnings
            .iter()
            .any(|warning| warning.contains("dc:publisher"))
    );
    assert_eq!(
        sha256(&source),
        before,
        "inspection must not mutate the source ZIP"
    );
    Ok(())
}

#[test]
fn fixture_generation_is_deterministic() -> anyhow::Result<()> {
    assert_eq!(fixtures::valid_epub()?, fixtures::valid_epub()?);
    Ok(())
}

#[test]
fn parses_epub_two_ncx_entries_into_ordered_toc() -> anyhow::Result<()> {
    let source = epub(&[
        FixtureEntry {
            path: "META-INF/container.xml",
            bytes: br#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="book.opf"/></rootfiles></container>"#,
            compression: Stored,
        },
        FixtureEntry {
            path: "book.opf",
            bytes: br#"<package xmlns="http://www.idpf.org/2007/opf" version="2.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>EPUB 2 Book</dc:title></metadata><manifest><item id="chapter-one" href="text/one.xhtml" media-type="application/xhtml+xml"/><item id="chapter-two" href="text/two.xhtml" media-type="application/xhtml+xml"/><item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/></manifest><spine toc="ncx"><itemref idref="chapter-one"/><itemref idref="chapter-two"/></spine></package>"#,
            compression: Stored,
        },
        FixtureEntry {
            path: "toc.ncx",
            bytes: br#"<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/"><navMap><navPoint id="one"><navLabel><text>One</text></navLabel><content src="text/one.xhtml#start"/><navPoint id="two"><navLabel><text>Two</text></navLabel><content src="text/two.xhtml"/></navPoint></navPoint></navMap></ncx>"#,
            compression: Stored,
        },
        FixtureEntry {
            path: "text/one.xhtml",
            bytes: b"<html/>",
            compression: Stored,
        },
        FixtureEntry {
            path: "text/two.xhtml",
            bytes: b"<html/>",
            compression: Stored,
        },
    ])?;

    let publication = EpubParser::inspect(&mut Cursor::new(source), ParserLimits::default())?;

    assert_eq!(publication.toc.len(), 2);
    assert_eq!(publication.toc[0].label, "One");
    assert_eq!(publication.toc[0].href.as_str(), "text/one.xhtml#start");
    assert_eq!(publication.toc[1].label, "Two");
    assert_eq!(publication.toc[1].href.as_str(), "text/two.xhtml");
    Ok(())
}

#[test]
fn accepts_supported_epub_two_patch_and_epub_three_minor_versions() -> anyhow::Result<()> {
    let epub_two = epub(&[
        FixtureEntry { path: "META-INF/container.xml", bytes: br#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="book.opf"/></rootfiles></container>"#, compression: Stored },
        FixtureEntry { path: "book.opf", bytes: br#"<package xmlns="http://www.idpf.org/2007/opf" version="2.0.1"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>EPUB 2.0.1</dc:title></metadata><manifest><item id="chapter" href="chapter.xhtml" media-type="application/xhtml+xml"/><item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/></manifest><spine toc="ncx"><itemref idref="chapter"/></spine></package>"#, compression: Stored },
        FixtureEntry { path: "chapter.xhtml", bytes: b"<html/>", compression: Stored },
        FixtureEntry { path: "toc.ncx", bytes: br#"<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/"><navMap><navPoint><navLabel><text>Chapter</text></navLabel><content src="chapter.xhtml"/></navPoint></navMap></ncx>"#, compression: Stored },
    ])?;
    let epub_three = epub(&[
        FixtureEntry { path: "META-INF/container.xml", bytes: br#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="book.opf"/></rootfiles></container>"#, compression: Stored },
        FixtureEntry { path: "book.opf", bytes: br#"<package xmlns="http://www.idpf.org/2007/opf" version="3.3"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>EPUB 3.3</dc:title></metadata><manifest><item id="chapter" href="chapter.xhtml" media-type="application/xhtml+xml"/><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/></manifest><spine><itemref idref="chapter"/></spine></package>"#, compression: Stored },
        FixtureEntry { path: "chapter.xhtml", bytes: b"<html/>", compression: Stored },
        FixtureEntry { path: "nav.xhtml", bytes: br#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><body><nav epub:type="toc"><a href="chapter.xhtml">Chapter</a></nav></body></html>"#, compression: Stored },
    ])?;

    assert_eq!(
        EpubParser::inspect(&mut Cursor::new(epub_two), ParserLimits::default())?
            .metadata
            .titles,
        ["EPUB 2.0.1"]
    );
    assert_eq!(
        EpubParser::inspect(&mut Cursor::new(epub_three), ParserLimits::default())?
            .metadata
            .titles,
        ["EPUB 3.3"]
    );
    Ok(())
}

#[test]
fn uses_unique_epub_two_ncx_without_spine_toc() -> anyhow::Result<()> {
    let source = epub(&[
        FixtureEntry { path: "META-INF/container.xml", bytes: br#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="book.opf"/></rootfiles></container>"#, compression: Stored },
        FixtureEntry { path: "book.opf", bytes: br#"<package xmlns="http://www.idpf.org/2007/opf" version="2.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Fallback NCX</dc:title></metadata><manifest><item id="chapter" href="chapter.xhtml" media-type="text/html"/><item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/></manifest><spine><itemref idref="chapter"/></spine></package>"#, compression: Stored },
        FixtureEntry { path: "chapter.xhtml", bytes: b"<html/>", compression: Stored },
        FixtureEntry { path: "toc.ncx", bytes: br#"<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/"><navMap><navPoint><navLabel><text>Chapter</text></navLabel><content src="chapter.xhtml"/></navPoint></navMap></ncx>"#, compression: Stored },
    ])?;

    let publication = EpubParser::inspect(&mut Cursor::new(source), ParserLimits::default())?;

    assert_eq!(publication.toc[0].label, "Chapter");
    assert_eq!(publication.spine[0].href.as_str(), "chapter.xhtml");
    assert!(
        publication
            .warnings
            .iter()
            .any(|warning| warning.contains("fallback") && warning.contains("NCX"))
    );
    assert!(
        publication
            .resources
            .iter()
            .any(|resource| resource.href.as_str() == "chapter.xhtml"
                && resource.media_type == "application/xhtml+xml")
    );
    Ok(())
}

#[test]
fn keeps_epub_three_nav_precedence_over_ncx_alternative() -> anyhow::Result<()> {
    let source = epub(&[
        FixtureEntry { path: "META-INF/container.xml", bytes: br#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="book.opf"/></rootfiles></container>"#, compression: Stored },
        FixtureEntry { path: "book.opf", bytes: br#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Preferred nav</dc:title></metadata><manifest><item id="chapter" href="chapter.xhtml" media-type="application/xhtml+xml"/><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/><item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/></manifest><spine><itemref idref="chapter"/></spine></package>"#, compression: Stored },
        FixtureEntry { path: "chapter.xhtml", bytes: b"<html/>", compression: Stored },
        FixtureEntry { path: "nav.xhtml", bytes: br#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><body><nav epub:type="toc"><a href="chapter.xhtml">HTML nav</a></nav></body></html>"#, compression: Stored },
        FixtureEntry { path: "toc.ncx", bytes: br#"<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/"><navMap><navPoint><navLabel><text>NCX</text></navLabel><content src="chapter.xhtml"/></navPoint></navMap></ncx>"#, compression: Stored },
    ])?;

    let publication = EpubParser::inspect(&mut Cursor::new(source), ParserLimits::default())?;

    assert_eq!(publication.toc[0].label, "HTML nav");
    assert!(publication.warnings.is_empty());
    Ok(())
}

#[test]
fn derives_toc_from_readable_spine_when_navigation_is_absent() -> anyhow::Result<()> {
    let source = epub(&[
        FixtureEntry { path: "META-INF/container.xml", bytes: br#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="book.opf"/></rootfiles></container>"#, compression: Stored },
        FixtureEntry { path: "book.opf", bytes: br#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Spine TOC</dc:title></metadata><manifest><item id="first" href="text/first.xhtml" media-type="application/xhtml+xml"/><item id="second" href="text/second.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="first"/><itemref idref="second"/></spine></package>"#, compression: Stored },
        FixtureEntry { path: "text/first.xhtml", bytes: b"<html/>", compression: Stored },
        FixtureEntry { path: "text/second.xhtml", bytes: b"<html/>", compression: Stored },
    ])?;

    let publication = EpubParser::inspect(&mut Cursor::new(source), ParserLimits::default())?;

    assert_eq!(publication.toc.len(), 2);
    assert_eq!(publication.toc[0].label, "text/first.xhtml");
    assert_eq!(publication.toc[1].href.as_str(), "text/second.xhtml");
    assert!(
        publication
            .warnings
            .iter()
            .any(|warning| warning.contains("spine"))
    );
    Ok(())
}

#[test]
fn keeps_navigation_fallback_warning_after_metadata_warning_saturation() -> anyhow::Result<()> {
    let metadata = (0..32)
        .map(|index| format!(r#"<meta property="unknown-{index}">value</meta>"#))
        .collect::<String>();
    let package = format!(
        r#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Warnings</dc:title>{metadata}</metadata><manifest><item id="chapter" href="chapter.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="chapter"/></spine></package>"#
    );
    let source = epub(&[
        FixtureEntry { path: "META-INF/container.xml", bytes: br#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="book.opf"/></rootfiles></container>"#, compression: Stored },
        FixtureEntry { path: "book.opf", bytes: package.as_bytes(), compression: Stored },
        FixtureEntry { path: "chapter.xhtml", bytes: b"<html/>", compression: Stored },
    ])?;

    let publication = EpubParser::inspect(&mut Cursor::new(source), ParserLimits::default())?;

    assert_eq!(publication.warnings.len(), 32);
    assert!(
        publication
            .warnings
            .iter()
            .any(|warning| warning.contains("generated table of contents"))
    );
    Ok(())
}

#[test]
fn uses_epub_two_manifest_cover_before_guide_cover() -> anyhow::Result<()> {
    let source = epub(&[
        FixtureEntry { path: "META-INF/container.xml", bytes: br#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="book.opf"/></rootfiles></container>"#, compression: Stored },
        FixtureEntry { path: "book.opf", bytes: br#"<package xmlns="http://www.idpf.org/2007/opf" version="2.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Cover</dc:title><meta name="cover" content="manifest-cover"/></metadata><manifest><item id="chapter" href="chapter.xhtml" media-type="application/xhtml+xml"/><item id="manifest-cover" href="cover.jpg" media-type="image/jpeg"/></manifest><spine><itemref idref="chapter"/></spine><guide><reference type="cover" href="guide.xhtml"/></guide></package>"#, compression: Stored },
        FixtureEntry { path: "chapter.xhtml", bytes: b"<html/>", compression: Stored },
        FixtureEntry { path: "cover.jpg", bytes: b"jpg", compression: Stored },
        FixtureEntry { path: "guide.xhtml", bytes: b"<html/>", compression: Stored },
    ])?;

    let publication = EpubParser::inspect(&mut Cursor::new(source), ParserLimits::default())?;

    assert_eq!(
        publication.cover.as_ref().map(EpubPath::as_str),
        Some("cover.jpg")
    );
    Ok(())
}

#[test]
fn uses_epub_two_guide_cover_when_manifest_cover_is_absent() -> anyhow::Result<()> {
    let source = epub(&[
        FixtureEntry { path: "META-INF/container.xml", bytes: br#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="book.opf"/></rootfiles></container>"#, compression: Stored },
        FixtureEntry { path: "book.opf", bytes: br#"<package xmlns="http://www.idpf.org/2007/opf" version="2.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Guide cover</dc:title></metadata><manifest><item id="chapter" href="chapter.xhtml" media-type="application/xhtml+xml"/><item id="guide" href="guide.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="chapter"/></spine><guide><reference type="cover" href="guide.xhtml"/></guide></package>"#, compression: Stored },
        FixtureEntry { path: "chapter.xhtml", bytes: b"<html/>", compression: Stored },
        FixtureEntry { path: "guide.xhtml", bytes: b"<html/>", compression: Stored },
    ])?;

    let publication = EpubParser::inspect(&mut Cursor::new(source), ParserLimits::default())?;

    assert_eq!(
        publication.cover.as_ref().map(EpubPath::as_str),
        Some("guide.xhtml")
    );
    Ok(())
}

#[tokio::test]
async fn omits_unmanifested_guide_cover_from_catalog_publication() -> anyhow::Result<()> {
    let source = epub(&[
        FixtureEntry { path: "META-INF/container.xml", bytes: br#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="book.opf"/></rootfiles></container>"#, compression: Stored },
        FixtureEntry { path: "book.opf", bytes: br#"<package xmlns="http://www.idpf.org/2007/opf" version="2.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Guide cover</dc:title></metadata><manifest><item id="chapter" href="chapter.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="chapter"/></spine><guide><reference type="cover" href="guide.xhtml"/></guide></package>"#, compression: Stored },
        FixtureEntry { path: "chapter.xhtml", bytes: b"<html/>", compression: Stored },
        FixtureEntry { path: "guide.xhtml", bytes: b"<html/>", compression: Stored },
    ])?;
    let parser =
        EpubPublicationParser::new(Arc::new(ArchiveBlobs(source)), ParserLimits::default());

    let publication = parser
        .parse(&StorageKey::from_opaque("fixture".to_owned()))
        .await?;

    assert_eq!(publication.cover_href(), None);
    Ok(())
}

fn sha256(input: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(input).into()
}
