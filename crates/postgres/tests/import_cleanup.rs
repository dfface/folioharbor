#![allow(clippy::expect_used, clippy::too_many_lines)]

use bytes::Bytes;
use folioharbor_application::{
    imports::{CleanupCursor, ReceiveUploadRequest, UploadApi, UploadService},
    ports::{
        BeginUploadReceipt, BlobStore, Clock, CreateUploadRecord, ImportCleanupRepository,
        ImportReconciliation, ImportRepository, ImportWork, PrepareUploadPromotion,
        UploadRepository, UploadRepositoryError,
    },
};
use folioharbor_domain::{
    id::{BlobId, LibraryId, RequestId, UploadId, UserId},
    imports::{
        blob::{DedupScope, Sha256Digest, StorageKey},
        job::JobKind,
        quota::ByteCount,
        upload::UploadState,
    },
    time::OffsetDateTime,
};
use folioharbor_postgres::{
    PgAuthorizationRepository, PgImportCleanupRepository, PgImportRepository, PgPools,
    PgUploadRepository, run_migrations,
};
use folioharbor_storage_local::LocalBlobStore;
use folioharbor_test_support::postgres::TestPostgres;
use sha2::{Digest as _, Sha256};
use std::sync::Arc;
use time::Duration;

#[derive(Clone, Copy)]
struct FixedClock(OffsetDateTime);

impl Clock for FixedClock {
    fn now(&self) -> OffsetDateTime {
        self.0
    }
}

#[derive(sqlx::FromRow)]
struct LocationLifecycleRow {
    state: String,
    purge_pending_at: Option<OffsetDateTime>,
    purge_after: Option<OffsetDateTime>,
    purged_at: Option<OffsetDateTime>,
    purge_lease_owner: Option<String>,
    purge_lease_token: Option<uuid::Uuid>,
    purge_lease_expires_at: Option<OffsetDateTime>,
}

