#[path = "../../../tests/fixtures/epub/generate-fixtures.rs"]
mod fixtures;

use std::{io::Cursor, time::Duration};

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
