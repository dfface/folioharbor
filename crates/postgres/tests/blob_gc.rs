#![allow(clippy::expect_used, clippy::panic, clippy::too_many_lines)]

use folioharbor_application::{
    catalog::{DeleteItem, DeleteItemCommand, GetItem, RestoreItem, RestoreItemCommand},
    error::AppError,
    ports::{GarbageCollectionRepository, ItemLifecycleRepository},
};
use folioharbor_domain::{
    catalog::ItemLifecycle,
    id::{
        BlobId, ContentUnitId, ExpressionId, HoldingId, ItemId, LibraryId, ManifestationId,
        PublicationPackageId, RequestId, UploadId, UserId, WorkId,
    },
    time::OffsetDateTime,
};
use folioharbor_postgres::{
    DatabaseContext, PgAuthorizationRepository, PgCatalogRepository, PgGarbageCollectionRepository,
    PgItemLifecycleRepository, PgPools, PgTransactionContext, connect_worker, run_migrations,
};
use folioharbor_test_support::postgres::TestPostgres;
use secrecy::SecretString;
use time::Duration;
use tokio::time::{Duration as TokioDuration, sleep, timeout};

struct SeededItem {
    actor: UserId,
    library: LibraryId,
    item: ItemId,
    blob: BlobId,
    package: PublicationPackageId,
    manifestation: ManifestationId,
    content_unit: ContentUnitId,
    storage_key: String,
}

#[tokio::test]
async fn delete_revokes_visibility_restore_is_bounded_and_audit_is_atomic() -> anyhow::Result<()> {
    let (database, pools) = database().await?;
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000)?;
    let seeded = seed_item(&pools, now, 37).await?;
    let lifecycle = PgItemLifecycleRepository::new(pools.api.clone());
    let authorization = PgAuthorizationRepository::new(pools.api.clone());

    let deleted = DeleteItem::new(&lifecycle, &authorization)
        .execute(DeleteItemCommand {
            actor: seeded.actor,
            library_id: seeded.library,
            item_id: seeded.item,
            request_id: RequestId::new(),
            now,
        })
        .await?;
    assert!(matches!(deleted, ItemLifecycle::Deleted { .. }));
    let hidden = GetItem::new(&PgCatalogRepository::new(pools.api.clone()), &authorization)
        .execute(seeded.actor, seeded.library, seeded.item, RequestId::new())
        .await;
    assert!(matches!(hidden, Err(AppError::NotFound { .. })));

    let restored = RestoreItem::new(&lifecycle, &authorization)
        .execute(RestoreItemCommand {
            actor: seeded.actor,
            library_id: seeded.library,
            item_id: seeded.item,
            request_id: RequestId::new(),
            now: now + Duration::days(7) - Duration::nanoseconds(1),
        })
        .await?;
    assert_eq!(restored, ItemLifecycle::Active);

    DeleteItem::new(&lifecycle, &authorization)
        .execute(DeleteItemCommand {
            actor: seeded.actor,
            library_id: seeded.library,
            item_id: seeded.item,
            request_id: RequestId::new(),
            now,
        })
        .await?;
    let too_late = RestoreItem::new(&lifecycle, &authorization)
        .execute(RestoreItemCommand {
            actor: seeded.actor,
            library_id: seeded.library,
            item_id: seeded.item,
            request_id: RequestId::new(),
            now: now + Duration::days(7),
        })
        .await;
    assert!(matches!(
        too_late,
        Err(AppError::Conflict {
            code: "item_recovery_window_elapsed"
        })
    ));
    let audit_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM folioharbor.audit_events WHERE resource_type='item' AND resource_id=$1 AND decision='allowed'",
    )
    .bind(seeded.item.as_uuid())
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(audit_count, 3, "two deletes and one restore are durable");

    finish(database, pools).await
}

