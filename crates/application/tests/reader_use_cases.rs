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
    publication: Arc<ReaderPublication>,
    revoke_after: usize,
}

#[async_trait]
impl ReaderCatalogRepository for Catalog {
    async fn find_readable_publication(
        &self,
        _: UserId,
        _: ItemId,
        _: RequestId,
    ) -> Result<Option<Arc<ReaderPublication>>, ReaderCatalogError> {
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

fn publication() -> Arc<ReaderPublication> {
    Arc::new(ReaderPublication::new(
        LibraryId::from_uuid(uuid::Uuid::from_u128(1)),
        ItemId::from_uuid(uuid::Uuid::from_u128(2)),
        ManifestationId::from_uuid(uuid::Uuid::from_u128(3)),
        PublicationPackageId::from_uuid(uuid::Uuid::from_u128(4)),
        BlobId::from_uuid(uuid::Uuid::from_u128(5)),
        StorageKey::from_opaque("blob:instance-v1:digest:42".to_owned()),
        "epub3-v1".to_owned(),
        "安全阅读".to_owned(),
        vec!["作者".to_owned()],
        vec!["zh-CN".to_owned()],
        vec![
            ReaderResource {
                normalized_href: "OPS/chapter.xhtml".to_owned(),
                media_type: "application/xhtml+xml".to_owned(),
            },
            ReaderResource {
                normalized_href: "OPS/cover.png".to_owned(),
                media_type: "image/png".to_owned(),
            },
        ],
        vec![ReaderSpineEntry {
            normalized_href: "OPS/chapter.xhtml".to_owned(),
            linear: true,
        }],
        vec![ReaderTocEntry {
            label: "第一章".to_owned(),
            normalized_href: "OPS/chapter.xhtml#start".to_owned(),
        }],
    ))
}

fn publication_with_unsafe_fragment() -> Arc<ReaderPublication> {
    let mut value = (*publication()).clone();
    "OPS/chapter.xhtml#part 1/章节?#next"
        .clone_into(&mut Arc::make_mut(&mut value.toc)[0].normalized_href);
    Arc::new(value)
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
async fn manifest_percent_encodes_untrusted_toc_fragments() {
    let catalog = Catalog {
        calls: AtomicUsize::new(0),
        publication: publication_with_unsafe_fragment(),
        revoke_after: 1,
    };
    let manifest = GetPublicationManifest::new(&catalog)
        .execute(UserId::new(), publication().item_id, RequestId::new())
        .await
        .unwrap_or_else(|_| std::process::abort());
    assert!(
        manifest.toc[0]
            .href
            .ends_with("#part%201%2F%E7%AB%A0%E8%8A%82%3F%23next")
    );
}

#[tokio::test]
async fn manifest_projects_the_4096_resource_boundary_in_one_indexed_pass() {
    let mut value = (*publication()).clone();
    value.resources = (0..4096)
        .map(|index| ReaderResource {
            normalized_href: format!("OPS/resource-{index}.xhtml"),
            media_type: "application/xhtml+xml".to_owned(),
        })
        .collect::<Vec<_>>()
        .into();
    value.reading_order = (0..2048)
        .map(|index| ReaderSpineEntry {
            normalized_href: format!("OPS/resource-{index}.xhtml"),
            linear: true,
        })
        .collect::<Vec<_>>()
        .into();
    value.toc = vec![ReaderTocEntry {
        label: "Last".to_owned(),
        normalized_href: "OPS/resource-4095.xhtml#end".to_owned(),
    }]
    .into();
    let catalog = Catalog {
        calls: AtomicUsize::new(0),
        publication: Arc::new(value),
        revoke_after: 1,
    };
    let manifest = GetPublicationManifest::new(&catalog)
        .execute(UserId::new(), publication().item_id, RequestId::new())
        .await
        .unwrap_or_else(|_| std::process::abort());
    assert_eq!(manifest.reading_order.len(), 2048);
    assert_eq!(manifest.resources.len(), 2048);
    assert_eq!(manifest.toc.len(), 1);
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
    assert!(
        first
            .etag
            .contains(&publication().item_id.as_uuid().to_string()),
        "resource bytes embed the Item-scoped HTTP rewrite base, so the strong ETag must be Item-scoped"
    );

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
