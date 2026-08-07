#![allow(clippy::expect_used, clippy::too_many_lines)]

use async_trait::async_trait;
use bytes::Bytes;
use folioharbor_application::{
    error::AppError,
    imports::{ReceiveUploadRequest, UploadApi, UploadService},
    ports::{
        AuthorizedUploadTransition, BeginUploadReceipt, Clock, CreateUploadRecord, ExpireUploads,
        FinalizeUploadReceipt, HeartbeatUploadReceipt, MarkUploadReceived, PrepareUploadPromotion,
        RecordPromotionDisposition, UploadReceiptAttempt, UploadRepository, UploadRepositoryError,
        WorkerUploadTransition,
    },
};
use folioharbor_domain::{
    id::{LibraryId, RequestId, UploadId, UserId},
    imports::{
        quota::ByteCount,
        upload::{UploadSession, UploadState},
    },
    time::OffsetDateTime,
};
use folioharbor_postgres::{
    PgAuthorizationRepository, PgPools, PgUploadRepository, run_migrations,
};
use folioharbor_storage_local::LocalBlobStore;
use folioharbor_test_support::postgres::TestPostgres;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use time::Duration;

#[derive(Clone, Copy)]
struct FixedClock(OffsetDateTime);
impl Clock for FixedClock {
    fn now(&self) -> OffsetDateTime {
        self.0
    }
}

struct FailFirstFinalize {
    inner: PgUploadRepository,
    fail: AtomicBool,
}
#[async_trait]
impl UploadRepository for FailFirstFinalize {
    async fn create_authorized(
        &self,
        record: CreateUploadRecord,
    ) -> Result<UploadSession, UploadRepositoryError> {
        self.inner.create_authorized(record).await
    }
    async fn find_authorized(
        &self,
        actor: UserId,
        library: LibraryId,
        upload: UploadId,
        request: RequestId,
    ) -> Result<Option<UploadSession>, UploadRepositoryError> {
        self.inner
            .find_authorized(actor, library, upload, request)
            .await
    }
    async fn transition_authorized(
        &self,
        transition: AuthorizedUploadTransition,
    ) -> Result<bool, UploadRepositoryError> {
        self.inner.transition_authorized(transition).await
    }
    async fn begin_receipt(
        &self,
        receipt: BeginUploadReceipt,
    ) -> Result<Option<UploadReceiptAttempt>, UploadRepositoryError> {
        self.inner.begin_receipt(receipt).await
    }
    async fn finalize_authorized(
        &self,
        receipt: FinalizeUploadReceipt,
    ) -> Result<bool, UploadRepositoryError> {
        if self.fail.swap(false, Ordering::SeqCst) {
            return Err(UploadRepositoryError::Persistence);
        }
        self.inner.finalize_authorized(receipt).await
    }
    async fn heartbeat_receipt(
        &self,
        receipt: HeartbeatUploadReceipt,
    ) -> Result<bool, UploadRepositoryError> {
        self.inner.heartbeat_receipt(receipt).await
    }
    async fn prepare_promotion(
        &self,
        promotion: PrepareUploadPromotion,
    ) -> Result<bool, UploadRepositoryError> {
        self.inner.prepare_promotion(promotion).await
    }
    async fn record_promotion_disposition(
        &self,
        promotion: RecordPromotionDisposition,
    ) -> Result<bool, UploadRepositoryError> {
        self.inner.record_promotion_disposition(promotion).await
    }
    async fn mark_received(
        &self,
        receipt: MarkUploadReceived,
    ) -> Result<bool, UploadRepositoryError> {
        self.inner.mark_received(receipt).await
    }
    async fn transition_worker(
        &self,
        transition: WorkerUploadTransition,
    ) -> Result<bool, UploadRepositoryError> {
        self.inner.transition_worker(transition).await
    }
    async fn expire_worker(&self, request: ExpireUploads) -> Result<u64, UploadRepositoryError> {
        self.inner.expire_worker(request).await
    }
}

