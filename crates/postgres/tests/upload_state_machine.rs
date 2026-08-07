#![allow(clippy::expect_used, clippy::too_many_lines)]

use folioharbor_application::ports::{
    AuthorizedUploadTransition, BeginUploadReceipt, BlobDisposition, ClaimUploadCleanup,
    CreateUploadRecord, ExpireUploads, FinalizeUploadReceipt, JobRepository,
    PrepareUploadPromotion, RecordPromotionDisposition, UploadReceiptAttempt, UploadRepository,
    UploadRepositoryError, WorkerUploadTransition,
};
use folioharbor_domain::{
    id::{JobId, LibraryId, RequestId, UploadId, UserId},
    imports::{
        blob::Sha256Digest,
        job::{JobInput, JobKind},
        quota::ByteCount,
        upload::UploadState,
    },
};
use folioharbor_postgres::{
    DatabaseContext, PgJobRepository, PgPools, PgTransactionContext, PgUploadRepository,
    run_migrations,
};
use folioharbor_test_support::postgres::TestPostgres;
use time::{Duration, OffsetDateTime};

fn instance_key(size: u64) -> String {
    format!("blob:instance-v1:{}:{size}", "0".repeat(64))
}

fn disabled_key(upload: UploadId, size: u64) -> String {
    format!(
        "blob:upload-{}:{}:{size}",
        upload.as_uuid().simple(),
        "0".repeat(64)
    )
}

const TEST_DIGEST: Sha256Digest = Sha256Digest::from_bytes([0; 32]);

async fn begin_receipt(
    repository: &PgUploadRepository,
    actor: UserId,
    library_id: LibraryId,
    upload_id: UploadId,
    from: UploadState,
    now: OffsetDateTime,
) -> anyhow::Result<UploadReceiptAttempt> {
    repository
        .begin_receipt(BeginUploadReceipt {
            actor,
            library_id,
            upload_id,
            from,
            request_id: RequestId::new(),
            now,
        })
        .await?
        .ok_or_else(|| anyhow::anyhow!("receipt was not claimed"))
}

