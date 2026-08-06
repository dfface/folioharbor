#![allow(clippy::expect_used)]

use std::{
    io::{Cursor, Write},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use folioharbor_application::ports::{
    BlobStore, BlobStoreError, PromotedBlob, PublicationResourceReader, ReaderResource,
    ResourceReadRequest,
};
use folioharbor_domain::{
    id::{BlobId, PublicationPackageId},
    imports::blob::{BlobIdentity, StorageKey},
};
use folioharbor_epub::{EpubResourceReader, ResourceCacheLimits};
use zip::{ZipWriter, write::SimpleFileOptions};

struct Blobs {
    archive: Vec<u8>,
    opens: AtomicUsize,
}

#[async_trait]
impl BlobStore for Blobs {
    fn candidate_key(&self, _: &BlobIdentity) -> StorageKey {
        StorageKey::from_opaque("unused".to_owned())
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
        self.opens.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(Cursor::new(self.archive.clone())))
    }
}

fn archive() -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default();
    writer
        .start_file("OPS/chapter.xhtml", options)
        .expect("fixture entry");
    writer.write_all(br#"<html><head><meta http-equiv="refresh" content="0;url=https://evil.test"></head><body onload="steal()"><script>steal()</script><form>x</form><iframe src="x"></iframe><object>x</object><p style="background:url(https://evil.test/x)">safe</p><img src="cover.png"></body></html>"#).expect("fixture html");
    writer
        .start_file("OPS/book.css", options)
        .expect("fixture entry");
    writer
        .write_all(
            b"@import 'https://evil.test/a'; p{color:red;background:url(https://evil.test/x)}",
        )
        .expect("fixture css");
    writer
        .start_file("OPS/cover.png", options)
        .expect("fixture entry");
    writer.write_all(b"PNG").expect("fixture image");
    writer.finish().expect("fixture archive").into_inner()
}

fn request(href: &str, media_type: &str) -> ResourceReadRequest {
    ResourceReadRequest {
        blob_id: BlobId::from_uuid(uuid::Uuid::from_u128(5)),
        storage_key: StorageKey::from_opaque("blob:instance-v1:digest:42".to_owned()),
        package_id: PublicationPackageId::from_uuid(uuid::Uuid::from_u128(4)),
        normalized_href: href.to_owned(),
        media_type: media_type.to_owned(),
        resources: vec![
            ReaderResource {
                normalized_href: "OPS/chapter.xhtml".to_owned(),
                media_type: "application/xhtml+xml".to_owned(),
            },
            ReaderResource {
                normalized_href: "OPS/book.css".to_owned(),
                media_type: "text/css".to_owned(),
            },
            ReaderResource {
                normalized_href: "OPS/cover.png".to_owned(),
                media_type: "image/png".to_owned(),
            },
        ],
    }
}

#[tokio::test]
async fn sanitizes_malicious_html_and_uses_bounded_disposable_cache() {
    let blobs = Arc::new(Blobs {
        archive: archive(),
        opens: AtomicUsize::new(0),
    });
    let reader = EpubResourceReader::new(
        blobs.clone(),
        ResourceCacheLimits {
            max_entries: 2,
            max_bytes: 1024 * 1024,
            max_resource_bytes: 1024 * 1024,
        },
    );
    let first = reader
        .read(request("OPS/chapter.xhtml", "application/xhtml+xml"))
        .await
        .expect("safe resource");
    let second = reader
        .read(request("OPS/chapter.xhtml", "application/xhtml+xml"))
        .await
        .expect("cached safe resource");
    let html = String::from_utf8(first.clone())
        .expect("utf8 output")
        .to_ascii_lowercase();
    for forbidden in [
        "<script",
        "<form",
        "<iframe",
        "<object",
        "http-equiv",
        "onload",
        "https://",
    ] {
        assert!(!html.contains(forbidden), "found {forbidden}: {html}");
    }
    assert!(html.contains("safe"));
    assert!(html.contains("resource:"));
    assert_eq!(first, second);
    assert_eq!(blobs.opens.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn sanitizes_external_urls_from_standalone_css() {
    let blobs = Arc::new(Blobs {
        archive: archive(),
        opens: AtomicUsize::new(0),
    });
    let reader = EpubResourceReader::new(blobs, ResourceCacheLimits::default());
    let bytes = reader
        .read(request("OPS/book.css", "text/css"))
        .await
        .expect("safe css");
    let css = String::from_utf8(bytes)
        .expect("utf8 output")
        .to_ascii_lowercase();
    assert!(!css.contains("@import"));
    assert!(!css.contains("https://"));
    assert!(css.contains("color:red"));
}

#[tokio::test]
async fn rejects_disallowed_types_and_decompressed_resources_over_limit() {
    let blobs = Arc::new(Blobs {
        archive: archive(),
        opens: AtomicUsize::new(0),
    });
    let reader = EpubResourceReader::new(
        blobs,
        ResourceCacheLimits {
            max_entries: 2,
            max_bytes: 1024,
            max_resource_bytes: 2,
        },
    );
    assert!(
        reader
            .read(request("OPS/cover.png", "image/svg+xml"))
            .await
            .is_err()
    );
    assert!(
        reader
            .read(request("OPS/cover.png", "image/png"))
            .await
            .is_err()
    );
}