#[tokio::test]
async fn delete_remains_idempotent_after_item_purge() -> anyhow::Result<()> {
    let (database, pools) = database().await?;
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000)?;
    let seeded = seed_item(&pools, now - Duration::days(8), 39).await?;
    mark_deleted(&pools, &seeded, now - Duration::days(7)).await?;
    assert_eq!(
        PgGarbageCollectionRepository::new(pools.worker.clone())
            .prepare(now, 10)
            .await?,
        1
    );

    let state = DeleteItem::new(
        &PgItemLifecycleRepository::new(pools.api.clone()),
        &PgAuthorizationRepository::new(pools.api.clone()),
    )
    .execute(DeleteItemCommand {
        actor: seeded.actor,
        library_id: seeded.library,
        item_id: seeded.item,
        request_id: RequestId::new(),
        now: now + Duration::hours(1),
    })
    .await?;
    assert!(matches!(state, ItemLifecycle::Purged { .. }));
    let deletes: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM folioharbor.audit_events WHERE resource_id=$1 AND action_code='item.delete' AND decision='allowed'",
    )
    .bind(seeded.item.as_uuid())
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(deletes, 2, "idempotent post-purge delete is still audited");

    finish(database, pools).await
}

#[tokio::test]
async fn purge_releases_quota_removes_cache_derivatives_and_preserves_progress_and_audit()
-> anyhow::Result<()> {
    let (database, pools) = database().await?;
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000)?;
    let seeded = seed_item(&pools, now - Duration::days(8), 41).await?;
    sqlx::query(
        "INSERT INTO folioharbor.reading_states(user_id,manifestation_id,package_id,content_unit_id,locator,version,updated_at) VALUES($1,$2,$3,$4,$5,1,$6)",
    )
    .bind(seeded.actor.as_uuid())
    .bind(seeded.manifestation.as_uuid())
    .bind(seeded.package.as_uuid())
    .bind(seeded.content_unit.as_uuid())
    .bind(serde_json::json!({"href":"chapter.xhtml","locations":{"progression":0.5}}))
    .bind(now - Duration::days(1))
    .execute(&pools.owner)
    .await?;
    let lifecycle = PgItemLifecycleRepository::new(pools.api.clone());
    DeleteItem::new(
        &lifecycle,
        &PgAuthorizationRepository::new(pools.api.clone()),
    )
    .execute(DeleteItemCommand {
        actor: seeded.actor,
        library_id: seeded.library,
        item_id: seeded.item,
        request_id: RequestId::new(),
        now: now - Duration::days(7),
    })
    .await?;
    let garbage = PgGarbageCollectionRepository::new(pools.worker.clone());

    assert_eq!(garbage.prepare(now, 10).await?, 1);
    let item: (String, Option<uuid::Uuid>, Option<OffsetDateTime>) =
        sqlx::query_as("SELECT state,package_id,purged_at FROM folioharbor.items WHERE item_id=$1")
            .bind(seeded.item.as_uuid())
            .fetch_one(&pools.owner)
            .await?;
    assert_eq!(item.0, "purged");
    assert_eq!(item.1, None);
    assert_eq!(item.2, Some(now));
    let quota: i64 = sqlx::query_scalar(
        "SELECT quota_used_bytes FROM folioharbor.libraries WHERE library_id=$1",
    )
    .bind(seeded.library.as_uuid())
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(quota, 0, "logical quota releases only when recovery ends");
    let derivative_counts: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM folioharbor.publication_packages WHERE package_id=$1),(SELECT count(*) FROM folioharbor.publication_resources WHERE package_id=$1),(SELECT count(*) FROM folioharbor.item_assets WHERE item_id=$2)",
    )
    .bind(seeded.package.as_uuid())
    .bind(seeded.item.as_uuid())
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(derivative_counts, (0, 0, 0));
    let progress: (uuid::Uuid, Option<uuid::Uuid>, Option<uuid::Uuid>, serde_json::Value) =
        sqlx::query_as(
            "SELECT manifestation_id,package_id,content_unit_id,locator FROM folioharbor.reading_states WHERE user_id=$1 AND manifestation_id=$2",
        )
        .bind(seeded.actor.as_uuid())
        .bind(seeded.manifestation.as_uuid())
        .fetch_one(&pools.owner)
        .await?;
    assert_eq!(progress.0, seeded.manifestation.as_uuid());
    assert_eq!(progress.1, None);
    assert_eq!(progress.2, Some(seeded.content_unit.as_uuid()));
    assert_eq!(progress.3["href"], "chapter.xhtml");
    let audit_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM folioharbor.audit_events WHERE resource_id=$1")
            .bind(seeded.item.as_uuid())
            .fetch_one(&pools.owner)
            .await?;
    assert_eq!(audit_count, 1, "audit history is never cascaded");

    assert!(
        garbage
            .claim(
                "worker-a",
                now + Duration::hours(24) - Duration::nanoseconds(1),
                10
            )
            .await?
            .is_empty()
    );
    let claims = garbage
        .claim("worker-a", now + Duration::hours(24), 10)
        .await?;
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].storage_key.as_str(), seeded.storage_key);
    assert!(
        garbage
            .complete(&claims[0], "worker-a", now + Duration::hours(24))
            .await?
    );
    let location: (String, Option<OffsetDateTime>) =
        sqlx::query_as("SELECT state,purged_at FROM folioharbor.blob_locations WHERE blob_id=$1")
            .bind(seeded.blob.as_uuid())
            .fetch_one(&pools.owner)
            .await?;
    assert_eq!(
        location,
        ("purged".to_owned(), Some(now + Duration::hours(24)))
    );

    finish(database, pools).await
}