#[tokio::test]
async fn normal_upload_promotes_and_reconciles_a_purged_content_addressed_blob()
-> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_migrations(&pools.owner).await?;
    let directory = tempfile::tempdir()?;
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000)?;
    let actor = UserId::new();
    let library = LibraryId::new();
    let upload = UploadId::new();
    let blob = BlobId::new();
    let content = Bytes::from_static(b"book");
    let digest = Sha256Digest::from_bytes(Sha256::digest(&content).into());
    let storage_key = format!("blob:instance-v1:{}:4", digest.to_hex());
    sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,$2,$2,'verified',$3,$3)")
        .bind(actor.as_uuid())
        .bind(format!("{}@normal-revive.test", actor.as_uuid()))
        .bind(now)
        .execute(&pools.owner)
        .await?;
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,created_at,updated_at) VALUES($1,'Normal revive',$2,$2)")
        .bind(library.as_uuid())
        .bind(now)
        .execute(&pools.owner)
        .await?;
    sqlx::query("INSERT INTO folioharbor.library_memberships(library_id,user_id,role_code,status,joined_at) VALUES($1,$2,'editor','active',$3)")
        .bind(library.as_uuid())
        .bind(actor.as_uuid())
        .bind(now)
        .execute(&pools.owner)
        .await?;
    sqlx::query("INSERT INTO folioharbor.blobs(blob_id,storage_namespace,sha256,byte_size,created_at) VALUES($1,'instance-v1',$2,4,$3)")
        .bind(blob.as_uuid())
        .bind(digest.as_bytes().to_vec())
        .bind(now - Duration::days(2))
        .execute(&pools.owner)
        .await?;
    sqlx::query("INSERT INTO folioharbor.blob_locations(blob_id,storage_key,state,created_at,updated_at,purge_pending_at,purge_after,purged_at) VALUES($1,$2,'purged',$3,$4,$3,$4,$4)")
        .bind(blob.as_uuid())
        .bind(&storage_key)
        .bind(now - Duration::days(2))
        .bind(now - Duration::days(1))
        .execute(&pools.owner)
        .await?;

    let uploads = Arc::new(PgUploadRepository::new(pools.api.clone()));
    uploads
        .create_authorized(CreateUploadRecord {
            upload_id: upload,
            library_id: library,
            actor,
            request_id: RequestId::new(),
            file_name: "same.epub".into(),
            media_type: "application/epub+zip".into(),
            declared_bytes: ByteCount::new(4),
            dedup_scope: DedupScope::Instance,
            expires_at: now + Duration::hours(24),
            now,
        })
        .await?;
    let blob_store = Arc::new(LocalBlobStore::new(directory.path()));
    let service = UploadService::new(
        uploads,
        Arc::new(PgAuthorizationRepository::new(pools.api.clone())),
        blob_store.clone(),
        Arc::new(FixedClock(now)),
        DedupScope::Instance,
    );
    let promoted = service
        .receive_upload(ReceiveUploadRequest {
            actor,
            request_id: RequestId::new(),
            library_id: library,
            upload_id: upload,
            traceparent: None,
            bytes: Box::pin(futures_util::stream::iter([Ok(content)])),
        })
        .await?;
    assert_eq!(promoted.state, UploadState::Queued);
    assert_eq!(
        blob_store
            .read_range(&StorageKey::from_opaque(storage_key.clone()), 0, 4)
            .await?,
        b"book"
    );

    let reconciled = PgImportRepository::new(pools.worker.clone())
        .reconcile(upload, library, RequestId::new(), now)
        .await?;
    assert!(
        matches!(reconciled, ImportReconciliation::Work(ref work) if work.blob_id == blob),
        "normal re-import reuses the content-addressed Blob"
    );
    let location: LocationLifecycleRow = sqlx::query_as("SELECT state,purge_pending_at,purge_after,purged_at,purge_lease_owner,purge_lease_token,purge_lease_expires_at FROM folioharbor.blob_locations WHERE blob_id=$1")
        .bind(blob.as_uuid())
        .fetch_one(&pools.owner)
        .await?;
    assert_eq!(location.state, "ready");
    assert!(location.purge_pending_at.is_none());
    assert!(location.purge_after.is_none());
    assert!(location.purged_at.is_none());
    assert!(location.purge_lease_owner.is_none());
    assert!(location.purge_lease_token.is_none());
    assert!(location.purge_lease_expires_at.is_none());

    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn active_deletion_still_fences_normal_upload_promotion() -> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_migrations(&pools.owner).await?;
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000)?;
    let actor = UserId::new();
    let library = LibraryId::new();
    let upload = UploadId::new();
    let blob = BlobId::new();
    let digest = Sha256Digest::from_bytes(Sha256::digest(b"busy").into());
    let storage_key = format!("blob:instance-v1:{}:4", digest.to_hex());
    sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,$2,$2,'verified',$3,$3)")
        .bind(actor.as_uuid())
        .bind(format!("{}@deleting-fence.test", actor.as_uuid()))
        .bind(now)
        .execute(&pools.owner)
        .await?;
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,created_at,updated_at) VALUES($1,'Deleting fence',$2,$2)")
        .bind(library.as_uuid())
        .bind(now)
        .execute(&pools.owner)
        .await?;
    sqlx::query("INSERT INTO folioharbor.library_memberships(library_id,user_id,role_code,status,joined_at) VALUES($1,$2,'editor','active',$3)")
        .bind(library.as_uuid())
        .bind(actor.as_uuid())
        .bind(now)
        .execute(&pools.owner)
        .await?;
    sqlx::query("INSERT INTO folioharbor.blobs(blob_id,storage_namespace,sha256,byte_size,created_at) VALUES($1,'instance-v1',$2,4,$3)")
        .bind(blob.as_uuid())
        .bind(digest.as_bytes().to_vec())
        .bind(now - Duration::days(2))
        .execute(&pools.owner)
        .await?;
    sqlx::query("INSERT INTO folioharbor.blob_locations(blob_id,storage_key,state,created_at,updated_at,purge_pending_at,purge_after,purge_lease_owner,purge_lease_token,purge_lease_expires_at) VALUES($1,$2,'deleting',$3,$4,$3,$4,'gc-active',$5,$6)")
        .bind(blob.as_uuid())
        .bind(&storage_key)
        .bind(now - Duration::days(1))
        .bind(now)
        .bind(uuid::Uuid::from_u128(1))
        .bind(now + Duration::minutes(5))
        .execute(&pools.owner)
        .await?;

    let uploads = PgUploadRepository::new(pools.api.clone());
    uploads
        .create_authorized(CreateUploadRecord {
            upload_id: upload,
            library_id: library,
            actor,
            request_id: RequestId::new(),
            file_name: "busy.epub".into(),
            media_type: "application/epub+zip".into(),
            declared_bytes: ByteCount::new(4),
            dedup_scope: DedupScope::Instance,
            expires_at: now + Duration::hours(24),
            now,
        })
        .await?;
    let attempt = uploads
        .begin_receipt(BeginUploadReceipt {
            actor,
            library_id: library,
            upload_id: upload,
            from: UploadState::Created,
            request_id: RequestId::new(),
            now,
        })
        .await?
        .expect("receipt is claimed");
    let error = uploads
        .prepare_promotion(PrepareUploadPromotion {
            actor,
            library_id: library,
            upload_id: upload,
            attempt_token: attempt.attempt_token,
            staging_key: attempt.staging_key,
            final_key: storage_key,
            digest,
            received: ByteCount::new(4),
            request_id: RequestId::new(),
            now,
        })
        .await
        .expect_err("an active deletion lease must fence promotion");
    assert!(matches!(error, UploadRepositoryError::Persistence));

    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn reconcile_revives_a_purged_content_addressed_location() -> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_migrations(&pools.owner).await?;
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000)?;
    let actor = UserId::new();
    let library = LibraryId::new();
    let upload = UploadId::new();
    let blob = BlobId::new();
    let digest = vec![0x66_u8; 32];
    let storage_key = format!("blob:instance-v1:{}:61", "66".repeat(32));
    sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,$2,$2,'verified',$3,$3)")
        .bind(actor.as_uuid())
        .bind(format!("{}@revive.test", actor.as_uuid()))
        .bind(now)
        .execute(&pools.owner)
        .await?;
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,quota_reserved_bytes,created_at,updated_at) VALUES($1,'Revive',61,$2,$2)")
        .bind(library.as_uuid())
        .bind(now)
        .execute(&pools.owner)
        .await?;
    sqlx::query("INSERT INTO folioharbor.blobs(blob_id,storage_namespace,sha256,byte_size,created_at) VALUES($1,'instance-v1',$2,61,$3)")
        .bind(blob.as_uuid())
        .bind(&digest)
        .bind(now - Duration::days(2))
        .execute(&pools.owner)
        .await?;
    sqlx::query("INSERT INTO folioharbor.blob_locations(blob_id,storage_key,state,created_at,updated_at,purge_pending_at,purge_after,purged_at) VALUES($1,$2,'purged',$3,$4,$3,$4,$4)")
        .bind(blob.as_uuid())
        .bind(&storage_key)
        .bind(now - Duration::days(2))
        .bind(now - Duration::days(1))
        .execute(&pools.owner)
        .await?;
    sqlx::query("INSERT INTO folioharbor.upload_sessions(upload_id,library_id,created_by,file_name,media_type,declared_bytes,received_bytes,state,dedup_scope,storage_key,sha256,expires_at,created_at,updated_at) VALUES($1,$2,$3,'same.epub','application/epub+zip',61,61,'validating','instance',$4,$5,$6,$7,$7)")
        .bind(upload.as_uuid())
        .bind(library.as_uuid())
        .bind(actor.as_uuid())
        .bind(&storage_key)
        .bind(&digest)
        .bind(now + Duration::hours(1))
        .bind(now)
        .execute(&pools.owner)
        .await?;
    sqlx::query("INSERT INTO folioharbor.quota_reservations(upload_id,library_id,reserved_bytes,expires_at,state) VALUES($1,$2,61,$3,'active')")
        .bind(upload.as_uuid())
        .bind(library.as_uuid())
        .bind(now + Duration::hours(1))
        .execute(&pools.owner)
        .await?;

    let reconciled = PgImportRepository::new(pools.worker.clone())
        .reconcile(upload, library, RequestId::new(), now)
        .await?;
    assert!(
        matches!(reconciled, ImportReconciliation::Work(ref work) if work.blob_id == blob),
        "re-import reuses the content-addressed Blob"
    );
    let location: LocationLifecycleRow = sqlx::query_as("SELECT state,purge_pending_at,purge_after,purged_at,purge_lease_owner,purge_lease_token,purge_lease_expires_at FROM folioharbor.blob_locations WHERE blob_id=$1")
        .bind(blob.as_uuid())
        .fetch_one(&pools.owner)
        .await?;
    assert_eq!(location.state, "ready");
    assert!(location.purge_pending_at.is_none());
    assert!(location.purge_after.is_none());
    assert!(location.purged_at.is_none());
    assert!(location.purge_lease_owner.is_none());
    assert!(location.purge_lease_token.is_none());
    assert!(location.purge_lease_expires_at.is_none());

    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn cleanup_is_bounded_skip_locked_and_persists_its_time_boundary() -> anyhow::Result<()> {
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
    sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,'cleanup@test.invalid','cleanup@test.invalid','verified',$2,$2)")
        .bind(actor.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,quota_reserved_bytes,created_at,updated_at) VALUES($1,'Cleanup',20,$2,$2)")
        .bind(library.as_uuid()).bind(now).execute(&pools.owner).await?;
    let locked = UploadId::new();
    let available = UploadId::new();
    for upload in [locked, available] {
        sqlx::query("INSERT INTO folioharbor.upload_sessions(upload_id,library_id,created_by,file_name,media_type,declared_bytes,state,dedup_scope,expires_at,created_at,updated_at) VALUES($1,$2,$3,'book.epub','application/epub+zip',10,'created','instance',$4,$5,$5)")
            .bind(upload.as_uuid()).bind(library.as_uuid()).bind(actor.as_uuid())
            .bind(now-Duration::hours(1)).bind(now-Duration::hours(2)).execute(&pools.owner).await?;
        sqlx::query("INSERT INTO folioharbor.quota_reservations(upload_id,library_id,reserved_bytes,expires_at,state) VALUES($1,$2,10,$3,'active')")
            .bind(upload.as_uuid()).bind(library.as_uuid()).bind(now-Duration::hours(1)).execute(&pools.owner).await?;
    }
    let mut lock = pools.owner.begin().await?;
    sqlx::query("SELECT 1 FROM folioharbor.upload_sessions WHERE upload_id=$1 FOR UPDATE")
        .bind(locked.as_uuid())
        .execute(&mut *lock)
        .await?;
    let repository = PgImportCleanupRepository::new(pools.worker.clone());
    let cursor = repository
        .begin_pass(JobKind::ExpireUploadsAndReservations, "cleanup-a", now, 1)
        .await?;

    assert_eq!(
        repository
            .expire_abandoned(cursor)
            .await
            .expect("first unlocked batch"),
        1
    );
    assert!(
        repository
            .has_pending(JobKind::ExpireUploadsAndReservations, cursor)
            .await
            .expect("locked eligible row remains visible")
    );
    lock.rollback().await.expect("release deliberate row lock");
    let before_complete: (OffsetDateTime, Option<OffsetDateTime>) = sqlx::query_as(
        "SELECT cursor_at,active_cutoff FROM folioharbor.cleanup_boundaries WHERE cleanup_kind='expire_uploads'",
    )
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(before_complete.1, Some(now));
    assert!(before_complete.0 < now);
    assert_eq!(
        repository
            .expire_abandoned(cursor)
            .await
            .expect("resumed locked batch"),
        1
    );
    assert!(
        !repository
            .has_pending(JobKind::ExpireUploadsAndReservations, cursor)
            .await
            .expect("stable pass has no eligible rows")
    );
    let states: Vec<(uuid::Uuid, String)> = sqlx::query_as("SELECT upload_id,state FROM folioharbor.upload_sessions WHERE upload_id IN($1,$2) ORDER BY upload_id")
        .bind(locked.as_uuid()).bind(available.as_uuid()).fetch_all(&pools.owner).await?;
    assert!(states.contains(&(locked.as_uuid(), "expired".to_owned())));
    assert!(states.contains(&(available.as_uuid(), "expired".to_owned())));
    repository
        .complete_pass(JobKind::ExpireUploadsAndReservations, "cleanup-a", cursor)
        .await?;
    let boundary: (OffsetDateTime, Option<OffsetDateTime>) = sqlx::query_as(
        "SELECT cursor_at,active_cutoff FROM folioharbor.cleanup_boundaries WHERE cleanup_kind='expire_uploads'",
    )
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(boundary, (now, None));
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn received_expiry_releases_quota_and_purge_waits_exactly_twenty_four_hours()
-> anyhow::Result<()> {
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
    sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,'purge@test.invalid','purge@test.invalid','verified',$2,$2)").bind(actor.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,quota_reserved_bytes,created_at,updated_at) VALUES($1,'Purge',12,$2,$2)").bind(library.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.upload_sessions(upload_id,library_id,created_by,file_name,media_type,declared_bytes,received_bytes,state,dedup_scope,storage_key,sha256,expires_at,created_at,updated_at) VALUES($1,$2,$3,'book.epub','application/epub+zip',12,12,'received','disabled',$4,$5,$6,$7,$7)")
        .bind(upload.as_uuid()).bind(library.as_uuid()).bind(actor.as_uuid())
        .bind(format!("blob:upload-{}:{}:12", upload.as_uuid().simple(), "11".repeat(32)))
        .bind(vec![0x11_u8;32]).bind(now-Duration::hours(1)).bind(now-Duration::hours(2)).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.quota_reservations(upload_id,library_id,reserved_bytes,expires_at,state) VALUES($1,$2,12,$3,'active')").bind(upload.as_uuid()).bind(library.as_uuid()).bind(now-Duration::hours(1)).execute(&pools.owner).await?;
    let repository = PgImportCleanupRepository::new(pools.worker.clone());
    assert_eq!(
        repository
            .expire_abandoned(CleanupCursor::new(now, 10).expect("cursor"))
            .await?,
        1
    );
    let quota: (i64, String) = sqlx::query_as("SELECT library.quota_reserved_bytes,reservation.state FROM folioharbor.libraries library JOIN folioharbor.quota_reservations reservation USING(library_id) WHERE reservation.upload_id=$1")
        .bind(upload.as_uuid()).fetch_one(&pools.owner).await?;
    assert_eq!(quota, (0, "released".to_owned()));
    assert!(
        repository
            .claim_failed_purges(
                "worker-a",
                CleanupCursor::new(now + Duration::hours(24) - Duration::seconds(1), 10)
                    .expect("cursor"),
                now + Duration::hours(24) - Duration::seconds(1),
            )
            .await?
            .is_empty()
    );
    let claims = repository
        .claim_failed_purges(
            "worker-a",
            CleanupCursor::new(now + Duration::hours(24), 10).expect("cursor"),
            now + Duration::hours(24),
        )
        .await?;
    assert_eq!(claims.len(), 1);
    assert!(claims[0].delete_file);
    let quarantined: String =
        sqlx::query_scalar("SELECT state FROM folioharbor.blob_locations WHERE storage_key=$1")
            .bind(claims[0].storage_key.as_str())
            .fetch_one(&pools.owner)
            .await?;
    assert_eq!(quarantined, "quarantined");
    assert!(
        repository
            .complete_failed_purge(
                upload,
                "worker-a",
                CleanupCursor::new(now + Duration::hours(24), 10).expect("cursor")
            )
            .await?
    );
    let purged: String =
        sqlx::query_scalar("SELECT state FROM folioharbor.blob_locations WHERE storage_key=$1")
            .bind(claims[0].storage_key.as_str())
            .fetch_one(&pools.owner)
            .await?;
    assert_eq!(purged, "purged");
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn crashed_purge_claim_reclaims_with_same_cutoff_after_wall_clock_lease_expiry()
-> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_migrations(&pools.owner).await?;
    let cutoff = OffsetDateTime::now_utc();
    let actor = UserId::new();
    let library = LibraryId::new();
    let upload = UploadId::new();
    sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,'reclaim@test.invalid','reclaim@test.invalid','verified',$2,$2)")
        .bind(actor.as_uuid()).bind(cutoff).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,created_at,updated_at) VALUES($1,'Reclaim',$2,$2)")
        .bind(library.as_uuid()).bind(cutoff).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.upload_sessions(upload_id,library_id,created_by,file_name,media_type,declared_bytes,received_bytes,state,dedup_scope,storage_key,sha256,error_code,expires_at,created_at,updated_at) VALUES($1,$2,$3,'bad.epub','application/epub+zip',1,1,'failed','instance','blob:shared',decode(repeat('33',32),'hex'),'invalid_epub',$4,$5,$5)")
        .bind(upload.as_uuid()).bind(library.as_uuid()).bind(actor.as_uuid())
        .bind(cutoff+Duration::hours(1)).bind(cutoff).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.failed_upload_purges(upload_id,storage_key,delete_file,eligible_at,created_at,updated_at) VALUES($1,'blob:shared',false,$2,$2,$2)")
        .bind(upload.as_uuid()).bind(cutoff).execute(&pools.owner).await?;
    let repository = PgImportCleanupRepository::new(pools.worker.clone());
    let cursor = repository
        .begin_pass(JobKind::PurgeFailedUploads, "same-pass", cutoff, 10)
        .await?;
    assert_eq!(
        repository
            .claim_failed_purges("same-pass", cursor, cutoff)
            .await?
            .len(),
        1
    );
    let reclaimed = repository
        .claim_failed_purges("same-pass", cursor, cutoff + Duration::minutes(6))
        .await?;
    assert_eq!(reclaimed.len(), 1);
    assert!(
        repository
            .complete_failed_purge(upload, "same-pass", cursor)
            .await?
    );
    repository
        .complete_pass(JobKind::PurgeFailedUploads, "same-pass", cursor)
        .await?;
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn failed_transition_and_purge_schedule_are_atomic_and_reconciliation_repairs_legacy_rows()
-> anyhow::Result<()> {
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
    let storage = StorageKey::from_opaque(format!(
        "blob:upload-{}:{}:12",
        upload.as_uuid().simple(),
        "22".repeat(32)
    ));
    sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,'atomic@test.invalid','atomic@test.invalid','verified',$2,$2)")
        .bind(actor.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,quota_reserved_bytes,created_at,updated_at) VALUES($1,'Atomic',12,$2,$2)")
        .bind(library.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.upload_sessions(upload_id,library_id,created_by,file_name,media_type,declared_bytes,received_bytes,state,dedup_scope,storage_key,sha256,expires_at,created_at,updated_at) VALUES($1,$2,$3,'book.epub','application/epub+zip',12,12,'validating','disabled',$4,$5,$6,$7,$7)")
        .bind(upload.as_uuid()).bind(library.as_uuid()).bind(actor.as_uuid())
        .bind(storage.as_str()).bind(vec![0x22_u8;32]).bind(now+Duration::hours(1)).bind(now)
        .execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.quota_reservations(upload_id,library_id,reserved_bytes,expires_at,state) VALUES($1,$2,12,$3,'active')")
        .bind(upload.as_uuid()).bind(library.as_uuid()).bind(now+Duration::hours(1))
        .execute(&pools.owner).await?;
    sqlx::query("ALTER TABLE folioharbor.failed_upload_purges ADD CONSTRAINT injected_purge_failure CHECK(false) NOT VALID")
        .execute(&pools.owner).await?;
    let repository = PgImportRepository::new(pools.worker.clone());
    let work = ImportWork {
        upload_id: upload,
        library_id: library,
        actor_id: actor,
        blob_id: BlobId::new(),
        logical_bytes: ByteCount::new(12),
        storage_key: storage.clone(),
        state: UploadState::Validating,
    };
    assert!(
        repository
            .record_failure(
                &work,
                UploadState::Failed,
                "invalid_epub",
                RequestId::new(),
                now
            )
            .await
            .is_err()
    );
    let rolled_back: (String, i64, i64, String) = sqlx::query_as("SELECT upload.state,(SELECT count(*) FROM folioharbor.failed_upload_purges WHERE upload_id=$1),library.quota_reserved_bytes,reservation.state FROM folioharbor.upload_sessions upload JOIN folioharbor.libraries library USING(library_id) JOIN folioharbor.quota_reservations reservation USING(upload_id) WHERE upload.upload_id=$1")
        .bind(upload.as_uuid()).fetch_one(&pools.owner).await?;
    assert_eq!(rolled_back, ("validating".into(), 0, 12, "active".into()));

    sqlx::query(
        "ALTER TABLE folioharbor.failed_upload_purges DROP CONSTRAINT injected_purge_failure",
    )
    .execute(&pools.owner)
    .await?;
    repository
        .record_failure(
            &work,
            UploadState::Failed,
            "invalid_epub",
            RequestId::new(),
            now,
        )
        .await
        .expect("atomic failure commits after injected fault is removed");
    let committed: (String, i64, i64, String) = sqlx::query_as("SELECT upload.state,(SELECT count(*) FROM folioharbor.failed_upload_purges WHERE upload_id=$1),library.quota_reserved_bytes,reservation.state FROM folioharbor.upload_sessions upload JOIN folioharbor.libraries library USING(library_id) JOIN folioharbor.quota_reservations reservation USING(upload_id) WHERE upload.upload_id=$1")
        .bind(upload.as_uuid()).fetch_one(&pools.owner).await?;
    assert_eq!(committed, ("failed".into(), 1, 0, "released".into()));

    sqlx::query("DELETE FROM folioharbor.failed_upload_purges WHERE upload_id=$1")
        .bind(upload.as_uuid())
        .execute(&pools.owner)
        .await?;
    let repaired = repository
        .reconcile(upload, library, RequestId::new(), now)
        .await
        .expect("reconciliation repairs the legacy missing purge");
    assert!(
        matches!(repaired, ImportReconciliation::TerminalFailure { code } if code == "invalid_epub")
    );
    let repaired_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM folioharbor.failed_upload_purges WHERE upload_id=$1",
    )
    .bind(upload.as_uuid())
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(repaired_count, 1);
    let after_reconcile: (i64, String) = sqlx::query_as("SELECT library.quota_reserved_bytes,reservation.state FROM folioharbor.libraries library JOIN folioharbor.quota_reservations reservation USING(library_id) WHERE reservation.upload_id=$1")
        .bind(upload.as_uuid()).fetch_one(&pools.owner).await?;
    assert_eq!(after_reconcile, (0, "released".into()));
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}
