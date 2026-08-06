#![allow(clippy::expect_used, clippy::too_many_lines)]

use folioharbor_application::{
    actor::Actor,
    catalog::{DownloadAuthorization, DownloadRange, DownloadRepository},
};
use folioharbor_domain::id::{
    BlobId, HoldingId, ItemId, LibraryId, ManifestationId, PublicationPackageId, RequestId,
    SessionId, UploadId, UserId,
};
use folioharbor_postgres::{PgDownloadRepository, PgPools, run_migrations};
use folioharbor_test_support::postgres::TestPostgres;
use time::OffsetDateTime;

#[tokio::test]
async fn download_matrix_setting_and_audit_are_enforced_in_postgres() -> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_migrations(&pools.owner).await?;
    let now = OffsetDateTime::now_utc();
    let owner = UserId::new();
    let editor = UserId::new();
    let reader = UserId::new();
    let outsider = UserId::new();
    let library = LibraryId::new();
    for (user, email) in [
        (owner, "owner-download@test.invalid"),
        (editor, "editor-download@test.invalid"),
        (reader, "reader-download@test.invalid"),
        (outsider, "outsider-download@test.invalid"),
    ] {
        sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,$2,$2,'verified',$3,$3)")
            .bind(user.as_uuid()).bind(email).bind(now).execute(&pools.owner).await?;
    }
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,created_at,updated_at) VALUES($1,'Downloads',$2,$2)")
        .bind(library.as_uuid()).bind(now).execute(&pools.owner).await?;
    for (user, role) in [(owner, "owner"), (editor, "editor"), (reader, "reader")] {
        sqlx::query("INSERT INTO folioharbor.library_memberships(library_id,user_id,role_code,status,joined_at) VALUES($1,$2,$3,'active',$4)")
            .bind(library.as_uuid()).bind(user.as_uuid()).bind(role).bind(now).execute(&pools.owner).await?;
    }
    let item = seed_item(&pools.owner, library, owner, now).await?;
    let repository = PgDownloadRepository::new(pools.api.clone());
    for (role, user) in [("owner", owner), ("editor", editor)] {
        assert!(matches!(
            repository
                .authorize_download(actor(user), item, RequestId::new())
                .await?,
            DownloadAuthorization::Granted(_)
        ));
        sqlx::query(
            "DELETE FROM folioharbor.role_permissions WHERE role_code=$1 AND permission_code='item.download'",
        )
        .bind(role)
        .execute(&pools.owner)
        .await?;
        assert_eq!(
            repository
                .authorize_download(actor(user), item, RequestId::new())
                .await?,
            DownloadAuthorization::Forbidden
        );
        sqlx::query(
            "INSERT INTO folioharbor.role_permissions(role_code,permission_code) VALUES($1,'item.download')",
        )
        .bind(role)
        .execute(&pools.owner)
        .await?;
    }
    assert_eq!(
        repository
            .authorize_download(actor(reader), item, RequestId::new())
            .await?,
        DownloadAuthorization::Forbidden
    );
    assert_eq!(
        repository
            .authorize_download(actor(outsider), item, RequestId::new())
            .await?,
        DownloadAuthorization::NotFound
    );
    assert_eq!(
        repository
            .authorize_download(actor(outsider), item, RequestId::new())
            .await?,
        DownloadAuthorization::NotFound
    );
    assert_eq!(
        repository
            .authorize_download(actor(outsider), ItemId::new(), RequestId::new())
            .await?,
        DownloadAuthorization::NotFound
    );
    let outsider_denials: Vec<(uuid::Uuid, uuid::Uuid, serde_json::Value)> = sqlx::query_as(
        "SELECT library_id,resource_id,metadata FROM folioharbor.audit_events WHERE action_code='item.download' AND decision='denied' AND actor_id=$1",
    )
    .bind(outsider.as_uuid())
    .fetch_all(&pools.owner)
    .await?;
    assert_eq!(
        outsider_denials,
        vec![(library.as_uuid(), item.as_uuid(), serde_json::json!({}))]
    );
    sqlx::query(
        "UPDATE folioharbor.libraries SET reader_download_enabled=true WHERE library_id=$1",
    )
    .bind(library.as_uuid())
    .execute(&pools.owner)
    .await?;
    assert!(matches!(
        repository
            .authorize_download(actor(reader), item, RequestId::new())
            .await?,
        DownloadAuthorization::Granted(_)
    ));
    sqlx::query(
        "DELETE FROM folioharbor.role_permissions WHERE role_code='reader' AND permission_code='item.download'",
    )
    .execute(&pools.owner)
    .await?;
    assert_eq!(
        repository
            .authorize_download(actor(reader), item, RequestId::new())
            .await?,
        DownloadAuthorization::Forbidden
    );
    assert!(
        !repository
            .record_download_start(
                actor(reader),
                item,
                RequestId::new(),
                DownloadRange { start: 2, end: 7 }
            )
            .await?
    );
    sqlx::query(
        "INSERT INTO folioharbor.role_permissions(role_code,permission_code) VALUES('reader','item.download')",
    )
    .execute(&pools.owner)
    .await?;
    let request_id = RequestId::new();
    assert!(
        repository
            .record_download_start(
                actor(reader),
                item,
                request_id,
                DownloadRange { start: 2, end: 7 }
            )
            .await?
    );
    let metadata: serde_json::Value = sqlx::query_scalar(
        "SELECT metadata FROM folioharbor.audit_events WHERE action_code='item.download' AND decision='allowed' AND request_id=$1",
    ).bind(request_id.as_ulid().to_string()).fetch_one(&pools.owner).await?;
    assert_eq!(
        metadata,
        serde_json::json!({"range_start": 2, "range_end": 7})
    );
    let denied: i64 = sqlx::query_scalar("SELECT count(*) FROM folioharbor.audit_events WHERE action_code='item.download' AND decision='denied' AND actor_id=$1")
        .bind(reader.as_uuid()).fetch_one(&pools.owner).await?;
    assert_eq!(denied, 1);
    database.cleanup().await?;
    Ok(())
}

