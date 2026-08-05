#![allow(clippy::expect_used)]

use async_trait::async_trait;
use bytes::Bytes;
use folioharbor_application::{
    authorization::{Action, AuthorizationFact, ResourceRef},
    error::AppError,
    imports::{
        ReceiveUploadRequest, UploadApi, UploadRecoveryService, UploadService, UploadStreamError,
    },
    ports::{
        AuthorizationRepository, AuthorizationRepositoryError, AuthorizedUploadTransition,
        BeginUploadReceipt, BlobDisposition, BlobStore, BlobStoreError, ClaimUploadCleanup,
        CreateUploadRecord, ExpireUploads, FinalizeUploadReceipt, HeartbeatUploadReceipt,
        MarkUploadReceived, PrepareUploadPromotion, RecordPromotionDisposition, UploadCleanup,
        UploadCleanupGuard, UploadReceiptAttempt, UploadRepository, UploadRepositoryError,
        WorkerUploadTransition,
    },
};
use folioharbor_domain::{
    id::{LibraryId, RequestId, UploadId, UserId},
    imports::{
        blob::{BlobIdentity, DedupScope, StorageKey},
        quota::ByteCount,
        upload::{UploadSession, UploadState},
    },
    libraries::role::RoleCode,
    time::OffsetDateTime,
};
use std::sync::{Arc, Mutex};

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
    fail_finalize: Mutex<bool>,
    heartbeats: Mutex<u32>,
    cleanup_leased: Mutex<bool>,
    heartbeat_alive: Mutex<bool>,
}
struct FakeCleanupGuard(UploadCleanup);
#[async_trait]
impl UploadCleanupGuard for FakeCleanupGuard {
    fn cleanup(&self) -> &UploadCleanup {
        &self.0
    }
    async fn complete(self: Box<Self>, _: OffsetDateTime) -> Result<bool, UploadRepositoryError> {
        Ok(true)
    }
    async fn abandon(self: Box<Self>) -> Result<(), UploadRepositoryError> {
        Ok(())
    }
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
    async fn begin_receipt(
        &self,
        receipt: BeginUploadReceipt,
    ) -> Result<Option<UploadReceiptAttempt>, UploadRepositoryError> {
        let mut session = self.session.lock().expect("session");
        if session.state != receipt.from {
            return Ok(None);
        }
        self.transitions
            .lock()
            .expect("transitions")
            .push((receipt.from, UploadState::Receiving));
        let staging_key = format!("staging:{}", "a".repeat(64));
        session.state = UploadState::Receiving;
        session.storage_key = Some(StorageKey::from_opaque(staging_key.clone()));
        Ok(Some(UploadReceiptAttempt {
            attempt_token: UploadId::new().as_uuid().to_string(),
            staging_key,
        }))
    }
    async fn finalize_authorized(
        &self,
        receipt: FinalizeUploadReceipt,
    ) -> Result<bool, UploadRepositoryError> {
        let mut fail = self.fail_finalize.lock().expect("failure injection");
        if *fail {
            *fail = false;
            return Err(UploadRepositoryError::Persistence);
        }
        drop(fail);
        let mut session = self.session.lock().expect("session");
        if !matches!(
            session.state,
            UploadState::Receiving | UploadState::Received | UploadState::Queued
        ) {
            return Ok(false);
        }
        if session.state != UploadState::Queued {
            self.transitions
                .lock()
                .expect("transitions")
                .push((session.state, UploadState::Queued));
        }
        session.state = UploadState::Queued;
        session.received_bytes = receipt.received;
        session.storage_key = Some(StorageKey::from_opaque(receipt.storage_key));
        Ok(true)
    }
    async fn heartbeat_receipt(
        &self,
        _: HeartbeatUploadReceipt,
    ) -> Result<bool, UploadRepositoryError> {
        *self.heartbeats.lock().expect("heartbeats") += 1;
        Ok(*self.heartbeat_alive.lock().expect("heartbeat alive"))
    }
    async fn prepare_promotion(
        &self,
        _: PrepareUploadPromotion,
    ) -> Result<bool, UploadRepositoryError> {
        Ok(true)
    }
    async fn record_promotion_disposition(
        &self,
        _: RecordPromotionDisposition,
    ) -> Result<bool, UploadRepositoryError> {
        Ok(true)
    }
    async fn mark_received(
        &self,
        receipt: MarkUploadReceived,
    ) -> Result<bool, UploadRepositoryError> {
        let mut session = self.session.lock().expect("session");
        if session.state != UploadState::Receiving {
            return Ok(false);
        }
        self.transitions
            .lock()
            .expect("transitions")
            .push((UploadState::Receiving, UploadState::Received));
        session.state = UploadState::Received;
        session.received_bytes = receipt.received;
        session.storage_key = Some(StorageKey::from_opaque(receipt.final_key));
        Ok(true)
    }
    async fn transition_worker(
        &self,
        _: WorkerUploadTransition,
    ) -> Result<bool, UploadRepositoryError> {
        unreachable!()
    }
    async fn expire_worker(&self, _: ExpireUploads) -> Result<u64, UploadRepositoryError> {
        Ok(0)
    }
    async fn claim_cleanup(
        &self,
        _: ClaimUploadCleanup,
    ) -> Result<Option<Box<dyn UploadCleanupGuard>>, UploadRepositoryError> {
        let mut leased = self.cleanup_leased.lock().expect("cleanup lease");
        if *leased {
            return Ok(None);
        }
        *leased = true;
        let session = self.session.lock().expect("session");
        Ok(Some(Box::new(FakeCleanupGuard(UploadCleanup {
            upload_id: session.upload_id,
            attempt_token: UploadId::new().as_uuid().to_string(),
            staging_key: "staging:test".into(),
            final_key: Some("blob:owned:test:4".into()),
            final_owned: true,
        }))))
    }
}
#[derive(Default)]
struct Blobs {
    deleted: Mutex<u32>,
    appended: Mutex<Vec<usize>>,
    promoted_namespaces: Mutex<Vec<String>>,
}
#[async_trait]
impl BlobStore for Blobs {
    fn candidate_key(&self, identity: &BlobIdentity) -> StorageKey {
        StorageKey::from_opaque(format!("blobs:{}", identity.namespace().as_str()))
    }
    async fn create_staging_for(&self, _: &StorageKey) -> Result<(), BlobStoreError> {
        Ok(())
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
        identity: &BlobIdentity,
    ) -> Result<folioharbor_application::ports::PromotedBlob, BlobStoreError> {
        self.promoted_namespaces
            .lock()
            .expect("namespaces")
            .push(identity.namespace().as_str().to_owned());
        Ok(folioharbor_application::ports::PromotedBlob {
            key: self.candidate_key(identity),
            disposition: BlobDisposition::Installed,
        })
    }
    async fn delete(&self, _: &StorageKey) -> Result<(), BlobStoreError> {
        *self.deleted.lock().expect("deleted") += 1;
        Ok(())
    }
    async fn free_bytes(&self) -> Result<u64, BlobStoreError> {
        Ok(u64::MAX)
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
    fixture_scope(declared, DedupScope::Instance)
}

fn fixture_scope(
    declared: u64,
    scope: DedupScope,
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
        fail_finalize: Mutex::new(false),
        heartbeats: Mutex::new(0),
        cleanup_leased: Mutex::new(false),
        heartbeat_alive: Mutex::new(true),
    });
    let blobs = Arc::new(Blobs::default());
    let service = UploadService::new(
        uploads.clone(),
        Arc::new(Allow),
        blobs.clone(),
        Arc::new(FixedClock(OffsetDateTime::UNIX_EPOCH)),
        scope,
    );
    (service, uploads, blobs, actor, library, upload)
}

