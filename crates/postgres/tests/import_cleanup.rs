#![allow(clippy::expect_used, clippy::too_many_lines)]

use folioharbor_application::{imports::CleanupCursor, ports::ImportCleanupRepository};
use folioharbor_domain::{
    id::{LibraryId, UploadId, UserId},
    time::OffsetDateTime,
};
use folioharbor_postgres::{PgImportCleanupRepository, PgPools, run_migrations};
use folioharbor_test_support::postgres::TestPostgres;
use time::Duration;

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
    let cursor = CleanupCursor::new(now, 1).expect("bounded cursor");

    assert_eq!(repository.expire_abandoned(cursor).await?, 1);
    lock.rollback().await?;
    let states: Vec<(uuid::Uuid, String)> = sqlx::query_as("SELECT upload_id,state FROM folioharbor.upload_sessions WHERE upload_id IN($1,$2) ORDER BY upload_id")
        .bind(locked.as_uuid()).bind(available.as_uuid()).fetch_all(&pools.owner).await?;
    assert!(states.contains(&(locked.as_uuid(), "created".to_owned())));
    assert!(states.contains(&(available.as_uuid(), "expired".to_owned())));
    let boundary: OffsetDateTime = sqlx::query_scalar(
        "SELECT cursor_at FROM folioharbor.cleanup_boundaries WHERE cleanup_kind='expire_uploads'",
    )
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(boundary, now);
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
                    .expect("cursor")
            )
            .await?
            .is_empty()
    );
    let claims = repository
        .claim_failed_purges(
            "worker-a",
            CleanupCursor::new(now + Duration::hours(24), 10).expect("cursor"),
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