#[tokio::test]
async fn shared_item_and_manifestation_asset_references_prevent_collection() -> anyhow::Result<()> {
    let (database, pools) = database().await?;
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000)?;
    let first = seed_item(&pools, now - Duration::days(8), 43).await?;
    let second = seed_shared_item(&pools, &first, now - Duration::days(8), 43).await?;
    mark_deleted(&pools, &first, now - Duration::days(7)).await?;
    let garbage = PgGarbageCollectionRepository::new(pools.worker.clone());

    assert_eq!(garbage.prepare(now, 10).await?, 1);
    let still_ready: String =
        sqlx::query_scalar("SELECT state FROM folioharbor.blob_locations WHERE blob_id=$1")
            .bind(first.blob.as_uuid())
            .fetch_one(&pools.owner)
            .await?;
    assert_eq!(still_ready, "ready");
    let shared_refs: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM folioharbor.item_assets WHERE blob_id=$1),(SELECT count(*) FROM folioharbor.manifestation_assets WHERE blob_id=$1)",
    )
    .bind(first.blob.as_uuid())
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(shared_refs, (1, 1));

    mark_deleted(&pools, &second, now - Duration::days(7)).await?;
    assert_eq!(garbage.prepare(now, 10).await?, 1);
    let pending: String =
        sqlx::query_scalar("SELECT state FROM folioharbor.blob_locations WHERE blob_id=$1")
            .bind(first.blob.as_uuid())
            .fetch_one(&pools.owner)
            .await?;
    assert_eq!(pending, "purge_pending");

    finish(database, pools).await
}

#[tokio::test]
async fn purged_blob_constraint_preserves_the_exact_twenty_four_hour_delay() -> anyhow::Result<()> {
    let (database, pools) = database().await?;
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000)?;
    let seeded = seed_item(&pools, now, 45).await?;

    let shortened = sqlx::query(
        "UPDATE folioharbor.blob_locations SET state='purged',purge_pending_at=$2,purge_after=$2,purged_at=$2,updated_at=$2 WHERE blob_id=$1",
    )
    .bind(seeded.blob.as_uuid())
    .bind(now)
    .execute(&pools.owner)
    .await;
    assert!(
        shortened.is_err(),
        "terminal Blob state must not shorten the independent 24-hour delay"
    );

    finish(database, pools).await
}

