#![allow(clippy::expect_used)]

use std::sync::{Arc, Mutex};

use folioharbor_application::ports::{BlobDisposition, BlobStore};
use folioharbor_domain::{
    id::{LibraryId, UploadId},
    imports::blob::{
        BlobIdentity, ByteCount, DedupScope, Sha256Digest, StorageKey, StorageNamespace,
    },
};
use folioharbor_storage_local::{CapacityProbe, LocalBlobStore, MIN_FREE_BYTES};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

#[derive(Clone)]
struct FakeCapacity(Arc<Mutex<u64>>);

impl FakeCapacity {
    fn new(bytes: u64) -> Self {
        Self(Arc::new(Mutex::new(bytes)))
    }

    fn set(&self, bytes: u64) {
        *self.0.lock().expect("capacity lock") = bytes;
    }
}

impl CapacityProbe for FakeCapacity {
    fn free_bytes(&self, _: &std::path::Path) -> std::io::Result<u64> {
        Ok(*self.0.lock().map_err(|_| std::io::ErrorKind::Other)?)
    }
}

fn identity(namespace: StorageNamespace, payload: &[u8]) -> BlobIdentity {
    let digest: [u8; 32] = Sha256::digest(payload).into();
    BlobIdentity::new(
        namespace,
        Sha256Digest::from_bytes(digest),
        ByteCount::new(payload.len() as u64),
    )
}

async fn create_staging(store: &LocalBlobStore<FakeCapacity>, marker: char) -> StorageKey {
    let key = StorageKey::from_opaque(format!("staging:{}", marker.to_string().repeat(64)));
    store
        .create_staging_for(&key)
        .await
        .expect("staging object");
    key
}

#[tokio::test]
async fn supplied_staging_capabilities_are_exclusive_and_traversal_never_escapes_the_root() {
    let root = TempDir::new().expect("temporary root");
    let store = LocalBlobStore::with_capacity(root.path(), FakeCapacity::new(u64::MAX));
    let first = create_staging(&store, '1').await;
    let second = create_staging(&store, '2').await;
    assert_ne!(first, second);
    assert!(store.create_staging_for(&first).await.is_err());

    let traversal = "staging:../../outside".parse().expect("opaque key syntax");
    assert!(store.append(&traversal, b"escape").await.is_err());
    assert!(
        !root
            .path()
            .parent()
            .expect("parent")
            .join("outside")
            .exists()
    );
}

#[tokio::test]
async fn append_is_bounded_ranges_are_exact_and_promotion_preserves_hash() {
    let root = TempDir::new().expect("temporary root");
    let store = LocalBlobStore::with_capacity(root.path(), FakeCapacity::new(u64::MAX));
    let staging = create_staging(&store, '3').await;
    store
        .append(&staging, b"hello ")
        .await
        .expect("first append");
    store
        .append(&staging, b"world")
        .await
        .expect("second append");
    assert_eq!(
        store.read_range(&staging, 6, 5).await.expect("range"),
        b"world"
    );
    assert!(
        store
            .append(&staging, &vec![0; 8 * 1024 * 1024 + 1])
            .await
            .is_err()
    );

    let namespace = StorageNamespace::for_scope(
        DedupScope::Library,
        LibraryId::from_uuid(uuid::Uuid::from_u128(7)),
        UploadId::from_uuid(uuid::Uuid::from_u128(8)),
    );
    let installed = store
        .promote(&staging, &identity(namespace, b"hello world"))
        .await
        .expect("promotion");
    assert_eq!(installed.disposition, BlobDisposition::Installed);
    let final_key = installed.key;
    assert_eq!(
        store
            .read_range(&final_key, 0, 64)
            .await
            .expect("full read"),
        b"hello world"
    );
    let reused = store
        .promote(
            &staging,
            &identity(
                StorageNamespace::for_scope(
                    DedupScope::Library,
                    LibraryId::from_uuid(uuid::Uuid::from_u128(7)),
                    UploadId::from_uuid(uuid::Uuid::from_u128(8)),
                ),
                b"hello world",
            ),
        )
        .await
        .expect("idempotent promotion");
    assert_eq!(reused.key, final_key);
    assert_eq!(reused.disposition, BlobDisposition::Reused);
    store.delete(&final_key).await.expect("delete");
    store.delete(&final_key).await.expect("idempotent delete");
}

