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