#[tokio::test]
async fn recovery_deletes_staging_and_only_owned_final_before_acknowledging_cleanup() {
    let (_service, uploads, blobs, _actor, _library, _upload) = fixture(4);
    let recovery = UploadRecoveryService::new(uploads, blobs.clone());
    assert_eq!(
        recovery
            .reconcile("recovery-a", OffsetDateTime::UNIX_EPOCH, 10)
            .await
            .expect("reconcile"),
        1
    );
    assert_eq!(*blobs.deleted.lock().expect("deleted"), 2);
}

#[tokio::test]
async fn configured_dedup_scope_selects_instance_library_and_upload_namespaces() {
    for scope in [
        DedupScope::Instance,
        DedupScope::Library,
        DedupScope::Disabled,
    ] {
        let (service, _uploads, blobs, actor, library, upload) = fixture_scope(4, scope);
        service
            .receive_upload(request(
                actor,
                library,
                upload,
                vec![Ok(Bytes::from_static(b"book"))],
            ))
            .await
            .expect("queued");
        let expected = match scope {
            DedupScope::Instance => "instance-v1".to_owned(),
            DedupScope::Library => format!("library-{}", library.as_uuid().simple()),
            DedupScope::Disabled => format!("upload-{}", upload.as_uuid().simple()),
        };
        assert_eq!(
            blobs
                .promoted_namespaces
                .lock()
                .expect("namespaces")
                .as_slice(),
            &[expected]
        );
    }
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
    let (service, uploads, blobs, actor, library, upload) = fixture(size as u64);
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
    assert_eq!(*uploads.heartbeats.lock().expect("heartbeats"), 0);
}

#[tokio::test(start_paused = true)]
async fn idle_body_is_heartbeated_and_lost_ownership_stops_without_deleting() {
    let (service, uploads, blobs, actor, library, upload) = fixture(4);
    *uploads.heartbeat_alive.lock().expect("heartbeat alive") = false;
    let service = service.with_receipt_heartbeat_interval(std::time::Duration::from_secs(30));
    let receive = tokio::spawn(async move {
        service
            .receive_upload(ReceiveUploadRequest {
                actor,
                request_id: RequestId::new(),
                library_id: library,
                upload_id: upload,
                bytes: Box::pin(futures_util::stream::pending()),
            })
            .await
    });
    tokio::task::yield_now().await;
    tokio::time::advance(std::time::Duration::from_secs(31)).await;
    let error = receive
        .await
        .expect("receive task")
        .expect_err("lease lost");
    assert!(matches!(
        error,
        AppError::Conflict {
            code: "upload_receipt_lease_lost"
        }
    ));
    assert_eq!(*uploads.heartbeats.lock().expect("heartbeats"), 1);
    assert_eq!(*blobs.deleted.lock().expect("deleted"), 0);
}

#[tokio::test]
async fn same_upload_retries_every_post_promotion_persistence_cut() {
    for _former_cut in ["receipt", "enqueue", "queue"] {
        let (service, uploads, _blobs, actor, library, upload) = fixture(4);
        *uploads.fail_finalize.lock().expect("failure injection") = true;
        service
            .receive_upload(request(
                actor,
                library,
                upload,
                vec![Ok(Bytes::from_static(b"book"))],
            ))
            .await
            .expect_err("injected post-promotion cut");
        let recovered = service
            .receive_upload(request(
                actor,
                library,
                upload,
                vec![Ok(Bytes::from_static(b"book"))],
            ))
            .await
            .expect("same upload id recovers");
        assert_eq!(recovered.upload_id, upload);
        assert_eq!(recovered.state, UploadState::Queued);
    }
}
