#![allow(clippy::expect_used, clippy::too_many_lines)]

use folioharbor_application::ports::{
    AuthorizedUploadTransition, CreateUploadRecord, ExpireUploads, FinalizeUploadReceipt,
    JobRepository, LeaseUploadCleanups, PrepareUploadPromotion, UploadRepository,
    UploadRepositoryError, WorkerUploadTransition,
};
use folioharbor_domain::{
    id::{JobId, LibraryId, RequestId, UploadId, UserId},
    imports::{
        job::{JobInput, JobKind},
        quota::ByteCount,
        upload::UploadState,
    },
};
use folioharbor_postgres::{PgJobRepository, PgPools, PgUploadRepository, run_migrations};
use folioharbor_test_support::postgres::TestPostgres;
use time::{Duration, OffsetDateTime};

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
            expires_at: now + Duration::hours(24),
            now,
        })
        .await
        .expect_err("reader cannot create");
    assert!(matches!(denied, UploadRepositoryError::Forbidden));
    assert!(
        repository
            .transition_authorized(AuthorizedUploadTransition {
                actor: user,
                library_id: library,
                upload_id: upload,
                from: UploadState::Created,
                to: UploadState::Receiving,
                received: ByteCount::new(0),
                storage_key: Some("staging:worker".into()),
                error_code: None,
                request_id: RequestId::new(),
                now
            })
            .await?
    );
    assert!(
        !repository
            .transition_authorized(AuthorizedUploadTransition {
                actor: user,
                library_id: library,
                upload_id: upload,
                from: UploadState::Receiving,
                to: UploadState::Receiving,
                received: ByteCount::new(0),
                storage_key: Some("staging:concurrent".into()),
                error_code: None,
                request_id: RequestId::new(),
                now,
            })
            .await?
    );
    assert!(
        repository
            .prepare_promotion(PrepareUploadPromotion {
                actor: user,
                library_id: library,
                upload_id: upload,
                staging_key: "staging:worker".into(),
                final_key: "blobs:worker".into(),
                final_owned: true,
                request_id: RequestId::new(),
                now,
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
                storage_key: "blobs:worker".into(),
                staging_key: Some("staging:worker".into()),
                job_id: JobId::new(),
                request_id: RequestId::new(),
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
                storage_key: "blobs:worker".into(),
                staging_key: None,
                job_id: JobId::new(),
                request_id: RequestId::new(),
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
                library,
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
                storage_key: Some("blobs:worker".into()),
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
            expires_at: now + Duration::hours(24),
            now,
        })
        .await?;
    assert!(
        repository
            .transition_authorized(AuthorizedUploadTransition {
                actor: user,
                library_id: library,
                upload_id: duplicate,
                from: UploadState::Created,
                to: UploadState::Receiving,
                received: ByteCount::new(0),
                storage_key: Some("staging:duplicate".into()),
                error_code: None,
                request_id: RequestId::new(),
                now,
            })
            .await?
    );
    assert!(
        repository
            .prepare_promotion(PrepareUploadPromotion {
                actor: user,
                library_id: library,
                upload_id: duplicate,
                staging_key: "staging:duplicate".into(),
                final_key: "blobs:duplicate".into(),
                final_owned: true,
                request_id: RequestId::new(),
                now,
            })
            .await?
    );
    assert!(
        repository
            .finalize_authorized(FinalizeUploadReceipt {
                actor: user,
                library_id: library,
                upload_id: duplicate,
                received: ByteCount::new(42),
                storage_key: "blobs:duplicate".into(),
                staging_key: Some("staging:duplicate".into()),
                job_id: JobId::new(),
                request_id: RequestId::new(),
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
            expires_at: now + Duration::hours(24),
            now,
        })
        .await?;
    assert!(
        repository
            .transition_authorized(AuthorizedUploadTransition {
                actor,
                library_id: library,
                upload_id: upload,
                from: UploadState::Created,
                to: UploadState::Receiving,
                received: ByteCount::new(0),
                storage_key: Some("staging:first".into()),
                error_code: None,
                request_id: RequestId::new(),
                now
            })
            .await?
    );
    assert!(
        repository
            .transition_authorized(AuthorizedUploadTransition {
                actor,
                library_id: library,
                upload_id: upload,
                from: UploadState::Receiving,
                to: UploadState::Failed,
                received: ByteCount::new(12),
                storage_key: Some("staging:first".into()),
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
                storage_key: Some("staging:first".into()),
                error_code: Some("upload_interrupted".into()),
                request_id: RequestId::new(),
                now
            })
            .await?
    );
    let after_failure:(i64,String)=sqlx::query_as("SELECT l.quota_reserved_bytes,q.state FROM folioharbor.libraries l JOIN folioharbor.quota_reservations q USING(library_id) WHERE q.upload_id=$1").bind(upload.as_uuid()).fetch_one(&pools.owner).await?;
    assert_eq!(after_failure, (0, "released".into()));
    assert!(
        repository
            .transition_authorized(AuthorizedUploadTransition {
                actor,
                library_id: library,
                upload_id: upload,
                from: UploadState::Failed,
                to: UploadState::Receiving,
                received: ByteCount::new(0),
                storage_key: Some("staging:retry".into()),
                error_code: None,
                request_id: RequestId::new(),
                now
            })
            .await?
    );
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
                storage_key: Some("staging:first".into()),
                error_code: Some("upload_interrupted".into()),
                request_id: RequestId::new(),
                now,
            })
            .await?
    );
    let after_stale_abort: (i64, String) = sqlx::query_as("SELECT l.quota_reserved_bytes,q.state FROM folioharbor.libraries l JOIN folioharbor.quota_reservations q USING(library_id) WHERE q.upload_id=$1")
        .bind(upload.as_uuid()).fetch_one(&pools.owner).await?;
    assert_eq!(after_stale_abort, (50, "active".into()));
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
            expires_at: now - Duration::seconds(1),
            now: now - Duration::hours(25),
        })
        .await?;
    assert!(
        PgUploadRepository::new(pools.api.clone())
            .transition_authorized(AuthorizedUploadTransition {
                actor,
                library_id: library,
                upload_id: receiving,
                from: UploadState::Created,
                to: UploadState::Receiving,
                received: ByteCount::new(0),
                storage_key: Some("staging:abandoned".into()),
                error_code: None,
                request_id: RequestId::new(),
                now: now - Duration::hours(2),
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
        ("pending".into(), "staging:abandoned".into(), false)
    );
    assert!(
        !PgUploadRepository::new(pools.api.clone())
            .transition_authorized(AuthorizedUploadTransition {
                actor,
                library_id: library,
                upload_id: receiving,
                from: UploadState::Failed,
                to: UploadState::Receiving,
                received: ByteCount::new(0),
                storage_key: Some("staging:too-early".into()),
                error_code: None,
                request_id: RequestId::new(),
                now,
            })
            .await?
    );
    let leased = expiry
        .lease_cleanups(LeaseUploadCleanups {
            owner: "expiry-worker".into(),
            now,
            lease_for: Duration::minutes(1),
            limit: 10,
            request_id: RequestId::new(),
        })
        .await?;
    assert_eq!(leased.len(), 1);
    assert!(
        expiry
            .complete_cleanup(
                receiving,
                &leased[0].attempt_token,
                "expiry-worker",
                now,
                RequestId::new(),
            )
            .await?
    );
    assert!(
        PgUploadRepository::new(pools.api.clone())
            .transition_authorized(AuthorizedUploadTransition {
                actor,
                library_id: library,
                upload_id: receiving,
                from: UploadState::Failed,
                to: UploadState::Receiving,
                received: ByteCount::new(0),
                storage_key: Some("staging:retry".into()),
                error_code: None,
                request_id: RequestId::new(),
                now,
            })
            .await?
    );
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}
