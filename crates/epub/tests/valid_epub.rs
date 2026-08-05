#[path = "../../../tests/fixtures/epub/generate-fixtures.rs"]
mod fixtures;

use std::io::Cursor;

use folioharbor_epub::{EpubParser, EpubPath, ParserLimits};

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

fn sha256(input: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(input).into()
}
