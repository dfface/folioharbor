#![allow(clippy::expect_used)]

use std::{
    fs,
    io::{self, Read},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use folioharbor_application::ports::BlobStore;
use folioharbor_domain::{
    id::{LibraryId, UploadId},
    imports::blob::{BlobIdentity, ByteCount, DedupScope, Sha256Digest, StorageNamespace},
};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use super::{CapacityProbe, HookPoint, LocalBlobStore, file_ops::read_up_to, paths};

#[derive(Clone, Copy, Debug)]
struct UnlimitedCapacity;

impl CapacityProbe for UnlimitedCapacity {
    fn free_bytes(&self, _: &Path) -> io::Result<u64> {
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

struct ShortReader {
    bytes: &'static [u8],
    offset: usize,
}

impl Read for ShortReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let count = output
            .len()
            .min(2)
            .min(self.bytes.len().saturating_sub(self.offset));
        output[..count].copy_from_slice(&self.bytes[self.offset..self.offset + count]);
        self.offset += count;
        Ok(count)
    }
}

#[test]
fn bounded_read_continues_after_short_reads_until_length_or_eof() {
    let mut reader = ShortReader {
        bytes: b"abcdef",
        offset: 0,
    };

    let bytes = read_up_to(&mut reader, 5).expect("read succeeds");

    assert_eq!(bytes, b"abcde");
}

#[tokio::test(flavor = "current_thread")]
async fn append_yields_the_current_thread_while_filesystem_work_is_blocked() {
    let root = TempDir::new().expect("root");
    let base = LocalBlobStore::with_capacity(root.path(), UnlimitedCapacity);
    let key = base.create_staging().await.expect("staging");
    let heartbeat = Arc::new(AtomicBool::new(false));
    let heartbeat_task = Arc::clone(&heartbeat);
    tokio::spawn(async move {
        heartbeat_task.store(true, Ordering::SeqCst);
    });

    let (entered_sender, entered_receiver) = mpsc::sync_channel(1);
    let release = Arc::new(AtomicBool::new(false));
    let hook_release = Arc::clone(&release);
    let store = base.with_test_hook(Arc::new(move |point| {
        if point == HookPoint::AppendOpen {
            entered_sender.send(()).expect("signal hook");
            while !hook_release.load(Ordering::SeqCst) {
                std::thread::yield_now();
            }
        }
    }));
    let observed = Arc::new(AtomicBool::new(false));
    let scheduling_result = Arc::clone(&observed);
    let releaser = Arc::clone(&release);
    let heartbeat_observer = Arc::clone(&heartbeat);
    let release_thread = std::thread::spawn(move || {
        entered_receiver.recv().expect("hook entered");
        let deadline = Instant::now() + Duration::from_millis(100);
        while Instant::now() < deadline && !heartbeat_observer.load(Ordering::SeqCst) {
            std::thread::yield_now();
        }
        scheduling_result.store(heartbeat_observer.load(Ordering::SeqCst), Ordering::SeqCst);
        releaser.store(true, Ordering::SeqCst);
    });

    store.append(&key, b"blob").await.expect("append");
    release_thread.join().expect("release thread");

    assert!(
        observed.load(Ordering::SeqCst),
        "the runtime could not schedule another task during filesystem work"
    );
}

#[tokio::test]
async fn promotion_recovery_syncs_matching_destination_then_its_parent() {
    let root = TempDir::new().expect("root");
    let base = LocalBlobStore::with_capacity(root.path(), UnlimitedCapacity);
    let staging = base.create_staging().await.expect("staging");
    base.append(&staging, b"blob").await.expect("payload");
    let blob = identity(b"blob");
    let destination = root.path().join(paths::final_relative(&blob));
    let staging_dir = root.path().join("staging");
    let events = Arc::new(Mutex::new(Vec::new()));
    let recorded_events = Arc::clone(&events);
    let store = base.with_test_hook(Arc::new(move |point| match point {
        HookPoint::PromoteSourceOpen => {
            fs::remove_dir_all(&staging_dir).expect("source disappears");
            fs::write(&destination, b"blob").expect("matching destination appears");
        }
        HookPoint::PromoteRecoveryDestinationSynced | HookPoint::PromoteRecoveryParentSynced => {
            recorded_events.lock().expect("events").push(point);
        }
        _ => {}
    }));

    store
        .promote(&staging, &blob)
        .await
        .expect("recover promotion");

    assert_eq!(
        *events.lock().expect("events"),
        [
            HookPoint::PromoteRecoveryDestinationSynced,
            HookPoint::PromoteRecoveryParentSynced,
        ]
    );
}