#[tokio::test]
async fn reclaimed_blob_claims_fence_stale_completion_and_release() -> anyhow::Result<()> {
    let (database, pools) = database().await?;
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000)?;
    let first = seed_item(&pools, now - Duration::days(8), 55).await?;
    let second = seed_item(&pools, now - Duration::days(8), 57).await?;
    mark_deleted(&pools, &first, now - Duration::days(7)).await?;
    mark_deleted(&pools, &second, now - Duration::days(7)).await?;
    let garbage = PgGarbageCollectionRepository::new(pools.worker.clone());
    assert_eq!(garbage.prepare(now, 10).await?, 2);
    let claim_at = now + Duration::hours(24);
    let stale = garbage.claim("worker-a", claim_at, 10).await?;
    assert_eq!(stale.len(), 2);
    let reclaimed = garbage
        .claim("worker-a", claim_at + Duration::minutes(5), 10)
        .await?;
    assert_eq!(reclaimed.len(), 2);
    let stale_first = stale
        .iter()
        .find(|claim| claim.blob_id == first.blob)
        .expect("first stale claim");
    let stale_second = stale
        .iter()
        .find(|claim| claim.blob_id == second.blob)
        .expect("second stale claim");
    let fresh_first = reclaimed
        .iter()
        .find(|claim| claim.blob_id == first.blob)
        .expect("first reclaimed claim");
    let fresh_second = reclaimed
        .iter()
        .find(|claim| claim.blob_id == second.blob)
        .expect("second reclaimed claim");

    assert!(
        !garbage
            .release(stale_first, "worker-a", claim_at + Duration::minutes(5))
            .await?,
        "an expired claim cannot release its successor's lease"
    );
    assert!(
        !garbage
            .complete(stale_second, "worker-a", claim_at + Duration::minutes(5))
            .await?,
        "an expired claim cannot complete its successor's lease"
    );
    assert!(
        garbage
            .complete(fresh_first, "worker-a", claim_at + Duration::minutes(5))
            .await?
    );
    assert!(
        garbage
            .complete(fresh_second, "worker-a", claim_at + Duration::minutes(5))
            .await?
    );

    finish(database, pools).await
}

#[tokio::test]
async fn concurrent_reference_creation_locks_or_defeats_the_final_recheck() -> anyhow::Result<()> {
    let (database, pools) = database().await?;
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000)?;
    let first = seed_item(&pools, now - Duration::days(8), 47).await?;
    let second = seed_shared_item(&pools, &first, now - Duration::days(8), 47).await?;
    mark_deleted(&pools, &first, now - Duration::days(7)).await?;
    sqlx::query("DELETE FROM folioharbor.item_assets WHERE item_id=$1")
        .bind(second.item.as_uuid())
        .execute(&pools.owner)
        .await?;
    let mut importing = pools.owner.begin().await?;
    sqlx::query("SELECT 1 FROM folioharbor.blobs WHERE blob_id=$1 FOR KEY SHARE")
        .bind(first.blob.as_uuid())
        .execute(&mut *importing)
        .await?;
    sqlx::query("INSERT INTO folioharbor.item_assets(item_id,blob_id,asset_kind,created_at) VALUES($1,$2,'original',$3)")
        .bind(second.item.as_uuid())
        .bind(first.blob.as_uuid())
        .bind(now)
        .execute(&mut *importing)
        .await?;
    let garbage = PgGarbageCollectionRepository::new(pools.worker.clone());
    let task = tokio::spawn(async move { garbage.prepare(now, 10).await });
    sleep(TokioDuration::from_millis(100)).await;
    assert!(
        !task.is_finished(),
        "GC must wait for the importing Blob lock"
    );
    importing.commit().await?;
    assert_eq!(
        timeout(TokioDuration::from_secs(2), task).await???,
        1,
        "the eligible item still makes bounded progress"
    );
    let state: String =
        sqlx::query_scalar("SELECT state FROM folioharbor.blob_locations WHERE blob_id=$1")
            .bind(first.blob.as_uuid())
            .fetch_one(&pools.owner)
            .await?;
    assert_eq!(
        state, "ready",
        "the final reference recheck defeats collection"
    );

    finish(database, pools).await
}

