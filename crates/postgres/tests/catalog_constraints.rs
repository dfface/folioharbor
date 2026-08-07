#![allow(clippy::expect_used, clippy::panic, clippy::too_many_lines)]

use folioharbor_application::catalog::{
    ImportCatalogCommand, ImportCatalogResult, ImportPublicationCatalog,
};
use folioharbor_domain::{
    catalog::{
        CatalogMetadata, CatalogPublication, ParserMetadata, PublicationResource, SpineEntry,
        TocEntry,
    },
    id::{BlobId, ContentUnitId, HoldingId, ItemId, LibraryId, RequestId, UploadId, UserId},
    imports::{blob::ByteCount, upload::UploadState},
};
use folioharbor_postgres::{
    DatabaseContext, PgCatalogRepository, PgPools, PgTransactionContext, run_migrations,
};
use folioharbor_test_support::postgres::TestPostgres;
use time::{Duration, OffsetDateTime};
use tokio::time::{Duration as TokioDuration, Instant, sleep, timeout};

#[tokio::test]
async fn exact_blob_is_idempotent_per_library_but_shared_across_libraries() -> anyhow::Result<()> {
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
    let first_library = LibraryId::new();
    let second_library = LibraryId::new();
    seed_user(&pools, actor, now).await?;
    seed_library(&pools, first_library, actor, now).await?;
    seed_library(&pools, second_library, actor, now).await?;
    let blob = seed_blob(&pools, 123, now).await?;
    let first_upload = seed_upload(&pools, first_library, actor, blob, 123, now).await?;
    let duplicate_upload = seed_upload(&pools, first_library, actor, blob, 123, now).await?;
    let second_upload = seed_upload(&pools, second_library, actor, blob, 123, now).await?;
    let repository = PgCatalogRepository::new(pools.worker.clone());
    let use_case = ImportPublicationCatalog::new(&repository);

    let first = use_case
        .execute(command(
            first_library,
            first_upload,
            actor,
            blob,
            123,
            "Title",
        ))
        .await?;
    let duplicate = use_case
        .execute(command(
            first_library,
            duplicate_upload,
            actor,
            blob,
            123,
            "Changed",
        ))
        .await?;
    let cross_library = use_case
        .execute(command(
            second_library,
            second_upload,
            actor,
            blob,
            123,
            "Changed",
        ))
        .await?;

    let ImportCatalogResult::Created {
        item_id: first_item,
        package_id,
    } = first
    else {
        panic!("first import must create")
    };
    assert_eq!(
        duplicate,
        ImportCatalogResult::Duplicate {
            item_id: first_item
        }
    );
    assert!(
        matches!(cross_library, ImportCatalogResult::Created { package_id: shared, .. } if shared == package_id)
    );
    let counts: (i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM folioharbor.works), (SELECT count(*) FROM folioharbor.expressions), (SELECT count(*) FROM folioharbor.manifestations), (SELECT count(*) FROM folioharbor.holdings), (SELECT count(*) FROM folioharbor.items)",
    )
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(counts, (1, 1, 1, 2, 2));
    let structure: (i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM folioharbor.manifestation_expressions), (SELECT count(*) FROM folioharbor.item_assets), (SELECT count(*) FROM folioharbor.publication_resources), (SELECT count(*) FROM folioharbor.content_units), (SELECT count(*) FROM folioharbor.manifestation_units), (SELECT count(*) FROM folioharbor.package_toc_entries), (SELECT count(*) FROM folioharbor.manifestation_assets WHERE asset_kind='cover')",
    )
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(structure, (1, 2, 2, 1, 1, 1, 1));
    let quota: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT quota_used_bytes,quota_reserved_bytes FROM folioharbor.libraries WHERE library_id IN($1,$2) ORDER BY library_id",
    )
    .bind(first_library.as_uuid())
    .bind(second_library.as_uuid())
    .fetch_all(&pools.owner)
    .await?;
    assert_eq!(quota, vec![(123, 0), (123, 0)]);
    let successful_audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM folioharbor.audit_events WHERE action_code='publication.import' AND decision='allowed'",
    )
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(successful_audits, 3);
    let source_upload: uuid::Uuid =
        sqlx::query_scalar("SELECT source_upload_id FROM folioharbor.items WHERE item_id=$1")
            .bind(first_item.as_uuid())
            .fetch_one(&pools.owner)
            .await?;
    assert_eq!(source_upload, first_upload.as_uuid());
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn similar_metadata_on_distinct_blobs_never_auto_merges() -> anyhow::Result<()> {
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
    seed_user(&pools, actor, now).await?;
    seed_library(&pools, library, actor, now).await?;
    let first_blob = seed_blob(&pools, 30, now).await?;
    let second_blob = seed_blob(&pools, 40, now).await?;
    let first_upload = seed_upload(&pools, library, actor, first_blob, 30, now).await?;
    let second_upload = seed_upload(&pools, library, actor, second_blob, 40, now).await?;
    let repository = PgCatalogRepository::new(pools.worker.clone());
    let use_case = ImportPublicationCatalog::new(&repository);
    let _ = use_case
        .execute(command(
            library,
            first_upload,
            actor,
            first_blob,
            30,
            "Same title",
        ))
        .await?;
    let _ = use_case
        .execute(command(
            library,
            second_upload,
            actor,
            second_blob,
            40,
            "Same title",
        ))
        .await?;
    let counts: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM folioharbor.works),(SELECT count(*) FROM folioharbor.expressions),(SELECT count(*) FROM folioharbor.manifestations)").fetch_one(&pools.owner).await?;
    assert_eq!(counts, (2, 2, 2));
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn successful_catalog_completion_releases_temporary_reachability_candidate()
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
    seed_user(&pools, actor, now).await?;
    seed_library(&pools, library, actor, now).await?;
    let blob = seed_blob(&pools, 23, now).await?;
    let upload = seed_upload(&pools, library, actor, blob, 23, now).await?;
    sqlx::query(
        "INSERT INTO folioharbor.blob_reachability_candidates(storage_key,source_upload_id,namespace,sha256,byte_size,state,created_at,updated_at) SELECT storage_key,upload_id,'instance-v1',sha256,received_bytes,'installed_shared',$2,$2 FROM folioharbor.upload_sessions WHERE upload_id=$1",
    )
    .bind(upload.as_uuid())
    .bind(now)
    .execute(&pools.owner)
    .await?;

    let result = ImportPublicationCatalog::new(&PgCatalogRepository::new(pools.worker.clone()))
        .execute(command(
            library,
            upload,
            actor,
            blob,
            23,
            "Reachability handoff",
        ))
        .await?;
    assert!(matches!(result, ImportCatalogResult::Created { .. }));
    let candidates: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM folioharbor.blob_reachability_candidates WHERE source_upload_id=$1",
    )
    .bind(upload.as_uuid())
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(
        candidates, 0,
        "catalog references must replace the temporary promotion guard"
    );
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn toc_fragment_locator_is_persisted_without_weakening_resource_paths() -> anyhow::Result<()>
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
    seed_user(&pools, actor, now).await?;
    seed_library(&pools, library, actor, now).await?;
    let blob = seed_blob(&pools, 19, now).await?;
    let upload = seed_upload(&pools, library, actor, blob, 19, now).await?;

    let result = ImportPublicationCatalog::new(&PgCatalogRepository::new(pools.worker.clone()))
        .execute(command_with_toc_locator(
            library,
            upload,
            actor,
            blob,
            19,
            "Fragment",
            "OPS/chapter.xhtml#section-1",
        ))
        .await?;
    assert!(matches!(result, ImportCatalogResult::Created { .. }));
    let locator: String = sqlx::query_scalar(
        "SELECT toc.locator_href FROM folioharbor.package_toc_entries toc JOIN folioharbor.publication_packages package USING(package_id) WHERE package.blob_id=$1",
    )
    .bind(blob.as_uuid())
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(locator, "OPS/chapter.xhtml#section-1");
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn deleting_an_item_keeps_shared_blob_and_package_rows() -> anyhow::Result<()> {
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
    seed_user(&pools, actor, now).await?;
    seed_library(&pools, library, actor, now).await?;
    let blob = seed_blob(&pools, 10, now).await?;
    let upload = seed_upload(&pools, library, actor, blob, 10, now).await?;
    let result = ImportPublicationCatalog::new(&PgCatalogRepository::new(pools.worker.clone()))
        .execute(command(library, upload, actor, blob, 10, "Delete me"))
        .await?;
    let ImportCatalogResult::Created { item_id, .. } = result else {
        panic!("created")
    };
    sqlx::query("DELETE FROM folioharbor.items WHERE item_id=$1")
        .bind(item_id.as_uuid())
        .execute(&pools.owner)
        .await?;
    let counts: (i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM folioharbor.blobs WHERE blob_id=$1),(SELECT count(*) FROM folioharbor.publication_packages WHERE blob_id=$1)").bind(blob.as_uuid()).fetch_one(&pools.owner).await?;
    assert_eq!(counts, (1, 1));
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn concurrent_same_library_finalization_creates_one_active_item() -> anyhow::Result<()> {
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
    seed_user(&pools, actor, now).await?;
    seed_library(&pools, library, actor, now).await?;
    let blob = seed_blob(&pools, 55, now).await?;
    let first_upload = seed_upload(&pools, library, actor, blob, 55, now).await?;
    let second_upload = seed_upload(&pools, library, actor, blob, 55, now).await?;
    let first_repository = PgCatalogRepository::new(pools.worker.clone());
    let second_repository = first_repository.clone();
    let first = tokio::spawn(async move {
        ImportPublicationCatalog::new(&first_repository)
            .execute(command(library, first_upload, actor, blob, 55, "Race"))
            .await
    });
    let second = tokio::spawn(async move {
        ImportPublicationCatalog::new(&second_repository)
            .execute(command(library, second_upload, actor, blob, 55, "Race"))
            .await
    });
    let outcomes = [first.await??, second.await??];
    assert_eq!(
        outcomes
            .iter()
            .filter(|result| matches!(result, ImportCatalogResult::Created { .. }))
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|result| matches!(result, ImportCatalogResult::Duplicate { .. }))
            .count(),
        1
    );
    let counts: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM folioharbor.holdings WHERE state='active'), (SELECT count(*) FROM folioharbor.items WHERE state='active'), (SELECT quota_used_bytes FROM folioharbor.libraries WHERE library_id=$1)",
    )
    .bind(library.as_uuid())
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(counts, (1, 1, 55));
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn inactive_reservation_rolls_back_catalog_and_success_audit() -> anyhow::Result<()> {
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
    seed_user(&pools, actor, now).await?;
    seed_library(&pools, library, actor, now).await?;
    let blob = seed_blob(&pools, 21, now).await?;
    let upload = seed_upload(&pools, library, actor, blob, 21, now).await?;
    sqlx::query("UPDATE folioharbor.quota_reservations SET state='released' WHERE upload_id=$1")
        .bind(upload.as_uuid())
        .execute(&pools.owner)
        .await?;
    let repository = PgCatalogRepository::new(pools.worker.clone());
    let error = ImportPublicationCatalog::new(&repository)
        .execute(command(library, upload, actor, blob, 21, "Rollback"))
        .await
        .expect_err("inactive reservation must reject finalization");
    assert!(matches!(
        error,
        folioharbor_application::error::AppError::Conflict {
            code: "upload_not_importable"
        }
    ));
    let counts: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM folioharbor.works), (SELECT count(*) FROM folioharbor.items), (SELECT count(*) FROM folioharbor.audit_events WHERE action_code='publication.import')",
    )
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(counts, (0, 0, 0));
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn upload_blob_and_reservation_identity_mismatches_roll_back_everything() -> anyhow::Result<()>
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
    seed_user(&pools, actor, now).await?;
    let repository = PgCatalogRepository::new(pools.worker.clone());

    for mismatch in ["blob", "hash", "size", "reservation", "storage"] {
        let library = LibraryId::new();
        seed_library(&pools, library, actor, now).await?;
        let blob = seed_blob(&pools, 21, now).await?;
        let upload = seed_upload(&pools, library, actor, blob, 21, now).await?;
        let command_blob = match mismatch {
            "blob" => seed_blob(&pools, 21, now).await?,
            "hash" => {
                sqlx::query("UPDATE folioharbor.upload_sessions SET sha256=$2 WHERE upload_id=$1")
                    .bind(upload.as_uuid())
                    .bind(vec![9_u8; 32])
                    .execute(&pools.owner)
                    .await?;
                blob
            }
            "size" => {
                sqlx::query(
                    "UPDATE folioharbor.upload_sessions SET received_bytes=20 WHERE upload_id=$1",
                )
                .bind(upload.as_uuid())
                .execute(&pools.owner)
                .await?;
                blob
            }
            "reservation" => {
                sqlx::query("UPDATE folioharbor.quota_reservations SET reserved_bytes=20 WHERE upload_id=$1")
                    .bind(upload.as_uuid()).execute(&pools.owner).await?;
                blob
            }
            "storage" => {
                sqlx::query("UPDATE folioharbor.upload_sessions SET storage_key='blob:instance-v1:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff:21' WHERE upload_id=$1")
                    .bind(upload.as_uuid()).execute(&pools.owner).await?;
                blob
            }
            _ => unreachable!(),
        };
        let result = ImportPublicationCatalog::new(&repository)
            .execute(command(
                library,
                upload,
                actor,
                command_blob,
                21,
                "Mismatch",
            ))
            .await;
        assert!(
            result.is_err(),
            "{mismatch} mismatch must reject finalization"
        );
        let state: (String, i64, i64, String, i64, i64, i64) = sqlx::query_as(
            "SELECT upload.state,library.quota_used_bytes,library.quota_reserved_bytes,reservation.state,(SELECT count(*) FROM folioharbor.holdings WHERE library_id=$1),(SELECT count(*) FROM folioharbor.audit_events WHERE library_id=$1 AND action_code='publication.import'),(SELECT count(*) FROM folioharbor.items item JOIN folioharbor.holdings holding USING(holding_id) WHERE holding.library_id=$1) FROM folioharbor.upload_sessions upload JOIN folioharbor.libraries library USING(library_id) JOIN folioharbor.quota_reservations reservation USING(upload_id) WHERE upload.upload_id=$2",
        )
        .bind(library.as_uuid()).bind(upload.as_uuid()).fetch_one(&pools.owner).await?;
        assert_eq!(state.0, "importing", "{mismatch}");
        assert_eq!(state.1, 0, "{mismatch}");
        assert_eq!(state.2, 21, "{mismatch}");
        assert_eq!(state.3, "active", "{mismatch}");
        assert_eq!((state.4, state.5, state.6), (0, 0, 0), "{mismatch}");
    }
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn catalog_completion_rejects_unproven_item_package_and_original_asset() -> anyhow::Result<()>
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
    seed_user(&pools, actor, now).await?;
    seed_library(&pools, library, actor, now).await?;
    let blob = seed_blob(&pools, 29, now).await?;
    let first_upload = seed_upload(&pools, library, actor, blob, 29, now).await?;
    let repository = PgCatalogRepository::new(pools.worker.clone());
    let ImportCatalogResult::Created { item_id, .. } = ImportPublicationCatalog::new(&repository)
        .execute(command(
            library,
            first_upload,
            actor,
            blob,
            29,
            "Provenance",
        ))
        .await?
    else {
        panic!("first item must be created")
    };
    let other_blob = seed_blob(&pools, 31, now).await?;
    let other_upload = seed_upload(&pools, library, actor, other_blob, 31, now).await?;
    let ImportCatalogResult::Created {
        item_id: other_item,
        ..
    } = ImportPublicationCatalog::new(&repository)
        .execute(command(
            library,
            other_upload,
            actor,
            other_blob,
            31,
            "Other",
        ))
        .await?
    else {
        panic!("other item must be created")
    };

    let wrong_item_upload = seed_upload(&pools, library, actor, blob, 29, now).await?;
    assert_rejected_completion(
        &pools,
        library,
        wrong_item_upload,
        actor,
        blob,
        29,
        other_item,
        now,
    )
    .await?;

    sqlx::query(
        "UPDATE folioharbor.item_assets SET blob_id=$2 WHERE item_id=$1 AND asset_kind='original'",
    )
    .bind(item_id.as_uuid())
    .bind(other_blob.as_uuid())
    .execute(&pools.owner)
    .await?;
    let wrong_asset_upload = seed_upload(&pools, library, actor, blob, 29, now).await?;
    assert_rejected_completion(
        &pools,
        library,
        wrong_asset_upload,
        actor,
        blob,
        29,
        item_id,
        now,
    )
    .await?;
    sqlx::query("UPDATE folioharbor.item_assets SET blob_id=$2 WHERE item_id=$1")
        .bind(item_id.as_uuid())
        .bind(blob.as_uuid())
        .execute(&pools.owner)
        .await?;

    let (manifestation, original_package): (uuid::Uuid, uuid::Uuid) = sqlx::query_as(
        "SELECT item.manifestation_id,item.package_id FROM folioharbor.items item WHERE item.item_id=$1",
    )
    .bind(item_id.as_uuid())
    .fetch_one(&pools.owner)
    .await?;
    let wrong_package = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO folioharbor.publication_packages(package_id,manifestation_id,blob_id,parser_profile_version,created_at) VALUES($1,$2,$3,'epub-v2',$4)")
        .bind(wrong_package).bind(manifestation).bind(blob.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query("UPDATE folioharbor.items SET package_id=$2 WHERE item_id=$1")
        .bind(item_id.as_uuid())
        .bind(wrong_package)
        .execute(&pools.owner)
        .await?;
    let wrong_package_upload = seed_upload(&pools, library, actor, blob, 29, now).await?;
    assert_rejected_completion(
        &pools,
        library,
        wrong_package_upload,
        actor,
        blob,
        29,
        item_id,
        now,
    )
    .await?;
    sqlx::query("UPDATE folioharbor.items SET package_id=$2 WHERE item_id=$1")
        .bind(item_id.as_uuid())
        .bind(original_package)
        .execute(&pools.owner)
        .await?;

    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn catalog_completion_exposes_no_caller_controlled_duplicate_flag() -> anyhow::Result<()> {
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
    seed_user(&pools, actor, now).await?;
    seed_library(&pools, library, actor, now).await?;
    let blob = seed_blob(&pools, 37, now).await?;
    let first_upload = seed_upload(&pools, library, actor, blob, 37, now).await?;
    let repository = PgCatalogRepository::new(pools.worker.clone());
    let ImportCatalogResult::Created { item_id, .. } = ImportPublicationCatalog::new(&repository)
        .execute(command(library, first_upload, actor, blob, 37, "Flags"))
        .await?
    else {
        panic!("first item must be created")
    };
    let late_upload = seed_upload(&pools, library, actor, blob, 37, now).await?;

    let mut provenance_rewrite = pools.owner.begin().await?;
    let rewrite_result =
        sqlx::query("UPDATE folioharbor.items SET source_upload_id=$2 WHERE item_id=$1")
            .bind(item_id.as_uuid())
            .bind(late_upload.as_uuid())
            .execute(&mut *provenance_rewrite)
            .await;
    provenance_rewrite.rollback().await?;
    assert!(rewrite_result.is_err(), "Item provenance must be immutable");

    for forged_duplicate in [false, true] {
        let request = RequestId::new();
        let mut tx = pools.worker.begin().await?;
        PgTransactionContext::apply(&mut tx, &DatabaseContext::worker(request, Some(library)))
            .await?;
        let result = sqlx::query_scalar::<_, String>(
            "SELECT folioharbor.catalog_finish_import($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
        )
        .bind(library.as_uuid())
        .bind(late_upload.as_uuid())
        .bind(actor.as_uuid())
        .bind(blob.as_uuid())
        .bind(37_i64)
        .bind(item_id.as_uuid())
        .bind(forged_duplicate)
        .bind(uuid::Uuid::now_v7())
        .bind(request.as_ulid().to_string())
        .bind(now)
        .fetch_one(&mut *tx)
        .await;
        tx.rollback().await?;
        assert!(
            result.is_err(),
            "catalog completion must not expose a caller-controlled duplicate flag"
        );
    }
    let state: (String, i64, i64, String, i64) = sqlx::query_as(
        "SELECT upload.state,library.quota_used_bytes,library.quota_reserved_bytes,reservation.state,(SELECT count(*) FROM folioharbor.audit_events WHERE resource_id=$1 AND action_code='publication.import') FROM folioharbor.upload_sessions upload JOIN folioharbor.libraries library USING(library_id) JOIN folioharbor.quota_reservations reservation USING(upload_id) WHERE upload.upload_id=$1",
    )
    .bind(late_upload.as_uuid())
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(
        (state.0, state.1, state.2, state.3),
        ("importing".into(), 37, 37, "active".into())
    );
    assert_eq!(state.4, 0);
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn catalog_finalization_and_late_upload_finalization_share_library_first_lock_order()
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
    seed_user(&pools, actor, now).await?;
    seed_library(&pools, library, actor, now).await?;
    let blob = seed_blob(&pools, 23, now).await?;
    let first_upload = seed_upload(&pools, library, actor, blob, 23, now).await?;
    let repository = PgCatalogRepository::new(pools.worker.clone());
    let ImportCatalogResult::Created { item_id, .. } = ImportPublicationCatalog::new(&repository)
        .execute(command(library, first_upload, actor, blob, 23, "Locks"))
        .await?
    else {
        panic!("first import must create the duplicate target")
    };
    let late_upload = seed_upload(&pools, library, actor, blob, 23, now).await?;
    let request = RequestId::new();

    timeout(TokioDuration::from_secs(5), async {
        let mut api = pools.api.begin().await?;
        PgTransactionContext::apply(
            &mut api,
            &DatabaseContext::api(actor, library, RequestId::new()),
        )
        .await?;
        let helper_upload = UploadId::new();
        let create_outcome: String = sqlx::query_scalar(
            "SELECT folioharbor.upload_create_authorized($1,$2,$3,'lock.epub','application/epub+zip',1,'instance',$4,$5)",
        )
            .bind(helper_upload.as_uuid())
            .bind(library.as_uuid())
            .bind(actor.as_uuid())
            .bind(now + Duration::hours(1))
            .bind(now)
            .fetch_one(&mut *api)
            .await?;
        assert_eq!(create_outcome, "created");

        let worker_pool = pools.worker.clone();
        let (pid_sender, pid_receiver) = tokio::sync::oneshot::channel();
        let worker = tokio::spawn(async move {
            let mut tx = worker_pool.begin().await?;
            PgTransactionContext::apply(
                &mut tx,
                &DatabaseContext::worker(request, Some(library)),
            )
            .await?;
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *tx)
                .await?;
            pid_sender.send(pid).expect("lock test still awaits worker pid");
            let outcome: String = sqlx::query_scalar(
                "SELECT folioharbor.catalog_finish_import($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
            )
            .bind(library.as_uuid())
            .bind(late_upload.as_uuid())
            .bind(actor.as_uuid())
            .bind(blob.as_uuid())
            .bind(23_i64)
            .bind("epub-v1")
            .bind(item_id.as_uuid())
            .bind(uuid::Uuid::now_v7())
            .bind(request.as_ulid().to_string())
            .bind(now)
            .fetch_one(&mut *tx)
            .await?;
            tx.commit().await?;
            anyhow::Ok(outcome)
        });
        let worker_pid = pid_receiver.await.expect("worker reports its backend pid");

        let deadline = Instant::now() + TokioDuration::from_secs(1);
        loop {
            let blocked: bool =
                sqlx::query_scalar("SELECT cardinality(pg_blocking_pids($1))>0")
            .bind(worker_pid)
            .fetch_one(&pools.owner)
            .await?;
            if blocked {
                break;
            }
            assert!(Instant::now() < deadline, "catalog finalizer did not block on library lock");
            sleep(TokioDuration::from_millis(10)).await;
        }

        let late_finalize: bool = sqlx::query_scalar(
            "SELECT folioharbor.upload_finalize_authorized($1,$2,$3,$4,$5,$6,$7,$8)",
        )
        .bind(late_upload.as_uuid())
        .bind(library.as_uuid())
        .bind(actor.as_uuid())
        .bind(23_i64)
        .bind(blob_key(blob, 23))
        .bind("unused-staging")
        .bind(uuid::Uuid::now_v7())
        .bind(now)
        .fetch_one(&mut *api)
        .await?;
        assert!(!late_finalize, "an importing upload cannot be finalized by the API");
        api.rollback().await?;
        let catalog_outcome = worker.await??;
        assert_eq!(catalog_outcome, "duplicate");
        anyhow::Ok(())
    })
    .await??;

    let final_state: (String, i64, i64, String) = sqlx::query_as(
        "SELECT upload.state,library.quota_used_bytes,library.quota_reserved_bytes,reservation.state FROM folioharbor.upload_sessions upload JOIN folioharbor.libraries library USING(library_id) JOIN folioharbor.quota_reservations reservation USING(upload_id) WHERE upload.upload_id=$1",
    )
    .bind(late_upload.as_uuid())
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(final_state, ("duplicate".into(), 23, 0, "released".into()));
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn schema_rejects_ambiguous_catalog_relations_and_ordering() -> anyhow::Result<()> {
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
    seed_user(&pools, actor, now).await?;
    seed_library(&pools, library, actor, now).await?;
    let blob = seed_blob(&pools, 17, now).await?;
    let upload = seed_upload(&pools, library, actor, blob, 17, now).await?;
    let repository = PgCatalogRepository::new(pools.worker.clone());
    let _ = ImportPublicationCatalog::new(&repository)
        .execute(command(library, upload, actor, blob, 17, "Constraints"))
        .await?;
    let (manifestation, package): (uuid::Uuid, uuid::Uuid) = sqlx::query_as(
        "SELECT manifestation_id,package_id FROM folioharbor.publication_packages WHERE blob_id=$1",
    )
    .bind(blob.as_uuid())
    .fetch_one(&pools.owner)
    .await?;

    assert!(
        sqlx::query("INSERT INTO folioharbor.holdings(holding_id,library_id,manifestation_id,state,created_at) VALUES($1,$2,$3,'active',$4)")
            .bind(HoldingId::new().as_uuid()).bind(library.as_uuid()).bind(manifestation).bind(now)
            .execute(&pools.owner).await.is_err(),
        "one library cannot have two active Holdings for one Manifestation"
    );
    assert!(
        sqlx::query("INSERT INTO folioharbor.items(item_id,holding_id,manifestation_id,package_id,state,created_at) VALUES($1,$2,$3,$4,'active',$5)")
            .bind(ItemId::new().as_uuid()).bind(HoldingId::new().as_uuid()).bind(manifestation).bind(package).bind(now)
            .execute(&pools.owner).await.is_err(),
        "an Item must reference exactly one existing Holding"
    );
    assert!(
        sqlx::query("INSERT INTO folioharbor.publication_packages(package_id,manifestation_id,blob_id,parser_profile_version,created_at) VALUES($1,$2,$3,'epub-v1',$4)")
            .bind(uuid::Uuid::now_v7()).bind(manifestation).bind(blob.as_uuid()).bind(now)
            .execute(&pools.owner).await.is_err(),
        "blob and parser profile identify at most one package"
    );
    assert!(
        sqlx::query("INSERT INTO folioharbor.publication_resources(package_id,resource_order,normalized_href,media_type,source_blob_id) VALUES($1,99,'OPS/chapter.xhtml','application/xhtml+xml',$2)")
            .bind(package).bind(blob.as_uuid()).execute(&pools.owner).await.is_err(),
        "normalized hrefs are unique within a package"
    );
    assert!(
        sqlx::query("INSERT INTO folioharbor.package_toc_entries(package_id,toc_order,label,locator_href) VALUES($1,-1,'Invalid','OPS/chapter.xhtml')")
            .bind(package).execute(&pools.owner).await.is_err(),
        "ordering keys cannot be negative"
    );
    let second_unit = ContentUnitId::new();
    sqlx::query("INSERT INTO folioharbor.content_units(content_unit_id,package_id,locator_href,created_at) VALUES($1,$2,'OPS/other.xhtml',$3)")
        .bind(second_unit.as_uuid()).bind(package).bind(now).execute(&pools.owner).await?;
    assert!(
        sqlx::query("INSERT INTO folioharbor.manifestation_units(manifestation_id,package_id,content_unit_id,spine_order,linear) VALUES($1,$2,$3,0,true)")
            .bind(manifestation).bind(package).bind(second_unit.as_uuid()).execute(&pools.owner).await.is_err(),
        "spine ordering is unique within a Manifestation"
    );
    assert!(
        sqlx::query("DELETE FROM folioharbor.blobs WHERE blob_id=$1")
            .bind(blob.as_uuid())
            .execute(&pools.owner)
            .await
            .is_err(),
        "catalog relations never cascade deletion to shared Blob bytes"
    );
    let other_blob = seed_blob(&pools, 18, now).await?;
    let other_upload = seed_upload(&pools, library, actor, other_blob, 18, now).await?;
    let _ = ImportPublicationCatalog::new(&repository)
        .execute(command(
            library,
            other_upload,
            actor,
            other_blob,
            18,
            "Other aggregate",
        ))
        .await?;
    let foreign_unit: uuid::Uuid = sqlx::query_scalar(
        "SELECT unit.content_unit_id FROM folioharbor.content_units unit JOIN folioharbor.publication_packages package USING(package_id) WHERE package.blob_id=$1",
    )
    .bind(other_blob.as_uuid())
    .fetch_one(&pools.owner)
    .await?;
    assert!(
        sqlx::query("INSERT INTO folioharbor.manifestation_units(manifestation_id,package_id,content_unit_id,spine_order,linear) VALUES($1,$2,$3,99,true)")
            .bind(manifestation).bind(package).bind(foreign_unit).execute(&pools.owner).await.is_err(),
        "a spine cannot reference a ContentUnit from another package aggregate"
    );
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn assert_rejected_completion(
    pools: &PgPools,
    library: LibraryId,
    upload: UploadId,
    actor: UserId,
    blob: BlobId,
    bytes: i64,
    item: ItemId,
    now: OffsetDateTime,
) -> anyhow::Result<()> {
    let before: (String, i64, i64, String, i64, i64, i64) = sqlx::query_as(
        "SELECT upload.state,library.quota_used_bytes,library.quota_reserved_bytes,reservation.state,(SELECT count(*) FROM folioharbor.items),(SELECT count(*) FROM folioharbor.holdings),(SELECT count(*) FROM folioharbor.audit_events WHERE resource_id=$1 AND action_code='publication.import') FROM folioharbor.upload_sessions upload JOIN folioharbor.libraries library USING(library_id) JOIN folioharbor.quota_reservations reservation USING(upload_id) WHERE upload.upload_id=$1",
    )
    .bind(upload.as_uuid())
    .fetch_one(&pools.owner)
    .await?;
    let request = RequestId::new();
    let mut tx = pools.worker.begin().await?;
    PgTransactionContext::apply(&mut tx, &DatabaseContext::worker(request, Some(library))).await?;
    let outcome: String = sqlx::query_scalar(
        "SELECT folioharbor.catalog_finish_import($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
    )
    .bind(library.as_uuid())
    .bind(upload.as_uuid())
    .bind(actor.as_uuid())
    .bind(blob.as_uuid())
    .bind(bytes)
    .bind("epub-v1")
    .bind(item.as_uuid())
    .bind(uuid::Uuid::now_v7())
    .bind(request.as_ulid().to_string())
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    assert_eq!(outcome, "not_active");
    let after: (String, i64, i64, String, i64, i64, i64) = sqlx::query_as(
        "SELECT upload.state,library.quota_used_bytes,library.quota_reserved_bytes,reservation.state,(SELECT count(*) FROM folioharbor.items),(SELECT count(*) FROM folioharbor.holdings),(SELECT count(*) FROM folioharbor.audit_events WHERE resource_id=$1 AND action_code='publication.import') FROM folioharbor.upload_sessions upload JOIN folioharbor.libraries library USING(library_id) JOIN folioharbor.quota_reservations reservation USING(upload_id) WHERE upload.upload_id=$1",
    )
    .bind(upload.as_uuid())
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(after, before, "rejected completion must be mutation-free");
    Ok(())
}

fn command(
    library_id: LibraryId,
    upload_id: UploadId,
    actor_id: UserId,
    blob_id: BlobId,
    bytes: u64,
    title: &str,
) -> ImportCatalogCommand {
    command_with_toc_locator(
        library_id,
        upload_id,
        actor_id,
        blob_id,
        bytes,
        title,
        "OPS/chapter.xhtml",
    )
}

fn command_with_toc_locator(
    library_id: LibraryId,
    upload_id: UploadId,
    actor_id: UserId,
    blob_id: BlobId,
    bytes: u64,
    title: &str,
    toc_locator: &str,
) -> ImportCatalogCommand {
    let metadata = CatalogMetadata::from_parser(&ParserMetadata {
        titles: vec![title.into()],
        authors: vec!["Author".into()],
        languages: vec!["en".into()],
        identifiers: vec!["id".into()],
    })
    .expect("metadata");
    let publication = CatalogPublication::from_parser(
        metadata,
        vec![
            PublicationResource::new("OPS/chapter.xhtml", "application/xhtml+xml")
                .expect("resource"),
            PublicationResource::new("OPS/cover.jpg", "image/jpeg").expect("cover resource"),
        ],
        vec![SpineEntry::new("OPS/chapter.xhtml", true).expect("spine")],
        vec![TocEntry::new("Chapter", toc_locator).expect("toc")],
        Some("OPS/cover.jpg".to_owned()),
    )
    .expect("publication");
    ImportCatalogCommand {
        library_id,
        upload_id,
        actor_id,
        original_blob_id: blob_id,
        logical_bytes: ByteCount::new(bytes),
        parser_profile_version: "epub-v1".into(),
        publication,
        request_id: RequestId::new(),
        now: OffsetDateTime::now_utc(),
    }
}

async fn seed_user(pools: &PgPools, actor: UserId, now: OffsetDateTime) -> anyhow::Result<()> {
    sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,$2,$2,'verified',$3,$3) ON CONFLICT DO NOTHING").bind(actor.as_uuid()).bind(format!("{}@test.invalid", actor.as_uuid())).bind(now).execute(&pools.owner).await?;
    Ok(())
}

async fn seed_library(
    pools: &PgPools,
    library: LibraryId,
    actor: UserId,
    now: OffsetDateTime,
) -> anyhow::Result<()> {
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,created_at,updated_at) VALUES($1,'Catalog',$2,$2)").bind(library.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.library_memberships(library_id,user_id,role_code,status,joined_at) VALUES($1,$2,'editor','active',$3)").bind(library.as_uuid()).bind(actor.as_uuid()).bind(now).execute(&pools.owner).await?;
    Ok(())
}