#[tokio::test]
async fn authorized_creation_atomically_creates_upload_and_quota_reservation() -> anyhow::Result<()>
{
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_migrations(&pools.owner).await?;
    let now = OffsetDateTime::now_utc();
    let user = UserId::new();
    let library = LibraryId::new();
    sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,$2,$2,'verified',$3,$3)").bind(user.as_uuid()).bind("upload@test.invalid").bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,created_at,updated_at) VALUES($1,'Uploads',$2,$2)").bind(library.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.library_memberships(library_id,user_id,role_code,status,joined_at) VALUES($1,$2,'editor','active',$3)").bind(library.as_uuid()).bind(user.as_uuid()).bind(now).execute(&pools.owner).await?;
    let upload = UploadId::new();
    let repository = PgUploadRepository::new(pools.api.clone());
    let created = repository
        .create_authorized(CreateUploadRecord {
            upload_id: upload,
            library_id: library,
            actor: user,
            request_id: RequestId::new(),
            file_name: "book.epub".into(),
            media_type: "application/epub+zip".into(),
            declared_bytes: ByteCount::new(42),
            dedup_scope: folioharbor_domain::imports::blob::DedupScope::Instance,
            expires_at: now + Duration::hours(24),
            now,
        })
        .await?;
    assert_eq!(created.state, UploadState::Created);
    let counts: (i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM folioharbor.upload_sessions WHERE upload_id=$1),(SELECT count(*) FROM folioharbor.quota_reservations WHERE upload_id=$1 AND state='active')").bind(upload.as_uuid()).fetch_one(&pools.owner).await?;
    assert_eq!(counts, (1, 1));
    let reader = UserId::new();
    sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,$2,$2,'verified',$3,$3)").bind(reader.as_uuid()).bind("reader-upload@test.invalid").bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.library_memberships(library_id,user_id,role_code,status,joined_at) VALUES($1,$2,'reader','active',$3)").bind(library.as_uuid()).bind(reader.as_uuid()).bind(now).execute(&pools.owner).await?;
    assert!(
        repository
            .find_authorized(reader, library, upload, RequestId::new())
            .await?
            .is_none()
    );
    let denied = repository
        .create_authorized(CreateUploadRecord {
            upload_id: UploadId::new(),
            library_id: library,
            actor: reader,
            request_id: RequestId::new(),
            file_name: "reader.epub".into(),
            media_type: "application/epub+zip".into(),
            declared_bytes: ByteCount::new(1),
            dedup_scope: folioharbor_domain::imports::blob::DedupScope::Instance,
            expires_at: now + Duration::hours(24),
            now,
        })
        .await
        .expect_err("reader cannot create");
    assert!(matches!(denied, UploadRepositoryError::Forbidden));
    let attempt = begin_receipt(
        &repository,
        user,
        library,
        upload,
        UploadState::Created,
        now,
    )
    .await?;
    assert!(attempt.staging_key.starts_with("staging:"));
    assert_eq!(attempt.staging_key.len(), 72);
    assert!(
        repository
            .begin_receipt(BeginUploadReceipt {
                actor: user,
                library_id: library,
                upload_id: upload,
                from: UploadState::Receiving,
                request_id: RequestId::new(),
                now,
            })
            .await?
            .is_none()
    );
    assert!(
        repository
            .prepare_promotion(PrepareUploadPromotion {
                actor: user,
                library_id: library,
                upload_id: upload,
                attempt_token: attempt.attempt_token.clone(),
                staging_key: attempt.staging_key.clone(),
                final_key: instance_key(42),
                digest: TEST_DIGEST,
                received: ByteCount::new(42),
                request_id: RequestId::new(),
                now,
            })
            .await?
    );
    assert!(
        repository
            .record_promotion_disposition(RecordPromotionDisposition {
                actor: user,
                library_id: library,
                upload_id: upload,
                attempt_token: attempt.attempt_token.clone(),
                staging_key: attempt.staging_key.clone(),
                final_key: instance_key(42),
                disposition: BlobDisposition::Installed,
                request_id: RequestId::new(),
                now,
            })
            .await?
    );
    let installed_shared: (String, bool) = sqlx::query_as(
        "SELECT c.state,u.promotion_owned FROM folioharbor.blob_reachability_candidates c JOIN folioharbor.upload_sessions u ON u.upload_id=c.source_upload_id WHERE c.source_upload_id=$1",
    )
    .bind(upload.as_uuid()).fetch_one(&pools.owner).await?;
    assert_eq!(installed_shared, ("installed_shared".into(), false));
    assert!(
        repository
            .finalize_authorized(FinalizeUploadReceipt {
                actor: user,
                library_id: library,
                upload_id: upload,
                received: ByteCount::new(42),
                storage_key: instance_key(42),
                staging_key: Some(attempt.staging_key.clone()),
                job_id: JobId::new(),
                request_id: RequestId::new(),
                traceparent: None,
                now
            })
            .await?
    );
    assert!(
        repository
            .finalize_authorized(FinalizeUploadReceipt {
                actor: user,
                library_id: library,
                upload_id: upload,
                received: ByteCount::new(42),
                storage_key: instance_key(42),
                staging_key: None,
                job_id: JobId::new(),
                request_id: RequestId::new(),
                traceparent: None,
                now,
            })
            .await?
    );
    let finalized: (String, i64, i64) = sqlx::query_as("SELECT u.state,q.reserved_bytes,(SELECT count(*) FROM folioharbor.background_jobs WHERE idempotency_key=$2) FROM folioharbor.upload_sessions u JOIN folioharbor.quota_reservations q USING(upload_id) WHERE u.upload_id=$1").bind(upload.as_uuid()).bind(format!("import:{}", upload.as_uuid())).fetch_one(&pools.owner).await?;
    assert_eq!(finalized, ("queued".into(), 42, 1));
    assert!(
        PgJobRepository::new(pools.api.clone())
            .enqueue(
                JobId::new(),
                Some(library),
                JobKind::ImportEpub,
                JobInput::upload_v1(upload.as_uuid().to_string()),
                "api-must-not-enqueue",
                now,
            )
            .await
            .is_err()
    );
    assert!(
        !repository
            .transition_authorized(AuthorizedUploadTransition {
                actor: user,
                library_id: library,
                upload_id: upload,
                from: UploadState::Queued,
                to: UploadState::Validating,
                received: ByteCount::new(42),
                attempt_token: None,
                storage_key: Some(instance_key(42)),
                error_code: None,
                request_id: RequestId::new(),
                now
            })
            .await?
    );
    let worker = PgUploadRepository::new(pools.worker.clone());
    assert!(
        worker
            .transition_worker(WorkerUploadTransition {
                library_id: library,
                upload_id: upload,
                from: UploadState::Queued,
                to: UploadState::Validating,
                error_code: None,
                request_id: RequestId::new(),
                now
            })
            .await?
    );
    assert!(
        worker
            .transition_worker(WorkerUploadTransition {
                library_id: library,
                upload_id: upload,
                from: UploadState::Validating,
                to: UploadState::Importing,
                error_code: None,
                request_id: RequestId::new(),
                now
            })
            .await?
    );
    assert!(
        worker
            .transition_worker(WorkerUploadTransition {
                library_id: library,
                upload_id: upload,
                from: UploadState::Importing,
                to: UploadState::Ready,
                error_code: None,
                request_id: RequestId::new(),
                now
            })
            .await?
    );
    assert!(
        !worker
            .transition_worker(WorkerUploadTransition {
                library_id: library,
                upload_id: upload,
                from: UploadState::Importing,
                to: UploadState::Ready,
                error_code: None,
                request_id: RequestId::new(),
                now
            })
            .await?
    );
    let quota:(i64,i64,String)=sqlx::query_as("SELECT l.quota_used_bytes,l.quota_reserved_bytes,q.state FROM folioharbor.libraries l JOIN folioharbor.quota_reservations q USING(library_id) WHERE q.upload_id=$1").bind(upload.as_uuid()).fetch_one(&pools.owner).await?;
    assert_eq!(quota, (0, 42, "active".into()));
    let duplicate = UploadId::new();
    repository
        .create_authorized(CreateUploadRecord {
            upload_id: duplicate,
            library_id: library,
            actor: user,
            request_id: RequestId::new(),
            file_name: "duplicate.epub".into(),
            media_type: "application/epub+zip".into(),
            declared_bytes: ByteCount::new(42),
            dedup_scope: folioharbor_domain::imports::blob::DedupScope::Instance,
            expires_at: now + Duration::hours(24),
            now,
        })
        .await?;
    let duplicate_attempt = begin_receipt(
        &repository,
        user,
        library,
        duplicate,
        UploadState::Created,
        now,
    )
    .await?;
    assert!(
        repository
            .prepare_promotion(PrepareUploadPromotion {
                actor: user,
                library_id: library,
                upload_id: duplicate,
                attempt_token: duplicate_attempt.attempt_token.clone(),
                staging_key: duplicate_attempt.staging_key.clone(),
                final_key: instance_key(42),
                digest: TEST_DIGEST,
                received: ByteCount::new(42),
                request_id: RequestId::new(),
                now,
            })
            .await?
    );
    assert!(
        repository
            .record_promotion_disposition(RecordPromotionDisposition {
                actor: user,
                library_id: library,
                upload_id: duplicate,
                attempt_token: duplicate_attempt.attempt_token.clone(),
                staging_key: duplicate_attempt.staging_key.clone(),
                final_key: instance_key(42),
                disposition: BlobDisposition::Reused,
                request_id: RequestId::new(),
                now,
            })
            .await?
    );
    let reused_candidate_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM folioharbor.blob_reachability_candidates WHERE source_upload_id=$1",
    )
    .bind(duplicate.as_uuid())
    .fetch_one(&pools.owner)
    .await?;
    let reused_owned: bool = sqlx::query_scalar(
        "SELECT promotion_owned FROM folioharbor.upload_sessions WHERE upload_id=$1",
    )
    .bind(duplicate.as_uuid())
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!((reused_candidate_count, reused_owned), (0, false));
    assert!(
        repository
            .finalize_authorized(FinalizeUploadReceipt {
                actor: user,
                library_id: library,
                upload_id: duplicate,
                received: ByteCount::new(42),
                storage_key: instance_key(42),
                staging_key: Some(duplicate_attempt.staging_key),
                job_id: JobId::new(),
                request_id: RequestId::new(),
                traceparent: None,
                now,
            })
            .await?
    );
    for (from, to) in [
        (UploadState::Queued, UploadState::Validating),
        (UploadState::Validating, UploadState::Importing),
        (UploadState::Importing, UploadState::Duplicate),
    ] {
        assert!(
            worker
                .transition_worker(WorkerUploadTransition {
                    library_id: library,
                    upload_id: duplicate,
                    from,
                    to,
                    error_code: None,
                    request_id: RequestId::new(),
                    now,
                })
                .await?
        );
    }
    let duplicate_quota: (i64, i64, String) = sqlx::query_as("SELECT l.quota_used_bytes,l.quota_reserved_bytes,q.state FROM folioharbor.libraries l JOIN folioharbor.quota_reservations q USING(library_id) WHERE q.upload_id=$1").bind(duplicate.as_uuid()).fetch_one(&pools.owner).await?;
    assert_eq!(duplicate_quota, (0, 84, "active".into()));
    let legacy = UploadId::new();
    repository
        .create_authorized(CreateUploadRecord {
            upload_id: legacy,
            library_id: library,
            actor: user,
            request_id: RequestId::new(),
            file_name: "legacy.epub".into(),
            media_type: "application/epub+zip".into(),
            declared_bytes: ByteCount::new(42),
            dedup_scope: folioharbor_domain::imports::blob::DedupScope::Instance,
            expires_at: now + Duration::hours(24),
            now,
        })
        .await?;
    sqlx::query("UPDATE folioharbor.upload_sessions SET state='received',received_bytes=42,storage_key='blobs:legacy' WHERE upload_id=$1")
        .bind(legacy.as_uuid())
        .execute(&pools.owner)
        .await?;
    assert!(
        !repository
            .finalize_authorized(FinalizeUploadReceipt {
                actor: user,
                library_id: library,
                upload_id: legacy,
                received: ByteCount::new(42),
                storage_key: "blobs:mismatch".into(),
                staging_key: None,
                job_id: JobId::new(),
                request_id: RequestId::new(),
                traceparent: None,
                now,
            })
            .await?
    );
    let legacy_jobs: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM folioharbor.background_jobs WHERE idempotency_key=$1",
    )
    .bind(format!("import:{}", legacy.as_uuid()))
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(legacy_jobs, 0);
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn failed_receipts_release_once_and_retry_the_same_upload_id() -> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_migrations(&pools.owner).await?;
    let now = OffsetDateTime::now_utc();
    let actor = UserId::new();
    let library = LibraryId::new();
    let upload = UploadId::new();
    sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,$2,$2,'verified',$3,$3)").bind(actor.as_uuid()).bind("retry@test.invalid").bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,created_at,updated_at) VALUES($1,'Retry',$2,$2)").bind(library.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.library_memberships(library_id,user_id,role_code,status,joined_at) VALUES($1,$2,'editor','active',$3)").bind(library.as_uuid()).bind(actor.as_uuid()).bind(now).execute(&pools.owner).await?;
    let repository = PgUploadRepository::new(pools.api.clone());
    repository
        .create_authorized(CreateUploadRecord {
            upload_id: upload,
            library_id: library,
            actor,
            request_id: RequestId::new(),
            file_name: "retry.epub".into(),
            media_type: "application/octet-stream".into(),
            declared_bytes: ByteCount::new(50),
            dedup_scope: folioharbor_domain::imports::blob::DedupScope::Instance,
            expires_at: now + Duration::hours(24),
            now,
        })
        .await?;
    let first_attempt = begin_receipt(
        &repository,
        actor,
        library,
        upload,
        UploadState::Created,
        now,
    )
    .await?;
    assert!(
        repository
            .transition_authorized(AuthorizedUploadTransition {
                actor,
                library_id: library,
                upload_id: upload,
                from: UploadState::Receiving,
                to: UploadState::Failed,
                received: ByteCount::new(12),
                attempt_token: Some(first_attempt.attempt_token.clone()),
                storage_key: Some(first_attempt.staging_key.clone()),
                error_code: Some("upload_interrupted".into()),
                request_id: RequestId::new(),
                now
            })
            .await?
    );
    assert!(
        !repository
            .transition_authorized(AuthorizedUploadTransition {
                actor,
                library_id: library,
                upload_id: upload,
                from: UploadState::Receiving,
                to: UploadState::Failed,
                received: ByteCount::new(12),
                attempt_token: Some(first_attempt.attempt_token.clone()),
                storage_key: Some(first_attempt.staging_key.clone()),
                error_code: Some("upload_interrupted".into()),
                request_id: RequestId::new(),
                now
            })
            .await?
    );
    let after_failure:(i64,String)=sqlx::query_as("SELECT l.quota_reserved_bytes,q.state FROM folioharbor.libraries l JOIN folioharbor.quota_reservations q USING(library_id) WHERE q.upload_id=$1").bind(upload.as_uuid()).fetch_one(&pools.owner).await?;
    assert_eq!(after_failure, (0, "released".into()));
    let second_attempt = begin_receipt(
        &repository,
        actor,
        library,
        upload,
        UploadState::Failed,
        now,
    )
    .await?;
    let after_retry:(i64,String)=sqlx::query_as("SELECT l.quota_reserved_bytes,q.state FROM folioharbor.libraries l JOIN folioharbor.quota_reservations q USING(library_id) WHERE q.upload_id=$1").bind(upload.as_uuid()).fetch_one(&pools.owner).await?;
    assert_eq!(after_retry, (50, "active".into()));
    assert!(
        !repository
            .transition_authorized(AuthorizedUploadTransition {
                actor,
                library_id: library,
                upload_id: upload,
                from: UploadState::Receiving,
                to: UploadState::Failed,
                received: ByteCount::new(12),
                attempt_token: Some(first_attempt.attempt_token.clone()),
                storage_key: Some(first_attempt.staging_key.clone()),
                error_code: Some("upload_interrupted".into()),
                request_id: RequestId::new(),
                now,
            })
            .await?
    );
    let after_stale_abort: (i64, String) = sqlx::query_as("SELECT l.quota_reserved_bytes,q.state FROM folioharbor.libraries l JOIN folioharbor.quota_reservations q USING(library_id) WHERE q.upload_id=$1")
        .bind(upload.as_uuid()).fetch_one(&pools.owner).await?;
    assert_eq!(after_stale_abort, (50, "active".into()));
    assert_ne!(first_attempt.staging_key, second_attempt.staging_key);
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn security_definer_rejects_forged_storage_keys_identity_and_ownership() -> anyhow::Result<()>
{
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_migrations(&pools.owner).await?;
    let now = OffsetDateTime::now_utc();
    let actor = UserId::new();
    let library = LibraryId::new();
    let upload = UploadId::new();
    let other_upload = UploadId::new();
    sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,$2,$2,'verified',$3,$3)")
        .bind(actor.as_uuid()).bind("storage-forgery@test.invalid").bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,created_at,updated_at) VALUES($1,'Storage forgery',$2,$2)")
        .bind(library.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.library_memberships(library_id,user_id,role_code,status,joined_at) VALUES($1,$2,'editor','active',$3)")
        .bind(library.as_uuid()).bind(actor.as_uuid()).bind(now).execute(&pools.owner).await?;
    let repository = PgUploadRepository::new(pools.api.clone());
    repository
        .create_authorized(CreateUploadRecord {
            upload_id: upload,
            library_id: library,
            actor,
            request_id: RequestId::new(),
            file_name: "forgery.epub".into(),
            media_type: "application/epub+zip".into(),
            declared_bytes: ByteCount::new(4),
            dedup_scope: folioharbor_domain::imports::blob::DedupScope::Instance,
            expires_at: now + Duration::hours(1),
            now,
        })
        .await?;
    repository
        .create_authorized(CreateUploadRecord {
            upload_id: other_upload,
            library_id: library,
            actor,
            request_id: RequestId::new(),
            file_name: "other.epub".into(),
            media_type: "application/epub+zip".into(),
            declared_bytes: ByteCount::new(4),
            dedup_scope: folioharbor_domain::imports::blob::DedupScope::Instance,
            expires_at: now + Duration::hours(1),
            now,
        })
        .await?;
    let attempt = begin_receipt(
        &repository,
        actor,
        library,
        upload,
        UploadState::Created,
        now,
    )
    .await?;
    let other_attempt = begin_receipt(
        &repository,
        actor,
        library,
        other_upload,
        UploadState::Created,
        now,
    )
    .await?;
    assert_ne!(attempt.staging_key, other_attempt.staging_key);

    let mut transaction = pools.api.begin().await?;
    PgTransactionContext::apply(
        &mut transaction,
        &DatabaseContext::api(actor, library, RequestId::new()),
    )
    .await?;
    sqlx::query("SELECT folioharbor.upload_record_orphan_cleanup_authorized($1,$2,$3,$4,$5,$6)")
        .bind(upload.as_uuid())
        .bind(library.as_uuid())
        .bind(actor.as_uuid())
        .bind(attempt.attempt_token.parse::<uuid::Uuid>()?)
        .bind("blob:instance-v1:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:4")
        .bind(now)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("SELECT folioharbor.upload_record_orphan_cleanup_authorized($1,$2,$3,$4,$5,$6)")
        .bind(upload.as_uuid())
        .bind(library.as_uuid())
        .bind(actor.as_uuid())
        .bind(other_attempt.attempt_token.parse::<uuid::Uuid>()?)
        .bind(&other_attempt.staging_key)
        .bind(now)
        .execute(&mut *transaction)
        .await?;
    let forged = sqlx::query_scalar::<_, bool>(
        "SELECT folioharbor.upload_prepare_promotion_authorized($1,$2,$3,$4,$5,$6,$7,$8,$9)",
    )
    .bind(upload.as_uuid())
    .bind(library.as_uuid())
    .bind(actor.as_uuid())
    .bind(attempt.attempt_token.parse::<uuid::Uuid>()?)
    .bind(&attempt.staging_key)
    .bind("blob:library-forged:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb:4")
    .bind(vec![0_u8; 32])
    .bind(4_i64)
    .bind(now)
    .fetch_one(&mut *transaction)
    .await?;
    transaction.commit().await?;
    assert!(!forged);
    let mut ownership_forgery = pools.api.begin().await?;
    PgTransactionContext::apply(
        &mut ownership_forgery,
        &DatabaseContext::api(actor, library, RequestId::new()),
    )
    .await?;
    assert!(
        sqlx::query(
            "UPDATE folioharbor.upload_sessions SET promotion_owned=true WHERE upload_id=$1"
        )
        .bind(upload.as_uuid())
        .execute(&mut *ownership_forgery)
        .await
        .is_err()
    );
    ownership_forgery.rollback().await?;
    let unsafe_targets: i64 =
        sqlx::query_scalar("SELECT count(*) FROM folioharbor.upload_cleanups WHERE upload_id=$1")
            .bind(upload.as_uuid())
            .fetch_one(&pools.owner)
            .await?;
    assert_eq!(unsafe_targets, 0);
    let promotion: (Option<String>, bool) = sqlx::query_as(
        "SELECT promotion_key,promotion_owned FROM folioharbor.upload_sessions WHERE upload_id=$1",
    )
    .bind(upload.as_uuid())
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(promotion, (None, false));
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn worker_expiry_releases_created_upload_reservations_exactly_once() -> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_migrations(&pools.owner).await?;
    let now = OffsetDateTime::now_utc();
    let actor = UserId::new();
    let library = LibraryId::new();
    let upload = UploadId::new();
    sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,$2,$2,'verified',$3,$3)").bind(actor.as_uuid()).bind("expiry@test.invalid").bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,created_at,updated_at) VALUES($1,'Expiry',$2,$2)").bind(library.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.library_memberships(library_id,user_id,role_code,status,joined_at) VALUES($1,$2,'editor','active',$3)").bind(library.as_uuid()).bind(actor.as_uuid()).bind(now).execute(&pools.owner).await?;
    PgUploadRepository::new(pools.api.clone())
        .create_authorized(CreateUploadRecord {
            upload_id: upload,
            library_id: library,
            actor,
            request_id: RequestId::new(),
            file_name: "expired.epub".into(),
            media_type: "application/epub+zip".into(),
            declared_bytes: ByteCount::new(64),
            dedup_scope: folioharbor_domain::imports::blob::DedupScope::Instance,
            expires_at: now - Duration::seconds(1),
            now: now - Duration::hours(25),
        })
        .await?;
    let receiving = UploadId::new();
    PgUploadRepository::new(pools.api.clone())
        .create_authorized(CreateUploadRecord {
            upload_id: receiving,
            library_id: library,
            actor,
            request_id: RequestId::new(),
            file_name: "abandoned.epub".into(),
            media_type: "application/epub+zip".into(),
            declared_bytes: ByteCount::new(32),
            dedup_scope: folioharbor_domain::imports::blob::DedupScope::Disabled,
            expires_at: now - Duration::seconds(1),
            now: now - Duration::hours(25),
        })
        .await?;
    let api_repository = PgUploadRepository::new(pools.api.clone());
    let receiving_attempt = begin_receipt(
        &api_repository,
        actor,
        library,
        receiving,
        UploadState::Created,
        now - Duration::hours(2),
    )
    .await?;
    let receipt_time = now - Duration::hours(2) + Duration::minutes(1);
    assert!(
        PgUploadRepository::new(pools.api.clone())
            .prepare_promotion(PrepareUploadPromotion {
                actor,
                library_id: library,
                upload_id: receiving,
                attempt_token: receiving_attempt.attempt_token.clone(),
                staging_key: receiving_attempt.staging_key.clone(),
                final_key: disabled_key(receiving, 0),
                digest: TEST_DIGEST,
                received: ByteCount::new(0),
                request_id: RequestId::new(),
                now: receipt_time,
            })
            .await?
    );
    assert!(
        PgUploadRepository::new(pools.api.clone())
            .record_promotion_disposition(RecordPromotionDisposition {
                actor,
                library_id: library,
                upload_id: receiving,
                attempt_token: receiving_attempt.attempt_token.clone(),
                staging_key: receiving_attempt.staging_key.clone(),
                final_key: disabled_key(receiving, 0),
                disposition: BlobDisposition::Installed,
                request_id: RequestId::new(),
                now: receipt_time,
            })
            .await?
    );
    let expiry = PgUploadRepository::new(pools.worker.clone());
    for expected in [2_u64, 0] {
        assert_eq!(
            expiry
                .expire_worker(ExpireUploads {
                    now,
                    limit: 10,
                    request_id: RequestId::new(),
                })
                .await?,
            expected
        );
    }
    let state: (String, i64, String) = sqlx::query_as("SELECT u.state,l.quota_reserved_bytes,q.state FROM folioharbor.upload_sessions u JOIN folioharbor.libraries l USING(library_id) JOIN folioharbor.quota_reservations q USING(upload_id) WHERE u.upload_id=$1").bind(upload.as_uuid()).fetch_one(&pools.owner).await?;
    assert_eq!(state, ("expired".into(), 0, "released".into()));
    let abandoned: (String, String) = sqlx::query_as(
        "SELECT state,error_code FROM folioharbor.upload_sessions WHERE upload_id=$1",
    )
    .bind(receiving.as_uuid())
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(abandoned, ("failed".into(), "receipt_expired".into()));
    let cleanup: (String, String, bool) = sqlx::query_as(
        "SELECT state,staging_key,final_owned FROM folioharbor.upload_cleanups WHERE upload_id=$1",
    )
    .bind(receiving.as_uuid())
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(
        cleanup,
        (
            "pending".into(),
            receiving_attempt.staging_key.clone(),
            true
        )
    );
    assert!(
        PgUploadRepository::new(pools.api.clone())
            .begin_receipt(BeginUploadReceipt {
                actor,
                library_id: library,
                upload_id: receiving,
                from: UploadState::Failed,
                request_id: RequestId::new(),
                now,
            })
            .await?
            .is_none()
    );
    let cleanup_guard = expiry
        .claim_cleanup(ClaimUploadCleanup {
            owner: "expiry-worker".into(),
            now,
            request_id: RequestId::new(),
        })
        .await?
        .expect("cleanup claim");
    assert!(
        PgUploadRepository::new(pools.api.clone())
            .begin_receipt(BeginUploadReceipt {
                actor,
                library_id: library,
                upload_id: receiving,
                from: UploadState::Failed,
                request_id: RequestId::new(),
                now: now + Duration::hours(1),
            })
            .await?
            .is_none()
    );
    let late_worker_pool = sqlx::PgPool::connect(&database.worker_url()?).await?;
    assert!(
        PgUploadRepository::new(late_worker_pool.clone())
            .claim_cleanup(ClaimUploadCleanup {
                owner: "late-worker".into(),
                now: now + Duration::hours(1),
                request_id: RequestId::new(),
            })
            .await?
            .is_none()
    );
    late_worker_pool.close().await;
    assert!(cleanup_guard.complete(now + Duration::hours(1)).await?);
    assert!(
        PgUploadRepository::new(pools.api.clone())
            .begin_receipt(BeginUploadReceipt {
                actor,
                library_id: library,
                upload_id: receiving,
                from: UploadState::Failed,
                request_id: RequestId::new(),
                now,
            })
            .await?
            .is_some()
    );
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}