#[tokio::test]
async fn gc_and_catalog_import_follow_library_then_blob_lock_order() -> anyhow::Result<()> {
    let (database, pools) = database().await?;
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000)?;
    let seeded = seed_item(&pools, now - Duration::days(8), 53).await?;
    mark_deleted(&pools, &seeded, now - Duration::days(7)).await?;
    let upload = seed_import(&pools, &seeded, 53, now).await?;
    let request = RequestId::new();
    let mut import_transaction = pools.worker.begin().await?;
    PgTransactionContext::apply(
        &mut import_transaction,
        &DatabaseContext::worker(request, Some(seeded.library)),
    )
    .await?;
    let quota_lock: String = sqlx::query_scalar("SELECT folioharbor.quota_resize($1,$2,$3)")
        .bind(seeded.library.as_uuid())
        .bind(upload.as_uuid())
        .bind(53_i64)
        .fetch_one(&mut *import_transaction)
        .await?;
    assert_eq!(quota_lock, "applied");
    let mut blob_blocker = pools.owner.begin().await?;
    sqlx::query("SELECT 1 FROM folioharbor.blobs WHERE blob_id=$1 FOR UPDATE")
        .bind(seeded.blob.as_uuid())
        .execute(&mut *blob_blocker)
        .await?;

    let gc_pool = connect_worker(&SecretString::from(database.worker_url()?)).await?;
    let garbage = PgGarbageCollectionRepository::new(gc_pool.clone());
    let gc = tokio::spawn(async move { garbage.prepare(now, 10).await });
    sleep(TokioDuration::from_millis(100)).await;
    assert!(!gc.is_finished(), "GC is queued behind the Blob blocker");
    let library = seeded.library;
    let actor = seeded.actor;
    let blob = seeded.blob;
    let importing = tokio::spawn(async move {
        let valid: bool =
            sqlx::query_scalar("SELECT folioharbor.catalog_validate_import($1,$2,$3,$4,$5,$6)")
                .bind(library.as_uuid())
                .bind(upload.as_uuid())
                .bind(actor.as_uuid())
                .bind(blob.as_uuid())
                .bind(53_i64)
                .bind(request.as_ulid().to_string())
                .fetch_one(&mut *import_transaction)
                .await?;
        import_transaction.commit().await?;
        anyhow::Ok(valid)
    });
    sleep(TokioDuration::from_millis(100)).await;
    assert!(
        !importing.is_finished(),
        "the real import path is queued behind a lock held by GC"
    );
    blob_blocker.commit().await?;
    assert!(timeout(TokioDuration::from_secs(3), importing).await???);
    assert_eq!(timeout(TokioDuration::from_secs(3), gc).await???, 1);
    gc_pool.close().await;

    finish(database, pools).await
}