async fn seed_blob(pools: &PgPools, bytes: i64, now: OffsetDateTime) -> anyhow::Result<BlobId> {
    let blob = BlobId::new();
    let mut digest = Vec::with_capacity(32);
    while digest.len() < 32 {
        digest.extend_from_slice(blob.as_uuid().as_bytes());
    }
    sqlx::query("INSERT INTO folioharbor.blobs(blob_id,storage_namespace,sha256,byte_size,created_at) VALUES($1,'instance-v1',$2,$3,$4)").bind(blob.as_uuid()).bind(digest).bind(bytes).bind(now).execute(&pools.owner).await?;
    Ok(blob)
}

async fn seed_upload(
    pools: &PgPools,
    library: LibraryId,
    actor: UserId,
    blob: BlobId,
    bytes: i64,
    now: OffsetDateTime,
) -> anyhow::Result<UploadId> {
    let upload = UploadId::new();
    sqlx::query("INSERT INTO folioharbor.quota_reservations(upload_id,library_id,reserved_bytes,expires_at,state) VALUES($1,$2,$3,$4,'active')").bind(upload.as_uuid()).bind(library.as_uuid()).bind(bytes).bind(now + Duration::hours(1)).execute(&pools.owner).await?;
    sqlx::query("UPDATE folioharbor.libraries SET quota_reserved_bytes=quota_reserved_bytes+$2 WHERE library_id=$1").bind(library.as_uuid()).bind(bytes).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.upload_sessions(upload_id,library_id,created_by,file_name,media_type,declared_bytes,state,dedup_scope,received_bytes,sha256,storage_key,created_at,updated_at,expires_at) SELECT $1,$2,$3,'book.epub','application/epub+zip',$4,$5,'instance',$4,sha256,$9,$6,$6,$7 FROM folioharbor.blobs WHERE blob_id=$8").bind(upload.as_uuid()).bind(library.as_uuid()).bind(actor.as_uuid()).bind(bytes).bind(UploadState::Importing.as_str()).bind(now).bind(now + Duration::hours(1)).bind(blob.as_uuid()).bind(blob_key(blob, bytes)).execute(&pools.owner).await?;
    Ok(upload)
}

fn blob_key(blob: BlobId, bytes: i64) -> String {
    let digest = blob.as_uuid().simple().to_string().repeat(2);
    format!("blob:instance-v1:{digest}:{bytes}")
}
