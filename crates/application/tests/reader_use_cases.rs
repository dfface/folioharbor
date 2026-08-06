use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use folioharbor_application::{
    error::AppError,
    ports::{
        PublicationResourceReader, ReaderCatalogError, ReaderCatalogRepository, ReaderPublication,
        ReaderResource, ReaderSpineEntry, ReaderTocEntry, ResourceReadRequest, ResourceReaderError,
    },
    reader::{GetPublicationManifest, GetPublicationResource, ResourceId},
};
use folioharbor_domain::{
    id::{BlobId, ItemId, LibraryId, ManifestationId, PublicationPackageId, RequestId, UserId},
    imports::blob::StorageKey,
};

struct Catalog {
    calls: AtomicUsize,
    publication: ReaderPublication,
    revoke_after: usize,
}

#[async_trait]
impl ReaderCatalogRepository for Catalog {
    async fn find_readable_publication(
        &self,
        _: UserId,
        _: ItemId,
        _: RequestId,
    ) -> Result<Option<ReaderPublication>, ReaderCatalogError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        Ok((call <= self.revoke_after).then(|| self.publication.clone()))
    }
}

struct Reader;

#[async_trait]
impl PublicationResourceReader for Reader {
    async fn read(&self, request: ResourceReadRequest) -> Result<Vec<u8>, ResourceReaderError> {
        assert_eq!(request.normalized_href, "OPS/chapter.xhtml");
        Ok(b"<p>safe</p>".to_vec())
    }
}

fn publication() -> ReaderPublication {
    ReaderPublication {
        library_id: LibraryId::from_uuid(uuid::Uuid::from_u128(1)),
        item_id: ItemId::from_uuid(uuid::Uuid::from_u128(2)),
        manifestation_id: ManifestationId::from_uuid(uuid::Uuid::from_u128(3)),
        package_id: PublicationPackageId::from_uuid(uuid::Uuid::from_u128(4)),
        blob_id: BlobId::from_uuid(uuid::Uuid::from_u128(5)),
        storage_key: StorageKey::from_opaque("blob:instance-v1:digest:42".to_owned()),
        parser_profile_version: "epub3-v1".to_owned(),
        primary_title: "安全阅读".to_owned(),
        authors: vec!["作者".to_owned()],
        languages: vec!["zh-CN".to_owned()],
        resources: vec![
            ReaderResource {
                normalized_href: "OPS/chapter.xhtml".to_owned(),
                media_type: "application/xhtml+xml".to_owned(),
            },
            ReaderResource {
                normalized_href: "OPS/cover.png".to_owned(),
                media_type: "image/png".to_owned(),
            },
        ],
        reading_order: vec![ReaderSpineEntry {
            normalized_href: "OPS/chapter.xhtml".to_owned(),
            linear: true,
        }],
        toc: vec![ReaderTocEntry {
            label: "第一章".to_owned(),
            normalized_href: "OPS/chapter.xhtml#start".to_owned(),
        }],
    }
}

#[tokio::test]
async fn manifest_uses_stable_opaque_links_without_archive_paths() {
    let catalog = Catalog {
        calls: AtomicUsize::new(0),
        publication: publication(),
        revoke_after: 2,
    };
    let first = GetPublicationManifest::new(&catalog)
        .execute(UserId::new(), publication().item_id, RequestId::new())
        .await
        .unwrap_or_else(|_| std::process::abort());
    let second = GetPublicationManifest::new(&catalog)
        .execute(UserId::new(), publication().item_id, RequestId::new())
        .await
        .unwrap_or_else(|_| std::process::abort());

    assert_eq!(first, second);
    assert_eq!(first.metadata.title, "安全阅读");
    assert_eq!(first.metadata.authors, ["作者"]);
    assert_eq!(first.metadata.languages, ["zh-CN"]);
    assert_eq!(first.manifestation_id, publication().manifestation_id);
    assert_eq!(first.reading_order.len(), 1);
    assert_eq!(first.resources.len(), 1);
    assert_eq!(first.toc[0].title.as_deref(), Some("第一章"));
    assert!(first.toc[0].href.ends_with("#start"));
    assert_eq!(first.links[0].relation, "self");
    assert!(
        first.reading_order[0]
            .href
            .contains("/api/v1/items/00000000-0000-0000-0000-000000000002/resources/")
    );
    assert!(!first.reading_order[0].href.contains("OPS/"));
    assert!(first.etag.contains("epub3-v1"));
}

#[tokio::test]
async fn resource_reauthorizes_every_request_and_revocation_beats_reader_cache() {
    let catalog = Catalog {
        calls: AtomicUsize::new(0),
        publication: publication(),
        revoke_after: 1,
    };
    let reader = Reader;
    let service = GetPublicationResource::new(&catalog, &reader);
    let id = ResourceId::for_resource(publication().package_id, "OPS/chapter.xhtml");
    let first = service
        .execute(
            UserId::new(),
            publication().item_id,
            id.clone(),
            RequestId::new(),
        )
        .await
        .unwrap_or_else(|_| std::process::abort());
    assert_eq!(first.bytes, b"<p>safe</p>");

    let denied = service
        .execute(UserId::new(), publication().item_id, id, RequestId::new())
        .await;
    assert!(matches!(
        denied,
        Err(AppError::NotFound {
            code: "item_not_found"
        })
    ));
    assert_eq!(catalog.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn unknown_resource_is_anti_enumerating_not_found_without_reading_blob() {
    struct MustNotRead;
    #[async_trait]
    impl PublicationResourceReader for MustNotRead {
        async fn read(&self, _: ResourceReadRequest) -> Result<Vec<u8>, ResourceReaderError> {
            std::process::abort()
        }
    }
    let catalog = Arc::new(Catalog {
        calls: AtomicUsize::new(0),
        publication: publication(),
        revoke_after: 1,
    });
    let result = GetPublicationResource::new(catalog.as_ref(), &MustNotRead)
        .execute(
            UserId::new(),
            publication().item_id,
            ResourceId::parse("missing").unwrap_or_else(|_| std::process::abort()),
            RequestId::new(),
        )
        .await;
    assert!(matches!(
        result,
        Err(AppError::NotFound {
            code: "resource_not_found"
        })
    ));
}
