#![allow(clippy::expect_used)]

use async_trait::async_trait;
use folioharbor_application::{
    catalog::{ImportCatalogCommand, ImportCatalogResult, ImportPublicationCatalog},
    ports::{CatalogRepository, CatalogRepositoryError, FinalizeCatalog},
};
use folioharbor_domain::{
    catalog::{
        CatalogMetadata, CatalogPublication, ParserMetadata, PublicationResource, SpineEntry,
        TocEntry,
    },
    id::{BlobId, ItemId, LibraryId, PublicationPackageId, RequestId, UploadId, UserId},
    imports::blob::ByteCount,
    time::OffsetDateTime,
};
use std::sync::Mutex;

#[derive(Default)]
struct RecordingCatalog {
    calls: Mutex<Vec<FinalizeCatalog>>,
}

#[async_trait]
impl CatalogRepository for RecordingCatalog {
    async fn finalize(
        &self,
        command: FinalizeCatalog,
    ) -> Result<ImportCatalogResult, CatalogRepositoryError> {
        self.calls.lock().expect("recording lock").push(command);
        Ok(ImportCatalogResult::Created {
            item_id: ItemId::new(),
            package_id: PublicationPackageId::new(),
        })
    }
}

#[tokio::test]
async fn parser_metadata_is_mapped_without_becoming_an_identity_merge_rule() {
    let repository = RecordingCatalog::default();
    let use_case = ImportPublicationCatalog::new(&repository);
    let first = command(BlobId::new(), "A shared title");
    let second = command(BlobId::new(), "A shared title");

    assert!(matches!(
        use_case.execute(first).await.expect("first import"),
        ImportCatalogResult::Created { .. }
    ));
    assert!(matches!(
        use_case.execute(second).await.expect("second import"),
        ImportCatalogResult::Created { .. }
    ));

    let calls = repository.calls.lock().expect("recording lock");
    assert_eq!(calls.len(), 2);
    assert_ne!(calls[0].original_blob_id, calls[1].original_blob_id);
    assert_eq!(
        calls[0].publication.metadata().primary_title(),
        "A shared title"
    );
    assert_eq!(
        calls[1].publication.metadata().primary_title(),
        "A shared title"
    );
}

fn command(blob_id: BlobId, title: &str) -> ImportCatalogCommand {
    let metadata = CatalogMetadata::from_parser(&ParserMetadata {
        titles: vec![title.to_owned()],
        authors: vec!["Author".to_owned()],
        languages: vec!["en".to_owned()],
        identifiers: vec!["same-looking-id".to_owned()],
    })
    .expect("valid metadata");
    let publication = CatalogPublication::from_parser(
        metadata,
        vec![
            PublicationResource::new("text/chapter.xhtml", "application/xhtml+xml")
                .expect("resource"),
        ],
        vec![SpineEntry::new("text/chapter.xhtml", true).expect("spine")],
        vec![TocEntry::new("Chapter", "text/chapter.xhtml").expect("toc")],
        None,
    )
    .expect("publication");
    ImportCatalogCommand {
        library_id: LibraryId::new(),
        upload_id: UploadId::new(),
        actor_id: UserId::new(),
        original_blob_id: blob_id,
        logical_bytes: ByteCount::new(123),
        parser_profile_version: "epub-v1".to_owned(),
        publication,
        request_id: RequestId::new(),
        now: OffsetDateTime::now_utc(),
    }
}

#[test]
fn catalog_hrefs_reject_transport_and_ambiguous_epub_paths() {
    for invalid in [
        "http://example.test/chapter.xhtml",
        "mailto:reader@example.test",
        "data:text/html,chapter",
        "scheme:value",
        "OPS/chapter\0.xhtml",
        "OPS/chapter\n.xhtml",
        "/OPS/chapter.xhtml",
        "OPS\\chapter.xhtml",
        "OPS/../chapter.xhtml",
        "OPS//chapter.xhtml",
        "OPS/%2e%2e/chapter.xhtml",
        "OPS/%2Fchapter.xhtml",
        "OPS/%5cchapter.xhtml",
        "OPS/%00chapter.xhtml",
        "OPS/%1fchapter.xhtml",
        "OPS/chapter%3Fmode.xhtml",
        "OPS/chapter%23fragment.xhtml",
        "OPS/scheme%3avalue.xhtml",
    ] {
        assert!(
            PublicationResource::new(invalid, "application/xhtml+xml").is_err(),
            "must reject {invalid:?}"
        );
        assert!(SpineEntry::new(invalid, true).is_err());
        assert!(TocEntry::new("Chapter", invalid).is_err());
    }
    for invalid_resource in ["OPS/chapter.xhtml?mode=raw", "OPS/chapter.xhtml#fragment"] {
        assert!(
            PublicationResource::new(invalid_resource, "application/xhtml+xml").is_err(),
            "resource href must be a fragment-free manifest path"
        );
        assert!(SpineEntry::new(invalid_resource, true).is_err());
    }
    for invalid_locator in [
        "OPS/chapter.xhtml#",
        "OPS/chapter.xhtml#section\0",
        "OPS/chapter.xhtml#section%00",
        "OPS/chapter.xhtml#section%3Anested",
        "OPS/chapter.xhtml#section%ZZ",
        "OPS/chapter.xhtml#http://example.test",
        "OPS/../chapter.xhtml#section-1",
        "https://example.test/chapter.xhtml#section-1",
        "OPS/chapter.xhtml#section#nested",
        "OPS/chapter.xhtml%23section-1",
    ] {
        assert!(
            TocEntry::new("Chapter", invalid_locator).is_err(),
            "must reject unsafe TOC locator {invalid_locator:?}"
        );
    }
    let toc =
        TocEntry::new("Section", "OPS/chapter.xhtml#section-1").expect("safe TOC fragment locator");
    assert_eq!(toc.href(), "OPS/chapter.xhtml#section-1");
    assert!(PublicationResource::new("OPS/chapter-1.xhtml", "application/xhtml+xml").is_ok());
}
