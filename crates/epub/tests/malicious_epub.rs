#[path = "../../../tests/fixtures/epub/generate-fixtures.rs"]
mod fixtures;

use std::{
    io::{Cursor, Read, Seek, SeekFrom},
    time::Duration,
};

use fixtures::{FixtureEntry, epub};
use folioharbor_epub::{EpubErrorCode, EpubParser, ParserLimits};
use zip::CompressionMethod::{Deflated, Stored};

fn inspect(source: &[u8], limits: ParserLimits) -> anyhow::Result<EpubErrorCode> {
    match EpubParser::inspect(&mut Cursor::new(source), limits) {
        Ok(_) => Err(anyhow::anyhow!("malicious EPUB was accepted")),
        Err(error) => Ok(error.code()),
    }
}

#[test]
fn rejects_malformed_or_missing_package_with_stable_codes() -> anyhow::Result<()> {
    let malformed = epub(&[FixtureEntry {
        path: "META-INF/container.xml",
        bytes: b"<broken",
        compression: Stored,
    }])?;
    let missing = epub(&[FixtureEntry { path: "META-INF/container.xml", bytes: br#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="missing.opf"/></rootfiles></container>"#, compression: Stored }])?;
    assert_eq!(
        inspect(&malformed, ParserLimits::default())?,
        EpubErrorCode::InvalidContainer
    );
    assert_eq!(
        inspect(&missing, ParserLimits::default())?,
        EpubErrorCode::MissingPackage
    );
    Ok(())
}

#[test]
fn rejects_unsafe_and_duplicate_normalized_paths() -> anyhow::Result<()> {
    for path in [
        "../escape",
        "/absolute",
        "C:\\book.opf",
        "safe/../../escape",
        "nul\0path",
    ] {
        let source = epub(&[FixtureEntry {
            path,
            bytes: b"x",
            compression: Stored,
        }])?;
        assert_eq!(
            inspect(&source, ParserLimits::default())?,
            EpubErrorCode::UnsafePath,
            "{path:?}"
        );
    }
    let duplicate = epub(&[
        FixtureEntry {
            path: "EPUB/book.opf",
            bytes: b"x",
            compression: Stored,
        },
        FixtureEntry {
            path: "EPUB/./book.opf",
            bytes: b"y",
            compression: Stored,
        },
    ])?;
    assert_eq!(
        inspect(&duplicate, ParserLimits::default())?,
        EpubErrorCode::DuplicatePath
    );
    Ok(())
}

#[test]
fn enforces_all_archive_limits() -> anyhow::Result<()> {
    let two = epub(&[
        FixtureEntry {
            path: "one",
            bytes: b"1",
            compression: Stored,
        },
        FixtureEntry {
            path: "two",
            bytes: b"2",
            compression: Stored,
        },
    ])?;
    assert_eq!(
        inspect(
            &two,
            ParserLimits {
                max_entries: 1,
                ..ParserLimits::default()
            }
        )?,
        EpubErrorCode::EntryLimit
    );
    assert_eq!(
        inspect(
            &two,
            ParserLimits {
                max_total_uncompressed_bytes: 1,
                ..ParserLimits::default()
            }
        )?,
        EpubErrorCode::TotalSizeLimit
    );

    let large = epub(&[FixtureEntry {
        path: "large",
        bytes: &[b'x'; 128],
        compression: Stored,
    }])?;
    assert_eq!(
        inspect(
            &large,
            ParserLimits {
                max_resource_bytes: 64,
                ..ParserLimits::default()
            }
        )?,
        EpubErrorCode::ResourceSizeLimit
    );

    let deep = epub(&[FixtureEntry {
        path: "a/b/c/d",
        bytes: b"x",
        compression: Stored,
    }])?;
    assert_eq!(
        inspect(
            &deep,
            ParserLimits {
                max_path_depth: 3,
                ..ParserLimits::default()
            }
        )?,
        EpubErrorCode::PathDepthLimit
    );

    let compressible = epub(&[FixtureEntry {
        path: "ratio",
        bytes: &[0; 16_384],
        compression: Deflated,
    }])?;
    assert_eq!(
        inspect(
            &compressible,
            ParserLimits {
                max_compression_ratio: 2,
                ..ParserLimits::default()
            }
        )?,
        EpubErrorCode::CompressionRatioLimit
    );

    assert_eq!(
        inspect(
            &fixtures::valid_epub()?,
            ParserLimits {
                deadline: Duration::ZERO,
                ..ParserLimits::default()
            }
        )?,
        EpubErrorCode::DeadlineExceeded
    );
    Ok(())
}

#[test]
fn rejects_encrypted_entries_before_reading_content() -> anyhow::Result<()> {
    let mut source = epub(&[FixtureEntry {
        path: "secret",
        bytes: b"payload",
        compression: Stored,
    }])?;
    for signature in [b"PK\x03\x04".as_slice(), b"PK\x01\x02".as_slice()] {
        let offset = source
            .windows(4)
            .position(|window| window == signature)
            .ok_or_else(|| anyhow::anyhow!("fixture ZIP header missing"))?;
        let flag_offset = if signature[2] == 3 {
            offset + 6
        } else {
            offset + 8
        };
        source[flag_offset] |= 1;
    }
    assert_eq!(
        inspect(&source, ParserLimits::default())?,
        EpubErrorCode::EncryptedContent
    );
    Ok(())
}

#[test]
fn rejects_external_manifest_resources() -> anyhow::Result<()> {
    let source = epub(&[
        FixtureEntry { path: "META-INF/container.xml", bytes: br#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="book.opf"/></rootfiles></container>"#, compression: Stored },
        FixtureEntry { path: "book.opf", bytes: br#"<package xmlns="http://www.idpf.org/2007/opf"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Unsafe</dc:title></metadata><manifest><item id="remote" href="https://attacker.test/book" media-type="text/html"/></manifest><spine><itemref idref="remote"/></spine></package>"#, compression: Stored },
    ])?;
    assert_eq!(
        inspect(&source, ParserLimits::default())?,
        EpubErrorCode::InvalidPackage
    );
    Ok(())
}

#[test]
fn container_elements_must_use_the_epub_namespace() -> anyhow::Result<()> {
    let source = epub(&[FixtureEntry { path: "META-INF/container.xml", bytes: br#"<container xmlns="https://attacker.test/not-epub"><rootfiles><rootfile full-path="book.opf"/></rootfiles></container>"#, compression: Stored }])?;
    assert_eq!(
        inspect(&source, ParserLimits::default())?,
        EpubErrorCode::InvalidContainer
    );
    Ok(())
}

#[test]
fn rejects_excessive_xml_nesting() -> anyhow::Result<()> {
    let nested = format!(
        r#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container">{}{}<rootfile full-path="book.opf"/></container>"#,
        "<rootfiles>".repeat(9),
        "</rootfiles>".repeat(9)
    );
    let source = epub(&[FixtureEntry {
        path: "META-INF/container.xml",
        bytes: nested.as_bytes(),
        compression: Stored,
    }])?;
    assert_eq!(
        inspect(
            &source,
            ParserLimits {
                max_xml_depth: 8,
                ..ParserLimits::default()
            }
        )?,
        EpubErrorCode::XmlDepthLimit
    );
    Ok(())
}

#[test]
fn rejects_hostile_declared_sizes_before_decompression() -> anyhow::Result<()> {
    let mut oversized = epub(&[FixtureEntry {
        path: "payload",
        bytes: b"small",
        compression: Stored,
    }])?;
    patch_first_zip_u32(&mut oversized, b"PK\x03\x04", 22, u32::MAX)?;
    patch_first_zip_u32(&mut oversized, b"PK\x01\x02", 24, u32::MAX)?;
    assert_eq!(
        inspect(
            &oversized,
            ParserLimits {
                max_resource_bytes: 1_024,
                ..ParserLimits::default()
            }
        )?,
        EpubErrorCode::ResourceSizeLimit
    );

    let mut zero_compressed = epub(&[FixtureEntry {
        path: "payload",
        bytes: b"small",
        compression: Stored,
    }])?;
    patch_first_zip_u32(&mut zero_compressed, b"PK\x03\x04", 18, 0)?;
    patch_first_zip_u32(&mut zero_compressed, b"PK\x01\x02", 20, 0)?;
    assert_eq!(
        inspect(&zero_compressed, ParserLimits::default())?,
        EpubErrorCode::CompressionRatioLimit
    );
    Ok(())
}

fn patch_first_zip_u32(
    source: &mut [u8],
    signature: &[u8],
    field_offset: usize,
    value: u32,
) -> anyhow::Result<()> {
    let header = source
        .windows(signature.len())
        .position(|window| window == signature)
        .ok_or_else(|| anyhow::anyhow!("fixture ZIP header missing"))?;
    let field = source
        .get_mut(header + field_offset..header + field_offset + 4)
        .ok_or_else(|| anyhow::anyhow!("fixture ZIP header truncated"))?;
    field.copy_from_slice(&value.to_le_bytes());
    Ok(())
}

#[test]
fn preflights_hostile_entry_count_before_zip_constructor() -> anyhow::Result<()> {
    let source = raw_eocd(2, 0, 0, 0, 0);
    let mut reader = CountingReader::new(source);
    let error = EpubParser::inspect(
        &mut reader,
        ParserLimits {
            max_entries: 1,
            ..ParserLimits::default()
        },
    )
    .err()
    .ok_or_else(|| anyhow::anyhow!("hostile central count was accepted"))?;
    assert_eq!(error.code(), EpubErrorCode::EntryLimit);
    assert_eq!(reader.bytes_read, 22);
    assert_eq!(reader.seek_calls, 2, "ZIP constructor must not be reached");
    Ok(())
}

#[test]
fn preflights_central_directory_size_zip64_split_and_malformed_metadata() -> anyhow::Result<()> {
    assert_eq!(
        inspect(
            &fixtures::valid_epub()?,
            ParserLimits {
                max_central_directory_bytes: 1,
                ..ParserLimits::default()
            }
        )?,
        EpubErrorCode::CentralDirectoryLimit
    );
    assert_eq!(
        inspect(&raw_eocd(u16::MAX, 0, 0, 0, 0), ParserLimits::default())?,
        EpubErrorCode::UnsupportedArchive
    );
    let mut zip64_locator = Vec::from(b"PK\x06\x07".as_slice());
    zip64_locator.extend_from_slice(&[0; 16]);
    zip64_locator.extend_from_slice(&raw_eocd(0, 0, 0, 0, 0));
    assert_eq!(
        inspect(&zip64_locator, ParserLimits::default())?,
        EpubErrorCode::UnsupportedArchive
    );
    assert_eq!(
        inspect(&raw_eocd(0, 1, 0, 0, 0), ParserLimits::default())?,
        EpubErrorCode::UnsupportedArchive
    );
    assert_eq!(
        inspect(&raw_eocd(0, 0, 0, 10, 99), ParserLimits::default())?,
        EpubErrorCode::InvalidArchive
    );
    Ok(())
}

#[test]
fn rejects_container_and_package_elements_hidden_in_wrappers() -> anyhow::Result<()> {
    let wrapped_container = epub(&[
        FixtureEntry { path: "META-INF/container.xml", bytes: br#"<wrapper xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><container><rootfiles><rootfile full-path="book.opf"/></rootfiles></container></wrapper>"#, compression: Stored },
        FixtureEntry { path: "book.opf", bytes: minimal_opf(), compression: Stored },
        FixtureEntry { path: "chapter.xhtml", bytes: b"<html xmlns=\"http://www.w3.org/1999/xhtml\"><body>ok</body></html>", compression: Stored },
    ])?;
    assert_eq!(
        inspect(&wrapped_container, ParserLimits::default())?,
        EpubErrorCode::InvalidContainer
    );

    let wrapped_package = epub(&[
        FixtureEntry { path: "META-INF/container.xml", bytes: standard_container(), compression: Stored },
        FixtureEntry { path: "book.opf", bytes: br#"<wrapper xmlns="http://www.idpf.org/2007/opf"><package version="3.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Wrapped</dc:title></metadata><manifest><item id="chapter" href="chapter.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="chapter"/></spine></package></wrapper>"#, compression: Stored },
        FixtureEntry { path: "chapter.xhtml", bytes: b"<html xmlns=\"http://www.w3.org/1999/xhtml\"><body>ok</body></html>", compression: Stored },
    ])?;
    assert_eq!(
        inspect(&wrapped_package, ParserLimits::default())?,
        EpubErrorCode::InvalidPackage
    );
    Ok(())
}

#[test]
fn requires_valid_nonempty_navigation_with_existing_internal_targets() -> anyhow::Result<()> {
    for nav in [
        br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>no toc</p></body></html>"#.as_slice(),
        br#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><body><nav epub:type="toc"><ol/></nav></body></html>"#.as_slice(),
        br#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><body><nav epub:type="toc"><a href="missing.xhtml#x">Missing</a></nav></body></html>"#.as_slice(),
    ] {
        let source = epub(&[
            FixtureEntry { path: "META-INF/container.xml", bytes: standard_container(), compression: Stored },
            FixtureEntry { path: "book.opf", bytes: nav_opf(), compression: Stored },
            FixtureEntry { path: "chapter.xhtml", bytes: b"<html xmlns=\"http://www.w3.org/1999/xhtml\"><body>ok</body></html>", compression: Stored },
            FixtureEntry { path: "nav.xhtml", bytes: nav, compression: Stored },
        ])?;
        assert_eq!(
            inspect(&source, ParserLimits::default())?,
            EpubErrorCode::InvalidNavigation
        );
    }
    Ok(())
}

#[test]
fn requires_exactly_one_readable_navigation_document() -> anyhow::Result<()> {
    let missing = epub(&[
        FixtureEntry {
            path: "META-INF/container.xml",
            bytes: standard_container(),
            compression: Stored,
        },
        FixtureEntry {
            path: "book.opf",
            bytes: minimal_opf(),
            compression: Stored,
        },
        FixtureEntry {
            path: "chapter.xhtml",
            bytes: b"<html xmlns=\"http://www.w3.org/1999/xhtml\"><body>ok</body></html>",
            compression: Stored,
        },
    ])?;
    assert_eq!(
        inspect(&missing, ParserLimits::default())?,
        EpubErrorCode::InvalidNavigation
    );

    let multiple = epub(&[
        FixtureEntry { path: "META-INF/container.xml", bytes: standard_container(), compression: Stored },
        FixtureEntry { path: "book.opf", bytes: br#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Two navs</dc:title></metadata><manifest><item id="chapter" href="chapter.xhtml" media-type="application/xhtml+xml"/><item id="nav-one" href="nav-one.xhtml" media-type="application/xhtml+xml" properties="nav"/><item id="nav-two" href="nav-two.xhtml" media-type="application/xhtml+xml" properties="nav"/></manifest><spine><itemref idref="chapter"/></spine></package>"#, compression: Stored },
        FixtureEntry { path: "chapter.xhtml", bytes: b"<html xmlns=\"http://www.w3.org/1999/xhtml\"><body>ok</body></html>", compression: Stored },
        FixtureEntry { path: "nav-one.xhtml", bytes: valid_nav(), compression: Stored },
        FixtureEntry { path: "nav-two.xhtml", bytes: valid_nav(), compression: Stored },
    ])?;
    assert_eq!(
        inspect(&multiple, ParserLimits::default())?,
        EpubErrorCode::InvalidNavigation
    );

    let unreadable = epub(&[
        FixtureEntry { path: "META-INF/container.xml", bytes: standard_container(), compression: Stored },
        FixtureEntry { path: "book.opf", bytes: br#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Unreadable nav</dc:title></metadata><manifest><item id="chapter" href="chapter.xhtml" media-type="application/xhtml+xml"/><item id="nav" href="nav.xhtml" media-type="text/html" properties="nav"/></manifest><spine><itemref idref="chapter"/></spine></package>"#, compression: Stored },
        FixtureEntry { path: "chapter.xhtml", bytes: b"<html xmlns=\"http://www.w3.org/1999/xhtml\"><body>ok</body></html>", compression: Stored },
        FixtureEntry { path: "nav.xhtml", bytes: valid_nav(), compression: Stored },
    ])?;
    assert_eq!(
        inspect(&unreadable, ParserLimits::default())?,
        EpubErrorCode::InvalidNavigation
    );
    Ok(())
}

#[test]
fn rejects_invalid_epub_two_ncx_navigation() -> anyhow::Result<()> {
    for ncx in [
        br#"<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/"><navMap><navPoint><navLabel><text>Outside</text></navLabel><content src="missing.xhtml"/></navPoint></navMap></ncx>"#.as_slice(),
        br#"<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/"><navMap><navPoint><navLabel><text>   </text></navLabel><content src="chapter.xhtml"/></navPoint></navMap></ncx>"#.as_slice(),
        br#"<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/"><head/></ncx>"#.as_slice(),
    ] {
        let source = epub(&[
            FixtureEntry { path: "META-INF/container.xml", bytes: standard_container(), compression: Stored },
            FixtureEntry { path: "book.opf", bytes: epub_two_ncx_opf(), compression: Stored },
            FixtureEntry { path: "chapter.xhtml", bytes: b"<html/>", compression: Stored },
            FixtureEntry { path: "toc.ncx", bytes: ncx, compression: Stored },
        ])?;
        assert_eq!(
            inspect(&source, ParserLimits::default())?,
            EpubErrorCode::InvalidNavigation
        );
    }
    Ok(())
}

#[test]
fn spine_requires_readable_content_or_a_readable_fallback() -> anyhow::Result<()> {
    let image_only = epub(&[
        FixtureEntry { path: "META-INF/container.xml", bytes: standard_container(), compression: Stored },
        FixtureEntry { path: "book.opf", bytes: br#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Image spine</dc:title></metadata><manifest><item id="image" href="cover.png" media-type="image/png"/></manifest><spine><itemref idref="image"/></spine></package>"#, compression: Stored },
        FixtureEntry { path: "cover.png", bytes: b"png", compression: Stored },
    ])?;
    assert_eq!(
        inspect(&image_only, ParserLimits::default())?,
        EpubErrorCode::InvalidSpine
    );

    let fallback = epub(&[
        FixtureEntry { path: "META-INF/container.xml", bytes: standard_container(), compression: Stored },
        FixtureEntry { path: "book.opf", bytes: br#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Fallback spine</dc:title></metadata><manifest><item id="image" href="cover.png" media-type="image/png" fallback="chapter"/><item id="chapter" href="chapter.xhtml" media-type="application/xhtml+xml"/><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/></manifest><spine><itemref idref="image"/></spine></package>"#, compression: Stored },
        FixtureEntry { path: "cover.png", bytes: b"png", compression: Stored },
        FixtureEntry { path: "chapter.xhtml", bytes: b"<html xmlns=\"http://www.w3.org/1999/xhtml\"><body>ok</body></html>", compression: Stored },
        FixtureEntry { path: "nav.xhtml", bytes: valid_nav(), compression: Stored },
    ])?;
    let publication = EpubParser::inspect(&mut Cursor::new(fallback), ParserLimits::default())?;
    assert_eq!(publication.spine[0].href.as_str(), "chapter.xhtml");
    Ok(())
}

struct CountingReader {
    inner: Cursor<Vec<u8>>,
    bytes_read: usize,
    seek_calls: usize,
}

impl CountingReader {
    fn new(bytes: Vec<u8>) -> Self {
        Self {
            inner: Cursor::new(bytes),
            bytes_read: 0,
            seek_calls: 0,
        }
    }
}

impl Read for CountingReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buffer)?;
        self.bytes_read = self.bytes_read.saturating_add(read);
        Ok(read)
    }
}

impl Seek for CountingReader {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.seek_calls = self.seek_calls.saturating_add(1);
        self.inner.seek(position)
    }
}

fn raw_eocd(
    entries: u16,
    disk: u16,
    central_disk: u16,
    central_size: u32,
    central_offset: u32,
) -> Vec<u8> {
    let mut bytes = Vec::from(b"PK\x05\x06".as_slice());
    bytes.extend_from_slice(&disk.to_le_bytes());
    bytes.extend_from_slice(&central_disk.to_le_bytes());
    bytes.extend_from_slice(&entries.to_le_bytes());
    bytes.extend_from_slice(&entries.to_le_bytes());
    bytes.extend_from_slice(&central_size.to_le_bytes());
    bytes.extend_from_slice(&central_offset.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes
}

fn standard_container() -> &'static [u8] {
    br#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="book.opf"/></rootfiles></container>"#
}

fn minimal_opf() -> &'static [u8] {
    br#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Book</dc:title></metadata><manifest><item id="chapter" href="chapter.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="chapter"/></spine></package>"#
}

fn nav_opf() -> &'static [u8] {
    br#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Book</dc:title></metadata><manifest><item id="chapter" href="chapter.xhtml" media-type="application/xhtml+xml"/><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/></manifest><spine><itemref idref="chapter"/></spine></package>"#
}

fn epub_two_ncx_opf() -> &'static [u8] {
    br#"<package xmlns="http://www.idpf.org/2007/opf" version="2.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Book</dc:title></metadata><manifest><item id="chapter" href="chapter.xhtml" media-type="application/xhtml+xml"/><item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/></manifest><spine toc="ncx"><itemref idref="chapter"/></spine></package>"#
}

fn valid_nav() -> &'static [u8] {
    br#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><body><nav epub:type="toc"><a href="chapter.xhtml#start">Chapter</a></nav></body></html>"#
}