#[tokio::test]
async fn process_restart_recovers_promoted_receipt_with_same_upload_id() -> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_migrations(&pools.owner).await?;
    let directory = tempfile::tempdir()?;
    std::fs::create_dir_all(directory.path())?;
    let now = OffsetDateTime::now_utc();
    let actor = UserId::new();
    let library = LibraryId::new();
    let upload = UploadId::new();
    sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,$2,$2,'verified',$3,$3)").bind(actor.as_uuid()).bind("process-restart@test.invalid").bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,created_at,updated_at) VALUES($1,'Restart upload',$2,$2)").bind(library.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.library_memberships(library_id,user_id,role_code,status,joined_at) VALUES($1,$2,'editor','active',$3)").bind(library.as_uuid()).bind(actor.as_uuid()).bind(now).execute(&pools.owner).await?;
    let uploads = PgUploadRepository::new(pools.api.clone());
    uploads
        .create_authorized(CreateUploadRecord {
            upload_id: upload,
            library_id: library,
            actor,
            request_id: RequestId::new(),
            file_name: "restart.epub".into(),
            media_type: "application/epub+zip".into(),
            declared_bytes: ByteCount::new(4),
            dedup_scope: folioharbor_domain::imports::blob::DedupScope::Instance,
            expires_at: now + Duration::hours(24),
            now,
        })
        .await?;
    let blobs = Arc::new(LocalBlobStore::new(directory.path()));
    let failing = UploadService::new(
        Arc::new(FailFirstFinalize {
            inner: uploads.clone(),
            fail: AtomicBool::new(true),
        }),
        Arc::new(PgAuthorizationRepository::new(pools.api.clone())),
        blobs.clone(),
        Arc::new(FixedClock(now)),
        folioharbor_domain::imports::blob::DedupScope::Instance,
        u64::MAX,
    );
    failing
        .receive_upload(request(actor, library, upload))
        .await
        .expect_err("injected finalize cut");
    assert_eq!(
        uploads
            .find_authorized(actor, library, upload, RequestId::new())
            .await?
            .expect("upload after cut")
            .state,
        UploadState::Received
    );
    drop(failing);
    let restarted = UploadService::new(
        Arc::new(uploads),
        Arc::new(PgAuthorizationRepository::new(pools.api.clone())),
        blobs,
        Arc::new(FixedClock(now + Duration::seconds(1))),
        folioharbor_domain::imports::blob::DedupScope::Instance,
        u64::MAX,
    );
    let recovered = restarted
        .receive_upload(request(actor, library, upload))
        .await?;
    assert_eq!(recovered.upload_id, upload);
    assert_eq!(recovered.state, UploadState::Queued);
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn concurrent_receipt_loser_never_creates_a_staging_file() -> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_migrations(&pools.owner).await?;
    let directory = tempfile::tempdir()?;
    let now = OffsetDateTime::now_utc();
    let actor = UserId::new();
    let library = LibraryId::new();
    let upload = UploadId::new();
    sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,$2,$2,'verified',$3,$3)")
        .bind(actor.as_uuid()).bind("concurrent-receipt@test.invalid").bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,created_at,updated_at) VALUES($1,'Concurrent upload',$2,$2)")
        .bind(library.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.library_memberships(library_id,user_id,role_code,status,joined_at) VALUES($1,$2,'editor','active',$3)")
        .bind(library.as_uuid()).bind(actor.as_uuid()).bind(now).execute(&pools.owner).await?;
    let uploads = Arc::new(PgUploadRepository::new(pools.api.clone()));
    uploads
        .create_authorized(CreateUploadRecord {
            upload_id: upload,
            library_id: library,
            actor,
            request_id: RequestId::new(),
            file_name: "concurrent.epub".into(),
            media_type: "application/epub+zip".into(),
            declared_bytes: ByteCount::new(4),
            dedup_scope: folioharbor_domain::imports::blob::DedupScope::Instance,
            expires_at: now + Duration::hours(24),
            now,
        })
        .await?;
    let service = Arc::new(UploadService::new(
        uploads,
        Arc::new(PgAuthorizationRepository::new(pools.api.clone())),
        Arc::new(LocalBlobStore::new(directory.path())),
        Arc::new(FixedClock(now)),
        folioharbor_domain::imports::blob::DedupScope::Instance,
        u64::MAX,
    ));
    let first = tokio::spawn({
        let service = service.clone();
        async move {
            service
                .receive_upload(ReceiveUploadRequest {
                    actor,
                    request_id: RequestId::new(),
                    library_id: library,
                    upload_id: upload,
                    traceparent: None,
                    bytes: Box::pin(futures_util::stream::pending()),
                })
                .await
        }
    });
    for _ in 0..100 {
        if staging_file_count(directory.path()) == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        !first.is_finished(),
        "first receipt must still await body data"
    );
    assert_eq!(staging_file_count(directory.path()), 1);
    let loser = service
        .receive_upload(request(actor, library, upload))
        .await
        .expect_err("second receipt must lose the database claim");
    assert!(matches!(
        loser,
        AppError::Conflict {
            code: "upload_state_conflict"
        }
    ));
    assert_eq!(staging_file_count(directory.path()), 1);
    first.abort();
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

fn staging_file_count(root: &std::path::Path) -> usize {
    std::fs::read_dir(root.join("staging"))
        .map(|entries| entries.filter_map(Result::ok).count())
        .unwrap_or(0)
}

fn request(actor: UserId, library: LibraryId, upload: UploadId) -> ReceiveUploadRequest {
    ReceiveUploadRequest {
        actor,
        request_id: RequestId::new(),
        library_id: library,
        upload_id: upload,
        traceparent: None,
        bytes: Box::pin(futures_util::stream::iter([Ok(Bytes::from_static(
            b"book",
        ))])),
    }
}