#[tokio::test]
async fn multi_library_gc_batch_does_not_retain_blob_before_a_later_library_lock()
-> anyhow::Result<()> {
    let (database, pools) = database().await?;
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000)?;
    let first = seed_item(&pools, now - Duration::days(8), 59).await?;
    let second = seed_shared_item(&pools, &first, now - Duration::days(8), 59).await?;
    mark_deleted(
        &pools,
        &first,
        now - Duration::days(7) - Duration::seconds(1),
    )
    .await?;
    mark_deleted(&pools, &second, now - Duration::days(7)).await?;
    let upload = seed_import(&pools, &second, 59, now).await?;
    let request = RequestId::new();
    let mut import_transaction = pools.worker.begin().await?;
    PgTransactionContext::apply(
        &mut import_transaction,
        &DatabaseContext::worker(request, Some(second.library)),
    )
    .await?;
    let quota_lock: String = sqlx::query_scalar("SELECT folioharbor.quota_resize($1,$2,$3)")
        .bind(second.library.as_uuid())
        .bind(upload.as_uuid())
        .bind(59_i64)
        .fetch_one(&mut *import_transaction)
        .await?;
    assert_eq!(quota_lock, "applied");

    let gc_pool = connect_worker(&SecretString::from(database.worker_url()?)).await?;
    let garbage = PgGarbageCollectionRepository::new(gc_pool.clone());
    let gc = tokio::spawn(async move { garbage.prepare(now, 10).await });
    sleep(TokioDuration::from_millis(100)).await;
    assert!(
        !gc.is_finished(),
        "GC reaches the later candidate's locked Library"
    );
    let library = second.library;
    let actor = second.actor;
    let blob = second.blob;
    let importing = tokio::spawn(async move {
        let valid: bool =
            sqlx::query_scalar("SELECT folioharbor.catalog_validate_import($1,$2,$3,$4,$5,$6)")
                .bind(library.as_uuid())
                .bind(upload.as_uuid())
                .bind(actor.as_uuid())
                .bind(blob.as_uuid())
                .bind(59_i64)
                .bind(request.as_ulid().to_string())
                .fetch_one(&mut *import_transaction)
                .await?;
        import_transaction.commit().await?;
        anyhow::Ok(valid)
    });
    assert!(timeout(TokioDuration::from_secs(3), importing).await???);
    assert_eq!(timeout(TokioDuration::from_secs(3), gc).await???, 2);
    gc_pool.close().await;

    finish(database, pools).await
}

async fn database() -> anyhow::Result<(TestPostgres, PgPools)> {
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_migrations(&pools.owner).await?;
    Ok((database, pools))
}

