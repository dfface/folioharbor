#![allow(clippy::expect_used)]

use std::{
    io::{Cursor, Write},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use folioharbor_application::ports::{
    BlobStore, BlobStoreError, PromotedBlob, PublicationResourceReader, ResourceReadRequest,
};
use folioharbor_domain::{
    id::{BlobId, ItemId, PublicationPackageId},
    imports::blob::{BlobIdentity, StorageKey},
};
use folioharbor_epub::{BlockingWorkHook, EpubResourceReader, ResourceCacheLimits};
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
    writer.write_all(br#"<html><head><meta http-equiv="refresh" content="0;url=https://evil.test"><link rel="stylesheet" href="book.css"></head><body onload="steal()"><script>steal()</script><form>x</form><iframe src="x"></iframe><object>x</object><p style="background:url(https://evil.test/x)">safe</p><img src="cover.png"></body></html>"#).expect("fixture html");
    writer
        .start_file("OPS/book.css", options)
        .expect("fixture entry");
    writer
        .write_all(b"@import 'https://evil.test/a'; p{color:red;background-image:url(cover.png)}")
        .expect("fixture css");
    writer
        .start_file("OPS/cover.png", options)
        .expect("fixture entry");
    writer.write_all(b"PNG").expect("fixture image");
    writer.finish().expect("fixture archive").into_inner()
}

fn request(href: &str, media_type: &str) -> ResourceReadRequest {
    ResourceReadRequest {
        item_id: ItemId::from_uuid(uuid::Uuid::from_u128(3)),
        blob_id: BlobId::from_uuid(uuid::Uuid::from_u128(5)),
        storage_key: StorageKey::from_opaque("blob:instance-v1:digest:42".to_owned()),
        package_id: PublicationPackageId::from_uuid(uuid::Uuid::from_u128(4)),
        normalized_href: href.to_owned(),
        media_type: media_type.to_owned(),
        resource_routes: Arc::new(
            ["OPS/chapter.xhtml", "OPS/book.css", "OPS/cover.png"]
                .into_iter()
                .map(|href| {
                    (
                        href.to_owned(),
                        folioharbor_application::reader::ResourceId::for_resource(
                            PublicationPackageId::from_uuid(uuid::Uuid::from_u128(4)),
                            href,
                        )
                        .as_str()
                        .to_owned(),
                    )
                })
                .collect(),
        ),
    }
}

fn request_for_item(item: u128, href: &str, media_type: &str) -> ResourceReadRequest {
    let mut request = request(href, media_type);
    request.item_id = ItemId::from_uuid(uuid::Uuid::from_u128(item));
    request
}

struct BlockingGate {
    started: tokio::sync::mpsc::UnboundedSender<()>,
    released: (Mutex<bool>, Condvar),
    active: AtomicUsize,
    max_active: AtomicUsize,
}

impl BlockingGate {
    fn release(&self) {
        *self.released.0.lock().expect("release lock") = true;
        self.released.1.notify_all();
    }
}

impl BlockingWorkHook for BlockingGate {
    fn before(&self) {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(active, Ordering::SeqCst);
        self.started.send(()).expect("test receiver");
        let mut released = self.released.0.lock().expect("release lock");
        while !*released {
            released = self.released.1.wait(released).expect("release wait");
        }
    }

    fn after(&self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}

fn blocking_gate() -> (Arc<BlockingGate>, tokio::sync::mpsc::UnboundedReceiver<()>) {
    let (started, receiver) = tokio::sync::mpsc::unbounded_channel();
    (
        Arc::new(BlockingGate {
            started,
            released: (Mutex::new(false), Condvar::new()),
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
        }),
        receiver,
    )
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
            max_concurrent_blocking: 2,
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
    let item = uuid::Uuid::from_u128(3);
    let package = PublicationPackageId::from_uuid(uuid::Uuid::from_u128(4));
    let image_id =
        folioharbor_application::reader::ResourceId::for_resource(package, "OPS/cover.png");
    let css_id = folioharbor_application::reader::ResourceId::for_resource(package, "OPS/book.css");
    assert!(
        html.contains(&format!(
            "/api/v1/items/{item}/resources/{}",
            image_id.as_str().to_ascii_lowercase()
        )),
        "{html}"
    );
    assert!(html.contains(&format!(
        "/api/v1/items/{item}/resources/{}",
        css_id.as_str().to_ascii_lowercase()
    )));
    assert!(!html.contains("resource:"));
    assert!(!html.contains("OPS/"));
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
    let image_id = folioharbor_application::reader::ResourceId::for_resource(
        PublicationPackageId::from_uuid(uuid::Uuid::from_u128(4)),
        "OPS/cover.png",
    );
    assert!(
        css.contains(&format!(
            "/api/v1/items/{}/resources/{}",
            uuid::Uuid::from_u128(3),
            image_id.as_str().to_ascii_lowercase()
        )),
        "{css}"
    );
    assert!(!css.contains("OPS/"));
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
            max_concurrent_blocking: 2,
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

#[tokio::test(flavor = "current_thread")]
async fn cold_work_is_offloaded_bounded_and_keeps_async_executor_responsive() {
    let blobs = Arc::new(Blobs {
        archive: archive(),
        opens: AtomicUsize::new(0),
    });
    let (gate, mut started) = blocking_gate();
    let reader = Arc::new(EpubResourceReader::new_with_hook(
        blobs,
        ResourceCacheLimits {
            max_entries: 4,
            max_bytes: 1024 * 1024,
            max_resource_bytes: 1024 * 1024,
            max_concurrent_blocking: 2,
        },
        gate.clone(),
    ));
    let tasks = [
        ("OPS/chapter.xhtml", "application/xhtml+xml"),
        ("OPS/book.css", "text/css"),
        ("OPS/cover.png", "image/png"),
    ]
    .into_iter()
    .map(|(href, media)| {
        let reader = reader.clone();
        tokio::spawn(async move { reader.read(request(href, media)).await })
    })
    .collect::<Vec<_>>();
    started.recv().await.expect("first blocking task");
    started.recv().await.expect("second blocking task");
    let (heartbeat, heartbeat_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        tokio::task::yield_now().await;
        heartbeat.send(()).expect("heartbeat receiver");
    });
    heartbeat_rx.await.expect("executor heartbeat");
    assert_eq!(gate.max_active.load(Ordering::SeqCst), 2);
    gate.release();
    for task in tasks {
        task.await.expect("reader task").expect("resource");
    }
    assert!(gate.max_active.load(Ordering::SeqCst) <= 2);
}

#[tokio::test(flavor = "current_thread")]
async fn concurrent_same_key_misses_are_single_flight_with_exact_cache_accounting() {
    let blobs = Arc::new(Blobs {
        archive: archive(),
        opens: AtomicUsize::new(0),
    });
    let (gate, mut started) = blocking_gate();
    let reader = Arc::new(EpubResourceReader::new_with_hook(
        blobs.clone(),
        ResourceCacheLimits {
            max_entries: 2,
            max_bytes: 1024 * 1024,
            max_resource_bytes: 1024 * 1024,
            max_concurrent_blocking: 2,
        },
        gate.clone(),
    ));
    let barrier = Arc::new(tokio::sync::Barrier::new(9));
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let reader = reader.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            reader
                .read(request("OPS/chapter.xhtml", "application/xhtml+xml"))
                .await
        }));
    }
    barrier.wait().await;
    started.recv().await.expect("single blocking task");
    gate.release();
    let mut expected_bytes = 0;
    for task in tasks {
        let bytes = task.await.expect("reader task").expect("resource");
        expected_bytes = bytes.len();
    }
    let metrics = reader.cache_metrics().expect("cache metrics");
    assert_eq!(blobs.opens.load(Ordering::SeqCst), 1);
    assert_eq!(metrics.entries, 1);
    assert_eq!(metrics.order_records, 1);
    assert_eq!(metrics.bytes, expected_bytes);

    let css = reader
        .read(request("OPS/book.css", "text/css"))
        .await
        .expect("css");
    let image = reader
        .read(request("OPS/cover.png", "image/png"))
        .await
        .expect("image");
    let metrics = reader.cache_metrics().expect("cache metrics");
    assert_eq!(metrics.entries, 2);
    assert_eq!(metrics.order_records, 2);
    assert_eq!(metrics.bytes, css.len() + image.len());
    reader
        .read(request("OPS/chapter.xhtml", "application/xhtml+xml"))
        .await
        .expect("evicted chapter reloads");
    assert_eq!(blobs.opens.load(Ordering::SeqCst), 4);
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_waiters_do_not_leak_bounded_owned_loaders_or_corrupt_cache_accounting() {
    let blobs = Arc::new(Blobs {
        archive: archive(),
        opens: AtomicUsize::new(0),
    });
    let (gate, mut started) = blocking_gate();
    let reader = Arc::new(EpubResourceReader::new_with_hook(
        blobs.clone(),
        ResourceCacheLimits {
            max_entries: 8,
            max_bytes: 1024 * 1024,
            max_resource_bytes: 1024 * 1024,
            max_concurrent_blocking: 2,
        },
        gate.clone(),
    ));
    let mut tasks = Vec::new();
    for item in 10..18 {
        let reader = reader.clone();
        tasks.push(tokio::spawn(async move {
            reader
                .read(request_for_item(
                    item,
                    "OPS/chapter.xhtml",
                    "application/xhtml+xml",
                ))
                .await
        }));
    }
    started.recv().await.expect("first owned loader");
    started.recv().await.expect("second owned loader");
    for task in &tasks {
        task.abort();
    }
    for task in tasks {
        assert!(task.await.expect_err("waiter is cancelled").is_cancelled());
    }
    gate.release();
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if reader
                .cache_metrics()
                .expect("cache metrics")
                .inflight_reads
                == 0
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("owned loaders finish and clean their state");
    let bytes = reader
        .read(request_for_item(
            10,
            "OPS/chapter.xhtml",
            "application/xhtml+xml",
        ))
        .await
        .expect("completed loader populated cache");
    let metrics = reader.cache_metrics().expect("cache metrics");
    assert_eq!(metrics.inflight_reads, 0);
    assert_eq!(metrics.entries, 2);
    assert_eq!(metrics.order_records, 2);
    assert_eq!(metrics.bytes, bytes.len() * 2);
    assert_eq!(blobs.opens.load(Ordering::SeqCst), 2);
}
