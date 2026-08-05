#![allow(clippy::expect_used)]

use async_trait::async_trait;
use bytes::Bytes;
use folioharbor_application::{
    authorization::{Action, AuthorizationFact, ResourceRef},
    error::AppError,
    imports::{ReceiveUploadRequest, UploadApi, UploadService, UploadStreamError},
    ports::{
        AuthorizationRepository, AuthorizationRepositoryError, AuthorizedUploadTransition,
        BlobStore, BlobStoreError, CreateUploadRecord, JobRepository, JobRepositoryError,
        LeaseJobs, UploadRepository, UploadRepositoryError, WorkerUploadTransition,
    },
};
use folioharbor_domain::{
    id::{JobId, LibraryId, RequestId, UploadId, UserId},
    imports::{
        blob::{BlobIdentity, StorageKey},
        job::{JobInput, JobKind, LeasedJob},
        quota::ByteCount,
        upload::{UploadSession, UploadState},
    },
    libraries::role::RoleCode,
    time::OffsetDateTime,
};
use std::sync::{Arc, Mutex};
use time::Duration;

struct Allow;
#[async_trait]
impl AuthorizationRepository for Allow {
    async fn resolve(
        &self,
        _: UserId,
        action: Action,
        resource: ResourceRef,
    ) -> Result<Option<AuthorizationFact>, AuthorizationRepositoryError> {
        Ok(Some(AuthorizationFact {
            library_id: resource.library_id(),
            role: RoleCode::Editor,
            membership_version: 1,
            discoverable: true,
            permitted: matches!(action, Action::InspectUpload),
        }))
    }
}
struct FixedClock(OffsetDateTime);
impl folioharbor_application::ports::Clock for FixedClock {
    fn now(&self) -> OffsetDateTime {
        self.0
    }
}

struct Uploads {
    session: Mutex<UploadSession>,
    transitions: Mutex<Vec<(UploadState, UploadState)>>,
}
#[async_trait]
impl UploadRepository for Uploads {
    async fn create_authorized(
        &self,
        _: CreateUploadRecord,
    ) -> Result<UploadSession, UploadRepositoryError> {
        Err(UploadRepositoryError::Persistence)
    }
    async fn find_authorized(
        &self,
        _: UserId,
        _: LibraryId,
        _: UploadId,
        _: RequestId,
    ) -> Result<Option<UploadSession>, UploadRepositoryError> {
        Ok(Some(self.session.lock().expect("session").clone()))
    }
    async fn transition_authorized(
        &self,
        change: AuthorizedUploadTransition,
    ) -> Result<bool, UploadRepositoryError> {
        let mut session = self.session.lock().expect("session");
        if session.state != change.from {
            return Ok(false);
        }
        session.state = change.to;
        session.received_bytes = change.received;
        session.storage_key = change.storage_key.map(StorageKey::from_opaque);
        session.error_code = change.error_code;
        self.transitions
            .lock()
            .expect("transitions")
            .push((change.from, change.to));
        Ok(true)
    }
    async fn transition_worker(
        &self,
        _: WorkerUploadTransition,
    ) -> Result<bool, UploadRepositoryError> {
        unreachable!()
    }
}
#[derive(Default)]
struct Blobs {
    deleted: Mutex<u32>,
    appended: Mutex<Vec<usize>>,
}
#[async_trait]
impl BlobStore for Blobs {
    async fn create_staging(&self) -> Result<StorageKey, BlobStoreError> {
        Ok(StorageKey::from_opaque("staging:test".into()))
    }
    async fn append(&self, _: &StorageKey, bytes: &[u8]) -> Result<(), BlobStoreError> {
        self.appended.lock().expect("appended").push(bytes.len());
        Ok(())
    }
    async fn read_range(&self, _: &StorageKey, _: u64, _: u64) -> Result<Vec<u8>, BlobStoreError> {
        unreachable!()
    }
    async fn promote(
        &self,
        _: &StorageKey,
        _: &BlobIdentity,
    ) -> Result<StorageKey, BlobStoreError> {
        Ok(StorageKey::from_opaque("blobs:test".into()))
    }
    async fn delete(&self, _: &StorageKey) -> Result<(), BlobStoreError> {
        *self.deleted.lock().expect("deleted") += 1;
        Ok(())
    }
    async fn free_bytes(&self) -> Result<u64, BlobStoreError> {
        Ok(u64::MAX)
    }
}
struct Jobs;
#[async_trait]
impl JobRepository for Jobs {
    async fn enqueue(
        &self,
        id: JobId,
        _: LibraryId,
        _: JobKind,
        _: JobInput,
        _: &str,
        _: OffsetDateTime,
    ) -> Result<JobId, JobRepositoryError> {
        Ok(id)
    }
    async fn lease(&self, _: LeaseJobs) -> Result<Vec<LeasedJob>, JobRepositoryError> {
        unreachable!()
    }
    async fn heartbeat(
        &self,
        _: JobId,
        _: &str,
        _: OffsetDateTime,
        _: Duration,
    ) -> Result<bool, JobRepositoryError> {
        unreachable!()
    }
    async fn succeed(
        &self,
        _: JobId,
        _: &str,
        _: OffsetDateTime,
    ) -> Result<bool, JobRepositoryError> {
        unreachable!()
    }
    async fn retry(
        &self,
        _: JobId,
        _: &str,
        _: OffsetDateTime,
        _: OffsetDateTime,
        _: &str,
        _: &str,
    ) -> Result<bool, JobRepositoryError> {
        unreachable!()
    }
    async fn fail(
        &self,
        _: JobId,
        _: &str,
        _: OffsetDateTime,
        _: &str,
        _: &str,
    ) -> Result<bool, JobRepositoryError> {
        unreachable!()
    }
}

