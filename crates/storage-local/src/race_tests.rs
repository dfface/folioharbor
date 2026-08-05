#![allow(clippy::expect_used)]

use std::{
    fs,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use folioharbor_application::ports::{BlobStore, BlobStoreError};
use folioharbor_domain::{
    id::{LibraryId, UploadId},
    imports::blob::{
        BlobIdentity, ByteCount, DedupScope, Sha256Digest, StorageKey, StorageNamespace,
    },
};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use super::{CapacityProbe, HookPoint, LocalBlobStore, paths};

#[derive(Clone, Copy, Debug)]
struct UnlimitedCapacity;

impl CapacityProbe for UnlimitedCapacity {
    fn free_bytes(&self, _: &Path) -> std::io::Result<u64> {
        Ok(u64::MAX)
    }
}

fn identity(payload: &[u8]) -> BlobIdentity {
    BlobIdentity::new(
        StorageNamespace::for_scope(DedupScope::Instance, LibraryId::new(), UploadId::new()),
        Sha256Digest::from_bytes(Sha256::digest(payload).into()),
        ByteCount::new(payload.len() as u64),
    )
}

async fn create_staging(store: &LocalBlobStore<UnlimitedCapacity>, marker: char) -> StorageKey {
    let key = StorageKey::from_opaque(format!("staging:{}", marker.to_string().repeat(64)));
    store.create_staging_for(&key).await.expect("staging");
    key
}

fn one_shot_swap(
    point: HookPoint,
    action: impl Fn() + Send + Sync + 'static,
) -> Arc<dyn Fn(HookPoint) + Send + Sync> {
    let fired = AtomicBool::new(false);
    Arc::new(move |actual| {
        if actual == point && !fired.swap(true, Ordering::SeqCst) {
            action();
        }
    })
}

#[cfg(unix)]
#[tokio::test]
async fn append_cannot_escape_when_staging_directory_is_swapped_after_validation() {
    use std::os::unix::fs::symlink;

    let root = TempDir::new().expect("root");
    let outside = TempDir::new().expect("outside");
    let base = LocalBlobStore::with_capacity(root.path(), UnlimitedCapacity);
    let key = create_staging(&base, '1').await;
    let token = key
        .as_str()
        .strip_prefix("staging:")
        .expect("token")
        .to_owned();
    fs::write(outside.path().join(&token), b"outside").expect("outside file");
    let root_path = root.path().to_owned();
    let outside_path = outside.path().to_owned();
    let store = base.with_test_hook(one_shot_swap(HookPoint::AppendOpen, move || {
        fs::rename(root_path.join("staging"), root_path.join("staging-held")).expect("swap out");
        symlink(&outside_path, root_path.join("staging")).expect("swap in");
    }));

    let _ = store.append(&key, b"-append").await;

    assert_eq!(
        fs::read(outside.path().join(token)).expect("outside unchanged"),
        b"outside"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn read_cannot_observe_outside_bytes_when_staging_directory_is_swapped() {
    use std::os::unix::fs::symlink;

    let root = TempDir::new().expect("root");
    let outside = TempDir::new().expect("outside");
    let base = LocalBlobStore::with_capacity(root.path(), UnlimitedCapacity);
    let key = create_staging(&base, '2').await;
    base.append(&key, b"inside").await.expect("inside bytes");
    let token = key
        .as_str()
        .strip_prefix("staging:")
        .expect("token")
        .to_owned();
    fs::write(outside.path().join(&token), b"secret").expect("outside file");
    let root_path = root.path().to_owned();
    let outside_path = outside.path().to_owned();
    let store = base.with_test_hook(one_shot_swap(HookPoint::ReadOpen, move || {
        fs::rename(root_path.join("staging"), root_path.join("staging-held")).expect("swap out");
        symlink(&outside_path, root_path.join("staging")).expect("swap in");
    }));

    let result = store.read_range(&key, 0, 6).await;

    assert_ne!(result.expect("capability retains inside file"), b"secret");
}

#[cfg(unix)]
#[tokio::test]
async fn delete_cannot_remove_outside_file_when_staging_directory_is_swapped() {
    use std::os::unix::fs::symlink;

    let root = TempDir::new().expect("root");
    let outside = TempDir::new().expect("outside");
    let base = LocalBlobStore::with_capacity(root.path(), UnlimitedCapacity);
    let key = create_staging(&base, '3').await;
    let token = key
        .as_str()
        .strip_prefix("staging:")
        .expect("token")
        .to_owned();
    fs::write(outside.path().join(&token), b"outside").expect("outside file");
    let root_path = root.path().to_owned();
    let outside_path = outside.path().to_owned();
    let store = base.with_test_hook(one_shot_swap(HookPoint::Delete, move || {
        fs::rename(root_path.join("staging"), root_path.join("staging-held")).expect("swap out");
        symlink(&outside_path, root_path.join("staging")).expect("swap in");
    }));

    let _ = store.delete(&key).await;

    assert_eq!(
        fs::read(outside.path().join(token)).expect("outside remains"),
        b"outside"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn promotion_cannot_escape_when_object_directory_is_swapped_before_install() {
    use std::os::unix::fs::symlink;

    let root = TempDir::new().expect("root");
    let outside = TempDir::new().expect("outside");
    let base = LocalBlobStore::with_capacity(root.path(), UnlimitedCapacity);
    let staging = create_staging(&base, '4').await;
    base.append(&staging, b"blob").await.expect("payload");
    let blob = identity(b"blob");
    let relative = paths::final_relative(&blob);
    let outside_relative = relative
        .strip_prefix("objects")
        .expect("object-relative path")
        .to_owned();
    fs::create_dir_all(
        outside
            .path()
            .join(outside_relative.parent().expect("final parent")),
    )
    .expect("outside hierarchy");
    let root_path = root.path().to_owned();
    let outside_path = outside.path().to_owned();
    let store = base.with_test_hook(one_shot_swap(HookPoint::PromoteInstall, move || {
        fs::rename(root_path.join("objects"), root_path.join("objects-held")).expect("swap out");
        symlink(&outside_path, root_path.join("objects")).expect("swap in");
    }));

    let _ = store.promote(&staging, &blob).await;

    assert!(!outside.path().join(outside_relative).exists());
}

#[tokio::test]
async fn concurrent_destination_creation_never_gets_overwritten_during_install() {
    let root = TempDir::new().expect("root");
    let base = LocalBlobStore::with_capacity(root.path(), UnlimitedCapacity);
    let staging = create_staging(&base, '5').await;
    base.append(&staging, b"blob").await.expect("payload");
    let blob = identity(b"blob");
    let destination = root.path().join(paths::final_relative(&blob));
    let collision = destination.clone();
    let store = base.with_test_hook(one_shot_swap(HookPoint::PromoteInstall, move || {
        let collision = collision.clone();
        std::thread::spawn(move || fs::write(&collision, b"evil"))
            .join()
            .expect("collision thread")
            .expect("collision");
    }));

    let result = store.promote(&staging, &blob).await;

    assert!(matches!(result, Err(BlobStoreError::IdentityMismatch)));
    assert_eq!(fs::read(destination).expect("collision remains"), b"evil");
}
