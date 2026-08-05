#![allow(clippy::expect_used, clippy::too_many_lines)]

use folioharbor_application::ports::{
    AuthorizedUploadTransition, CreateUploadRecord, UploadRepository, UploadRepositoryError,
    WorkerUploadTransition,
};
use folioharbor_domain::{
    id::{LibraryId, RequestId, UploadId, UserId},
    imports::{quota::ByteCount, upload::UploadState},
};
use folioharbor_postgres::{PgPools, PgUploadRepository, run_migrations};
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
        repository
            .transition_authorized(AuthorizedUploadTransition {
                actor: user,
                library_id: library,
                upload_id: upload,
                from: UploadState::Receiving,
                to: UploadState::Received,
                received: ByteCount::new(42),
                storage_key: Some("blobs:worker".into()),
                error_code: None,
                request_id: RequestId::new(),
                now
            })
            .await?
    );
    assert!(
        repository
            .transition_authorized(AuthorizedUploadTransition {
                actor: user,
                library_id: library,
                upload_id: upload,
                from: UploadState::Received,
                to: UploadState::Queued,
                received: ByteCount::new(42),
                storage_key: Some("blobs:worker".into()),
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
    assert_eq!(quota, (42, 0, "consumed".into()));
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
                storage_key: None,
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
                storage_key: None,
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
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}