async fn finish(database: TestPostgres, pools: PgPools) -> anyhow::Result<()> {
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

async fn seed_item(pools: &PgPools, now: OffsetDateTime, bytes: i64) -> anyhow::Result<SeededItem> {
    let actor = UserId::new();
    let library = LibraryId::new();
    let blob = BlobId::new();
    let manifestation = ManifestationId::new();
    let package = PublicationPackageId::new();
    let content_unit = ContentUnitId::new();
    let work = WorkId::new();
    let expression = ExpressionId::new();
    let upload = UploadId::new();
    let holding = HoldingId::new();
    let item = ItemId::new();
    let storage_key = format!("blob:instance-v1:{}:{bytes}", "ab".repeat(32));
    sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,$2,$2,'verified',$3,$3)")
        .bind(actor.as_uuid()).bind(format!("{}@gc.test", actor.as_uuid())).bind(now).execute(&pools.owner).await?;
    seed_library(pools, library, actor, bytes, now).await?;
    sqlx::query("INSERT INTO folioharbor.upload_sessions(upload_id,library_id,created_by,file_name,media_type,declared_bytes,received_bytes,state,dedup_scope,storage_key,sha256,expires_at,created_at,updated_at) VALUES($1,$2,$3,'book.epub','application/epub+zip',$4,$4,'ready','instance',$5,$6,$7,$8,$8)")
        .bind(upload.as_uuid()).bind(library.as_uuid()).bind(actor.as_uuid()).bind(bytes).bind(&storage_key)
        .bind(vec![0xab_u8; 32]).bind(now + Duration::days(30)).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.blobs(blob_id,storage_namespace,sha256,byte_size,created_at) VALUES($1,'instance-v1',$2,$3,$4)")
        .bind(blob.as_uuid()).bind(vec![0xab_u8; 32]).bind(bytes).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.blob_locations(blob_id,storage_key,state,created_at,updated_at) VALUES($1,$2,'ready',$3,$3)")
        .bind(blob.as_uuid()).bind(&storage_key).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.works(work_id,primary_title,authors,created_at) VALUES($1,'GC Book',ARRAY['Author'],$2)")
        .bind(work.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.expressions(expression_id,work_id,languages,created_at) VALUES($1,$2,ARRAY['en'],$3)")
        .bind(expression.as_uuid()).bind(work.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.manifestations(manifestation_id,identifiers,created_at) VALUES($1,ARRAY['urn:gc'],$2)")
        .bind(manifestation.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.manifestation_expressions(manifestation_id,expression_id,expression_order) VALUES($1,$2,0)")
        .bind(manifestation.as_uuid()).bind(expression.as_uuid()).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.publication_packages(package_id,manifestation_id,blob_id,parser_profile_version,created_at) VALUES($1,$2,$3,'epub-v1',$4)")
        .bind(package.as_uuid()).bind(manifestation.as_uuid()).bind(blob.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.publication_resources(package_id,resource_order,normalized_href,media_type,source_blob_id) VALUES($1,0,'chapter.xhtml','application/xhtml+xml',$2)")
        .bind(package.as_uuid()).bind(blob.as_uuid()).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.content_units(content_unit_id,package_id,locator_href,created_at) VALUES($1,$2,'chapter.xhtml',$3)")
        .bind(content_unit.as_uuid()).bind(package.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.manifestation_units(manifestation_id,package_id,content_unit_id,spine_order,linear) VALUES($1,$2,$3,0,true)")
        .bind(manifestation.as_uuid()).bind(package.as_uuid()).bind(content_unit.as_uuid()).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.manifestation_assets(manifestation_id,blob_id,asset_kind,locator_href,created_at) VALUES($1,$2,'original',NULL,$3)")
        .bind(manifestation.as_uuid()).bind(blob.as_uuid()).bind(now).execute(&pools.owner).await?;
    seed_holding_item(
        pools,
        library,
        actor,
        upload,
        holding,
        item,
        manifestation,
        package,
        blob,
        now,
    )
    .await?;
    Ok(SeededItem {
        actor,
        library,
        item,
        blob,
        package,
        manifestation,
        content_unit,
        storage_key,
    })
}

async fn seed_shared_item(
    pools: &PgPools,
    shared: &SeededItem,
    now: OffsetDateTime,
    bytes: i64,
) -> anyhow::Result<SeededItem> {
    let library = LibraryId::new();
    let upload = UploadId::new();
    let holding = HoldingId::new();
    let item = ItemId::new();
    seed_library(pools, library, shared.actor, bytes, now).await?;
    sqlx::query("INSERT INTO folioharbor.upload_sessions(upload_id,library_id,created_by,file_name,media_type,declared_bytes,received_bytes,state,dedup_scope,storage_key,sha256,expires_at,created_at,updated_at) VALUES($1,$2,$3,'shared.epub','application/epub+zip',$4,$4,'ready','instance',$5,$6,$7,$8,$8)")
        .bind(upload.as_uuid()).bind(library.as_uuid()).bind(shared.actor.as_uuid()).bind(bytes).bind(&shared.storage_key)
        .bind(vec![0xab_u8; 32]).bind(now + Duration::days(30)).bind(now).execute(&pools.owner).await?;
    seed_holding_item(
        pools,
        library,
        shared.actor,
        upload,
        holding,
        item,
        shared.manifestation,
        shared.package,
        shared.blob,
        now,
    )
    .await?;
    Ok(SeededItem {
        actor: shared.actor,
        library,
        item,
        blob: shared.blob,
        package: shared.package,
        manifestation: shared.manifestation,
        content_unit: shared.content_unit,
        storage_key: shared.storage_key.clone(),
    })
}

async fn seed_import(
    pools: &PgPools,
    seeded: &SeededItem,
    bytes: i64,
    now: OffsetDateTime,
) -> anyhow::Result<UploadId> {
    let upload = UploadId::new();
    sqlx::query("INSERT INTO folioharbor.quota_reservations(upload_id,library_id,reserved_bytes,expires_at,state) VALUES($1,$2,$3,$4,'active')")
        .bind(upload.as_uuid())
        .bind(seeded.library.as_uuid())
        .bind(bytes)
        .bind(now + Duration::hours(1))
        .execute(&pools.owner)
        .await?;
    sqlx::query("UPDATE folioharbor.libraries SET quota_reserved_bytes=quota_reserved_bytes+$2 WHERE library_id=$1")
        .bind(seeded.library.as_uuid())
        .bind(bytes)
        .execute(&pools.owner)
        .await?;
    sqlx::query("INSERT INTO folioharbor.upload_sessions(upload_id,library_id,created_by,file_name,media_type,declared_bytes,received_bytes,state,dedup_scope,storage_key,sha256,expires_at,created_at,updated_at) VALUES($1,$2,$3,'import.epub','application/epub+zip',$4,$4,'importing','instance',$5,$6,$7,$8,$8)")
        .bind(upload.as_uuid())
        .bind(seeded.library.as_uuid())
        .bind(seeded.actor.as_uuid())
        .bind(bytes)
        .bind(&seeded.storage_key)
        .bind(vec![0xab_u8; 32])
        .bind(now + Duration::hours(1))
        .bind(now)
        .execute(&pools.owner)
        .await?;
    Ok(upload)
}

async fn seed_library(
    pools: &PgPools,
    library: LibraryId,
    actor: UserId,
    bytes: i64,
    now: OffsetDateTime,
) -> anyhow::Result<()> {
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,quota_used_bytes,created_at,updated_at) VALUES($1,'GC',$2,$3,$3)")
        .bind(library.as_uuid()).bind(bytes).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.library_memberships(library_id,user_id,role_code,status,joined_at) VALUES($1,$2,'editor','active',$3)")
        .bind(library.as_uuid()).bind(actor.as_uuid()).bind(now).execute(&pools.owner).await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn seed_holding_item(
    pools: &PgPools,
    library: LibraryId,
    _: UserId,
    upload: UploadId,
    holding: HoldingId,
    item: ItemId,
    manifestation: ManifestationId,
    package: PublicationPackageId,
    blob: BlobId,
    now: OffsetDateTime,
) -> anyhow::Result<()> {
    sqlx::query("INSERT INTO folioharbor.holdings(holding_id,library_id,manifestation_id,state,created_at) VALUES($1,$2,$3,'active',$4)")
        .bind(holding.as_uuid()).bind(library.as_uuid()).bind(manifestation.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.items(item_id,holding_id,manifestation_id,package_id,source_upload_id,state,created_at) VALUES($1,$2,$3,$4,$5,'active',$6)")
        .bind(item.as_uuid()).bind(holding.as_uuid()).bind(manifestation.as_uuid()).bind(package.as_uuid()).bind(upload.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.item_assets(item_id,blob_id,asset_kind,created_at) VALUES($1,$2,'original',$3)")
        .bind(item.as_uuid()).bind(blob.as_uuid()).bind(now).execute(&pools.owner).await?;
    Ok(())
}

async fn mark_deleted(
    pools: &PgPools,
    item: &SeededItem,
    at: OffsetDateTime,
) -> anyhow::Result<()> {
    let lifecycle = PgItemLifecycleRepository::new(pools.api.clone());
    let state = DeleteItem::new(
        &lifecycle,
        &PgAuthorizationRepository::new(pools.api.clone()),
    )
    .execute(DeleteItemCommand {
        actor: item.actor,
        library_id: item.library,
        item_id: item.item,
        request_id: RequestId::new(),
        now: at,
    })
    .await?;
    assert!(matches!(state, ItemLifecycle::Deleted { .. }));
    Ok(())
}

const _: Option<&dyn ItemLifecycleRepository> = None;