#[tokio::test]
async fn capacity_is_checked_before_staging_append_and_promotion() {
    let root = TempDir::new().expect("temporary root");
    let low = FakeCapacity::new(MIN_FREE_BYTES - 1);
    let store = LocalBlobStore::with_capacity(root.path(), low);
    let unavailable = StorageKey::from_opaque(format!("staging:{}", "4".repeat(64)));
    assert!(store.create_staging_for(&unavailable).await.is_err());

    let capacity = FakeCapacity::new(MIN_FREE_BYTES + 3);
    let store = LocalBlobStore::with_capacity(root.path(), capacity.clone());
    let staging = StorageKey::from_opaque(format!("staging:{}", "5".repeat(64)));
    store
        .create_staging_for(&staging)
        .await
        .expect("threshold permits staging");
    assert!(store.append(&staging, b"four").await.is_err());

    let capacity = FakeCapacity::new(u64::MAX);
    let store = LocalBlobStore::with_capacity(root.path(), capacity.clone());
    let staging = create_staging(&store, '6').await;
    store.append(&staging, b"blob").await.expect("append");
    capacity.set(MIN_FREE_BYTES - 1);
    let namespace =
        StorageNamespace::for_scope(DedupScope::Instance, LibraryId::new(), UploadId::new());
    assert!(
        store
            .promote(&staging, &identity(namespace, b"blob"))
            .await
            .is_err()
    );
    assert_eq!(
        store
            .read_range(&staging, 0, 4)
            .await
            .expect("staging remains"),
        b"blob"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn promotion_does_not_follow_an_internal_directory_symlink() {
    use std::os::unix::fs::symlink;

    let root = TempDir::new().expect("temporary root");
    let outside = TempDir::new().expect("outside directory");
    let store = LocalBlobStore::with_capacity(root.path(), FakeCapacity::new(u64::MAX));
    let staging = create_staging(&store, '7').await;
    store.append(&staging, b"blob").await.expect("append");
    symlink(outside.path(), root.path().join("objects")).expect("internal symlink");
    let namespace =
        StorageNamespace::for_scope(DedupScope::Library, LibraryId::new(), UploadId::new());
    assert!(
        store
            .promote(&staging, &identity(namespace, b"blob"))
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read_dir(outside.path())
            .expect("outside listing")
            .count(),
        0,
        "no directories may be created beyond the configured root"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn deletion_does_not_follow_an_internal_directory_symlink() {
    use std::os::unix::fs::symlink;

    let root = TempDir::new().expect("temporary root");
    let outside = TempDir::new().expect("outside directory");
    let hash = "0".repeat(64);
    let external_parent = outside.path().join("instance-v1/00/00");
    std::fs::create_dir_all(&external_parent).expect("external hierarchy");
    let external_file = external_parent.join(format!("{hash}-4"));
    std::fs::write(&external_file, b"blob").expect("external blob");
    symlink(outside.path(), root.path().join("objects")).expect("internal symlink");
    let store = LocalBlobStore::with_capacity(root.path(), FakeCapacity::new(u64::MAX));
    let key = format!("blob:instance-v1:{hash}:4")
        .parse()
        .expect("valid opaque key");
    assert!(store.delete(&key).await.is_err());
    assert_eq!(
        std::fs::read(external_file).expect("external blob remains"),
        b"blob"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn adapter_created_root_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let parent = TempDir::new().expect("parent");
    let root = parent.path().join("new-root");
    let store = LocalBlobStore::with_capacity(&root, FakeCapacity::new(u64::MAX));
    create_staging(&store, '8').await;
    let mode = std::fs::metadata(root)
        .expect("root metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o700);
}

#[test]
fn dedup_scopes_resolve_to_the_required_stable_or_fresh_namespaces() {
    let library = LibraryId::from_uuid(uuid::Uuid::from_u128(11));
    let first_upload = UploadId::from_uuid(uuid::Uuid::from_u128(12));
    let second_upload = UploadId::from_uuid(uuid::Uuid::from_u128(13));
    assert_eq!(
        StorageNamespace::for_scope(DedupScope::Instance, library, first_upload),
        StorageNamespace::for_scope(DedupScope::Instance, LibraryId::new(), second_upload)
    );
    assert_eq!(
        StorageNamespace::for_scope(DedupScope::Library, library, first_upload),
        StorageNamespace::for_scope(DedupScope::Library, library, second_upload)
    );
    assert_ne!(
        StorageNamespace::for_scope(DedupScope::Library, library, first_upload),
        StorageNamespace::for_scope(DedupScope::Library, LibraryId::new(), first_upload)
    );
    assert_ne!(
        StorageNamespace::for_scope(DedupScope::Disabled, library, first_upload),
        StorageNamespace::for_scope(DedupScope::Disabled, library, second_upload)
    );
}
