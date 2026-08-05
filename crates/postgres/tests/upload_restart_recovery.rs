#![allow(clippy::expect_used, clippy::too_many_lines)]

use async_trait::async_trait;
use bytes::Bytes;
use folioharbor_application::{
    imports::{ReceiveUploadRequest, UploadApi, UploadService},
    ports::{
        AuthorizedUploadTransition, Clock, CreateUploadRecord, ExpireUploads,
        FinalizeUploadReceipt, UploadRepository, UploadRepositoryError, WorkerUploadTransition,
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
    async fn finalize_authorized(
        &self,
        receipt: FinalizeUploadReceipt,
    ) -> Result<bool, UploadRepositoryError> {
        if self.fail.swap(false, Ordering::SeqCst) {
            return Err(UploadRepositoryError::Persistence);
        }
        self.inner.finalize_authorized(receipt).await
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
    );
    failing
        .receive_upload(request(actor, library, upload))
        .await
        .expect_err("injected finalize cut");
    drop(failing);
    let restarted = UploadService::new(
        Arc::new(uploads),
        Arc::new(PgAuthorizationRepository::new(pools.api.clone())),
        blobs,
        Arc::new(FixedClock(now + Duration::seconds(1))),
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

fn request(actor: UserId, library: LibraryId, upload: UploadId) -> ReceiveUploadRequest {
    ReceiveUploadRequest {
        actor,
        request_id: RequestId::new(),
        library_id: library,
        upload_id: upload,
        bytes: Box::pin(futures_util::stream::iter([Ok(Bytes::from_static(
            b"book",
        ))])),
    }
}