fn actor(user_id: UserId) -> Actor {
    Actor {
        user_id,
        session_id: SessionId::new(),
    }
}

async fn seed_item(
    pool: &sqlx::PgPool,
    library: LibraryId,
    creator: UserId,
    now: OffsetDateTime,
) -> anyhow::Result<ItemId> {
    let manifestation = ManifestationId::new();
    let blob = BlobId::new();
    let package = PublicationPackageId::new();
    let holding = HoldingId::new();
    let item = ItemId::new();
    let upload = UploadId::new();
    sqlx::query("INSERT INTO folioharbor.manifestations(manifestation_id,identifiers,created_at) VALUES($1,ARRAY[]::text[],$2)").bind(manifestation.as_uuid()).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.blobs(blob_id,storage_namespace,sha256,byte_size,created_at) VALUES($1,'instance-v1',$2,16,$3)").bind(blob.as_uuid()).bind(vec![42_u8;32]).bind(now).execute(pool).await?;
    let key = format!("blob:instance-v1:{}:16", "2a".repeat(32));
    sqlx::query("INSERT INTO folioharbor.blob_locations(blob_id,storage_key,state,created_at,updated_at) VALUES($1,$2,'ready',$3,$3)").bind(blob.as_uuid()).bind(&key).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.upload_sessions(upload_id,library_id,created_by,file_name,media_type,declared_bytes,dedup_scope,received_bytes,state,storage_key,sha256,expires_at,created_at,updated_at) VALUES($1,$2,$3,'原著.epub','application/epub+zip',16,'instance',16,'ready',$4,$5,$6,$7,$7)")
        .bind(upload.as_uuid()).bind(library.as_uuid()).bind(creator.as_uuid()).bind(&key).bind(vec![42_u8;32]).bind(now + time::Duration::hours(1)).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.publication_packages(package_id,manifestation_id,blob_id,parser_profile_version,created_at) VALUES($1,$2,$3,'epub-v1',$4)").bind(package.as_uuid()).bind(manifestation.as_uuid()).bind(blob.as_uuid()).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.holdings(holding_id,library_id,manifestation_id,state,created_at) VALUES($1,$2,$3,'active',$4)").bind(holding.as_uuid()).bind(library.as_uuid()).bind(manifestation.as_uuid()).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.items(item_id,holding_id,manifestation_id,package_id,source_upload_id,state,created_at) VALUES($1,$2,$3,$4,$5,'active',$6)").bind(item.as_uuid()).bind(holding.as_uuid()).bind(manifestation.as_uuid()).bind(package.as_uuid()).bind(upload.as_uuid()).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.item_assets(item_id,blob_id,asset_kind,created_at) VALUES($1,$2,'original',$3)").bind(item.as_uuid()).bind(blob.as_uuid()).bind(now).execute(pool).await?;
    Ok(item)
}
