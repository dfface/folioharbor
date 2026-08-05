use std::io::{Cursor, Write};

use zip::write::SimpleFileOptions;

pub struct FixtureEntry<'a> {
    pub path: &'a str,
    pub bytes: &'a [u8],
    pub compression: zip::CompressionMethod,
}

pub fn epub(entries: &[FixtureEntry<'_>]) -> anyhow::Result<Vec<u8>> {
    let cursor = Cursor::new(Vec::new());
    let mut archive = zip::ZipWriter::new(cursor);
    for entry in entries {
        let options = SimpleFileOptions::default()
            .compression_method(entry.compression)
            .last_modified_time(zip::DateTime::default());
        archive.start_file(entry.path, options)?;
        archive.write_all(entry.bytes)?;
    }
    Ok(archive.finish()?.into_inner())
}

pub fn valid_epub() -> anyhow::Result<Vec<u8>> {
    epub(&[
        FixtureEntry { path: "mimetype", bytes: b"application/epub+zip", compression: zip::CompressionMethod::Stored },
        FixtureEntry { path: "META-INF/container.xml", bytes: br#"<?xml version="1.0"?><container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="EPUB/book.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#, compression: zip::CompressionMethod::Deflated },
        FixtureEntry { path: "EPUB/book.opf", bytes: br#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Small Book</dc:title><dc:creator>Ada Author</dc:creator><dc:language>en</dc:language><dc:identifier>urn:fixture:1</dc:identifier><dc:publisher>Fixture Press</dc:publisher><meta property="fixture:unknown">kept-as-warning</meta></metadata><manifest><item id="chapter" href="text/chapter.xhtml" media-type="application/xhtml+xml"/><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/><item id="css" href="styles/book.css" media-type="text/css"/><item id="cover" href="images/cover.png" media-type="image/png" properties="cover-image"/></manifest><spine><itemref idref="chapter"/></spine></package>"#, compression: zip::CompressionMethod::Deflated },
        FixtureEntry { path: "EPUB/text/chapter.xhtml", bytes: br#"<html xmlns="http://www.w3.org/1999/xhtml"><head><link rel="stylesheet" href="../styles/book.css"/></head><body><h1>Chapter</h1><img src="../images/cover.png"/></body></html>"#, compression: zip::CompressionMethod::Deflated },
        FixtureEntry { path: "EPUB/nav.xhtml", bytes: br#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><body><nav epub:type="toc"><ol><li><a href="text/chapter.xhtml#start">Chapter</a></li></ol></nav></body></html>"#, compression: zip::CompressionMethod::Deflated },
        FixtureEntry { path: "EPUB/styles/book.css", bytes: b"body { writing-mode: vertical-rl; color: #222; }", compression: zip::CompressionMethod::Deflated },
        FixtureEntry { path: "EPUB/images/cover.png", bytes: b"fixture-image", compression: zip::CompressionMethod::Deflated },
    ])
}