fn fixture(
    declared: u64,
) -> (
    UploadService,
    Arc<Uploads>,
    Arc<Blobs>,
    UserId,
    LibraryId,
    UploadId,
) {
    let actor = UserId::new();
    let library = LibraryId::new();
    let upload = UploadId::new();
    let uploads = Arc::new(Uploads {
        session: Mutex::new(UploadSession {
            upload_id: upload,
            library_id: library,
            file_name: "book.epub".into(),
            media_type: "application/epub+zip".into(),
            declared_bytes: ByteCount::new(declared),
            received_bytes: ByteCount::new(0),
            state: UploadState::Created,
            storage_key: None,
            error_code: None,
        }),
        transitions: Mutex::new(Vec::new()),
    });
    let blobs = Arc::new(Blobs::default());
    let service = UploadService::new(
        uploads.clone(),
        Arc::new(Allow),
        blobs.clone(),
        Arc::new(Jobs),
        Arc::new(FixedClock(OffsetDateTime::UNIX_EPOCH)),
    );
    (service, uploads, blobs, actor, library, upload)
}
fn request(
    actor: UserId,
    library: LibraryId,
    upload: UploadId,
    items: Vec<Result<Bytes, UploadStreamError>>,
) -> ReceiveUploadRequest {
    ReceiveUploadRequest {
        actor,
        request_id: RequestId::new(),
        library_id: library,
        upload_id: upload,
        bytes: Box::pin(futures_util::stream::iter(items)),
    }
}

#[tokio::test]
async fn oversized_and_interrupted_streams_fail_recoverably_and_release_once() {
    for (items, code) in [
        (
            vec![Ok(Bytes::from_static(b"12345"))],
            "upload_exceeds_declared_size",
        ),
        (
            vec![Ok(Bytes::from_static(b"12")), Err(UploadStreamError)],
            "upload_interrupted",
        ),
    ] {
        let (service, uploads, blobs, actor, library, upload) = fixture(4);
        let error = service
            .receive_upload(request(actor, library, upload, items))
            .await
            .expect_err("must fail");
        assert!(matches!(error,AppError::Invalid{code:actual,..} if actual==code));
        assert_eq!(
            uploads.session.lock().expect("session").state,
            UploadState::Failed
        );
        assert_eq!(*blobs.deleted.lock().expect("deleted"), 1);
        assert_eq!(
            uploads.transitions.lock().expect("transitions").as_slice(),
            &[
                (UploadState::Created, UploadState::Receiving),
                (UploadState::Receiving, UploadState::Failed)
            ]
        );
    }
}

#[tokio::test]
async fn large_body_frames_are_split_into_bounded_blob_appends() {
    let size = 1024 * 1024 + 7;
    let (service, _uploads, blobs, actor, library, upload) = fixture(size as u64);
    service
        .receive_upload(request(
            actor,
            library,
            upload,
            vec![Ok(Bytes::from(vec![7_u8; size]))],
        ))
        .await
        .expect("queued");
    assert_eq!(
        blobs.appended.lock().expect("appended").as_slice(),
        &[1024 * 1024, 7]
    );
}
