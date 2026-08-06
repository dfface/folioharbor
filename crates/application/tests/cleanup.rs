#![allow(clippy::expect_used)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use folioharbor_application::{
    imports::{CleanupCursor, CleanupImports, CleanupJobKind},
    ports::{
        BlobStore, BlobStoreError, FailedUploadPurge, ImportCleanupRepository,
        ImportCleanupRepositoryError, PromotedBlob,
    },
};
use folioharbor_domain::{
    id::UploadId,
    imports::blob::{BlobIdentity, StorageKey},
    time::OffsetDateTime,
};

#[test]
fn cleanup_jobs_are_closed_and_every_pass_has_a_stable_time_boundary() {
    let boundary = OffsetDateTime::from_unix_timestamp(1_700_000_000).expect("valid fixture time");
    let cursor = CleanupCursor::new(boundary, 50).expect("bounded batch");
    let kinds = [
        CleanupJobKind::ExpireUploadsAndReservations,
        CleanupJobKind::PurgeFailedUploads,
        CleanupJobKind::CollectBlobsLater,
    ];

    assert_eq!(cursor.not_after(), boundary);
    assert_eq!(cursor.limit(), 50);
    assert_eq!(kinds.len(), 3);
    assert!(CleanupCursor::new(boundary, 0).is_none());
    assert!(CleanupCursor::new(boundary, 1_001).is_none());
}

struct CleanupRepo {
    claims: Mutex<Vec<FailedUploadPurge>>,
    completed: Mutex<Vec<UploadId>>,
}

#[async_trait]
impl ImportCleanupRepository for CleanupRepo {
    async fn expire_abandoned(
        &self,
        _: CleanupCursor,
    ) -> Result<u64, ImportCleanupRepositoryError> {
        Ok(2)
    }
    async fn claim_failed_purges(
        &self,
        _: &str,
        _: CleanupCursor,
    ) -> Result<Vec<FailedUploadPurge>, ImportCleanupRepositoryError> {
        Ok(self.claims.lock().expect("claim fixture").clone())
    }
    async fn complete_failed_purge(
        &self,
        upload_id: UploadId,
        _: &str,
        _: CleanupCursor,
    ) -> Result<bool, ImportCleanupRepositoryError> {
        self.completed
            .lock()
            .expect("complete fixture")
            .push(upload_id);
        Ok(true)
    }
}

#[derive(Default)]
struct Blobs(Mutex<Vec<String>>);

#[async_trait]
impl BlobStore for Blobs {
    fn candidate_key(&self, _: &BlobIdentity) -> StorageKey {
        StorageKey::from_opaque("unused".into())
    }
    async fn create_staging_for(&self, _: &StorageKey) -> Result<(), BlobStoreError> {
        Ok(())
    }
    async fn append(&self, _: &StorageKey, _: &[u8]) -> Result<(), BlobStoreError> {
        Ok(())
    }
    async fn read_range(&self, _: &StorageKey, _: u64, _: u64) -> Result<Vec<u8>, BlobStoreError> {
        Ok(Vec::new())
    }
    async fn promote(
        &self,
        _: &StorageKey,
        _: &BlobIdentity,
    ) -> Result<PromotedBlob, BlobStoreError> {
        Err(BlobStoreError::InvalidKey)
    }
    async fn delete(&self, key: &StorageKey) -> Result<(), BlobStoreError> {
        self.0
            .lock()
            .expect("blob fixture")
            .push(key.as_str().to_owned());
        Ok(())
    }
    async fn free_bytes(&self) -> Result<u64, BlobStoreError> {
        Ok(u64::MAX)
    }
}

#[tokio::test]
async fn failed_purge_deletes_only_owned_bytes_and_completes_every_durable_claim() {
    let boundary = OffsetDateTime::from_unix_timestamp(1_700_000_000).expect("fixture time");
    let owned = UploadId::new();
    let shared = UploadId::new();
    let repository = Arc::new(CleanupRepo {
        claims: Mutex::new(vec![
            FailedUploadPurge {
                upload_id: owned,
                storage_key: StorageKey::from_opaque("blob:upload-owned".into()),
                delete_file: true,
            },
            FailedUploadPurge {
                upload_id: shared,
                storage_key: StorageKey::from_opaque("blob:instance-shared".into()),
                delete_file: false,
            },
        ]),
        completed: Mutex::new(Vec::new()),
    });
    let blobs = Arc::new(Blobs::default());
    let cleanup = CleanupImports::new(repository.clone(), blobs.clone());

    let result = cleanup
        .run(
            "worker-a",
            CleanupCursor::new(boundary, 10).expect("cursor"),
        )
        .await
        .expect("cleanup succeeds");

    assert_eq!(result.expired, 2);
    assert_eq!(result.purged, 2);
    assert_eq!(
        *blobs.0.lock().expect("blob fixture"),
        vec!["blob:upload-owned"]
    );
    assert_eq!(
        *repository.completed.lock().expect("complete fixture"),
        vec![owned, shared]
    );
}
