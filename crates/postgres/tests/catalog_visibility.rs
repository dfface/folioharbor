#![allow(clippy::expect_used, clippy::too_many_lines)]

use folioharbor_domain::id::{
    BlobId, ExpressionId, HoldingId, ItemId, LibraryId, ManifestationId, PublicationPackageId,
    RequestId, UploadId, UserId, WorkId,
};
use folioharbor_postgres::{
    DatabaseContext, PgAuthorizationRepository, PgCatalogRepository, PgPools, PgTransactionContext,
    run_migrations,
};
use folioharbor_test_support::postgres::TestPostgres;
use sqlx::PgPool;
use time::OffsetDateTime;

#[tokio::test]
async fn api_catalog_access_starts_from_visible_items_and_global_tables_are_not_enumerable()
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
    let allowed = UserId::new();
    let outsider = UserId::new();
    let library = LibraryId::new();
    for (user, email) in [
        (allowed, "allowed@test.invalid"),
        (outsider, "outsider@test.invalid"),
    ] {
        sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,$2,$2,'verified',$3,$3)").bind(user.as_uuid()).bind(email).bind(now).execute(&pools.owner).await?;
    }
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,created_at,updated_at) VALUES($1,'Visible',$2,$2)").bind(library.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.library_memberships(library_id,user_id,role_code,status,joined_at) VALUES($1,$2,'reader','active',$3)").bind(library.as_uuid()).bind(allowed.as_uuid()).bind(now).execute(&pools.owner).await?;
    let (item, intended_package, manifestation, blob) =
        seed_item(&pools.owner, library, allowed, now).await?;
    sqlx::query("INSERT INTO folioharbor.publication_packages(package_id,manifestation_id,blob_id,parser_profile_version,created_at) VALUES($1,$2,$3,'epub-v2',$4)")
        .bind(PublicationPackageId::new().as_uuid()).bind(manifestation.as_uuid()).bind(blob.as_uuid()).bind(now).execute(&pools.owner).await?;
    let associated_package: uuid::Uuid =
        sqlx::query_scalar("SELECT package_id FROM folioharbor.items WHERE item_id=$1")
            .bind(item.as_uuid())
            .fetch_one(&pools.owner)
            .await?;
    assert_eq!(associated_package, intended_package.as_uuid());

    for context in [
        DatabaseContext::api(allowed, library, RequestId::new()),
        DatabaseContext::api(outsider, library, RequestId::new()),
    ] {
        for table in [
            "works",
            "expressions",
            "manifestations",
            "publication_packages",
        ] {
            let mut tx = pools.api.begin().await?;
            PgTransactionContext::apply(&mut tx, &context).await?;
            let result = sqlx::query(&format!("SELECT count(*) FROM folioharbor.{table}"))
                .fetch_one(&mut *tx)
                .await;
            assert!(result.is_err(), "API must not enumerate {table}");
            tx.rollback().await?;
        }
    }
    assert_eq!(
        visible_items(
            &pools.api,
            Some(DatabaseContext::api(allowed, library, RequestId::new()))
        )
        .await?,
        1
    );
    assert_eq!(
        visible_items(
            &pools.api,
            Some(DatabaseContext::api(outsider, library, RequestId::new()))
        )
        .await?,
        0
    );
    assert_eq!(visible_items(&pools.api, None).await?, 0);
    assert_eq!(
        visible_items(
            &pools.worker,
            Some(DatabaseContext::worker(RequestId::new(), Some(library)))
        )
        .await?,
        1
    );

    let version: i64 = sqlx::query_scalar(
        "SELECT version FROM folioharbor.library_memberships WHERE library_id=$1 AND user_id=$2",
    )
    .bind(library.as_uuid())
    .bind(allowed.as_uuid())
    .fetch_one(&pools.owner)
    .await?;
    let mut tx = pools.api.begin().await?;
    PgTransactionContext::apply(
        &mut tx,
        &DatabaseContext::api(allowed, library, RequestId::new()),
    )
    .await?;
    let visible: Option<uuid::Uuid> =
        sqlx::query_scalar("SELECT item_id FROM folioharbor.catalog_item_visible($1,$2,$3,$4)")
            .bind(allowed.as_uuid())
            .bind(library.as_uuid())
            .bind(item.as_uuid())
            .bind(version)
            .fetch_optional(&mut *tx)
            .await?;
    assert_eq!(visible, Some(item.as_uuid()));
    tx.rollback().await?;
    let catalog = PgCatalogRepository::new(pools.api.clone());
    let grant = Authorization::new(&PgAuthorizationRepository::new(pools.api.clone()))
        .require(allowed, Action::ViewLibrary, ResourceRef::Library(library))
        .await?;
    let visible = catalog
        .find_visible_item(grant, library, item, RequestId::new())
        .await?
        .expect("authorized Item must resolve from its Holding");
    assert_eq!(visible.item_id, item);
    assert_eq!(visible.package_id, intended_package);
    assert_eq!(visible.primary_title, "Visible work");
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

async fn visible_items(pool: &PgPool, context: Option<DatabaseContext>) -> anyhow::Result<i64> {
    let mut tx = pool.begin().await?;
    if let Some(context) = context {
        PgTransactionContext::apply(&mut tx, &context).await?;
    }
    let count = sqlx::query_scalar("SELECT count(*) FROM folioharbor.items")
        .fetch_one(&mut *tx)
        .await?;
    tx.rollback().await?;
    Ok(count)
}

async fn seed_item(
    pool: &PgPool,
    library: LibraryId,
    actor: UserId,
    now: OffsetDateTime,
) -> anyhow::Result<(ItemId, PublicationPackageId, ManifestationId, BlobId)> {
    let work = WorkId::new();
    let expression = ExpressionId::new();
    let manifestation = ManifestationId::new();
    let blob = BlobId::new();
    let package = PublicationPackageId::new();
    let holding = HoldingId::new();
    let item = ItemId::new();
    let upload = UploadId::new();
    sqlx::query("INSERT INTO folioharbor.works(work_id,primary_title,authors,created_at) VALUES($1,'Visible work',ARRAY[]::text[],$2)").bind(work.as_uuid()).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.expressions(expression_id,work_id,languages,created_at) VALUES($1,$2,ARRAY['en'],$3)").bind(expression.as_uuid()).bind(work.as_uuid()).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.manifestations(manifestation_id,identifiers,created_at) VALUES($1,ARRAY[]::text[],$2)").bind(manifestation.as_uuid()).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.manifestation_expressions(manifestation_id,expression_id,expression_order) VALUES($1,$2,0)").bind(manifestation.as_uuid()).bind(expression.as_uuid()).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.blobs(blob_id,storage_namespace,sha256,byte_size,created_at) VALUES($1,'instance-v1',$2,1,$3)").bind(blob.as_uuid()).bind(vec![7_u8; 32]).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.upload_sessions(upload_id,library_id,created_by,file_name,media_type,declared_bytes,dedup_scope,received_bytes,state,storage_key,sha256,expires_at,created_at,updated_at) VALUES($1,$2,$3,'visible.epub','application/epub+zip',1,'instance',1,'ready',$4,$5,$6,$6,$6)")
        .bind(upload.as_uuid()).bind(library.as_uuid()).bind(actor.as_uuid())
        .bind(format!("blob:instance-v1:{}:1", "07".repeat(32))).bind(vec![7_u8; 32]).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.publication_packages(package_id,manifestation_id,blob_id,parser_profile_version,created_at) VALUES($1,$2,$3,'epub-v1',$4)").bind(package.as_uuid()).bind(manifestation.as_uuid()).bind(blob.as_uuid()).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.holdings(holding_id,library_id,manifestation_id,state,created_at) VALUES($1,$2,$3,'active',$4)").bind(holding.as_uuid()).bind(library.as_uuid()).bind(manifestation.as_uuid()).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.items(item_id,holding_id,manifestation_id,package_id,source_upload_id,state,created_at) VALUES($1,$2,$3,$4,$5,'active',$6)").bind(item.as_uuid()).bind(holding.as_uuid()).bind(manifestation.as_uuid()).bind(package.as_uuid()).bind(upload.as_uuid()).bind(now).execute(pool).await?;
    Ok((item, package, manifestation, blob))
}
use folioharbor_application::{
    authorization::{Action, Authorization, ResourceRef},
    ports::CatalogQueryRepository,
};

#[tokio::test]
async fn visible_holding_keyset_is_stable_when_a_newer_item_is_inserted() -> anyhow::Result<()> {
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
    sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,'page@test.invalid','page@test.invalid','verified',$2,$2)")
        .bind(actor.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,created_at,updated_at) VALUES($1,'Page',$2,$2)")
        .bind(library.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.library_memberships(library_id,user_id,role_code,status,joined_at) VALUES($1,$2,'reader','active',$3)")
        .bind(library.as_uuid()).bind(actor.as_uuid()).bind(now).execute(&pools.owner).await?;
    for seed in [11_u8, 12, 13] {
        seed_named_item(
            &pools.owner,
            library,
            actor,
            now,
            seed,
            &format!("Book {seed}"),
        )
        .await?;
    }
    let grant = Authorization::new(&PgAuthorizationRepository::new(pools.api.clone()))
        .require(actor, Action::ViewLibrary, ResourceRef::Library(library))
        .await?;
    let catalog = PgCatalogRepository::new(pools.api.clone());
    let first = catalog
        .list_visible_items(grant, library, None, 2, RequestId::new())
        .await?;
    assert_eq!(first.len(), 2);
    let cursor = first[1].holding_id;
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    let inserted = seed_named_item(&pools.owner, library, actor, now, 14, "Concurrent")
        .await?
        .0;
    let second = catalog
        .list_visible_items(grant, library, Some(cursor), 2, RequestId::new())
        .await?;
    assert_eq!(second.len(), 1);
    assert!(second.iter().all(|row| row.item_id != inserted));
    assert_eq!(
        first
            .iter()
            .chain(&second)
            .map(|row| row.holding_id)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        3
    );
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn catalog_returns_one_deterministic_item_per_holding_even_if_active_item_uniqueness_is_relaxed()
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
    sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,'duplicate@test.invalid','duplicate@test.invalid','verified',$2,$2)")
        .bind(actor.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,created_at,updated_at) VALUES($1,'Duplicate',$2,$2)")
        .bind(library.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.library_memberships(library_id,user_id,role_code,status,joined_at) VALUES($1,$2,'reader','active',$3)")
        .bind(library.as_uuid()).bind(actor.as_uuid()).bind(now).execute(&pools.owner).await?;
    let (_, package, manifestation, _, holding) =
        seed_named_item(&pools.owner, library, actor, now, 31, "One Holding").await?;
    sqlx::query("DROP INDEX folioharbor.items_one_active_per_holding")
        .execute(&pools.owner)
        .await?;
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    let selected = insert_additional_active_item(
        &pools.owner,
        library,
        actor,
        package,
        manifestation,
        holding,
        now,
    )
    .await?;
    let grant = Authorization::new(&PgAuthorizationRepository::new(pools.api.clone()))
        .require(actor, Action::ViewLibrary, ResourceRef::Library(library))
        .await?;
    let rows = PgCatalogRepository::new(pools.api.clone())
        .list_visible_items(grant, library, None, 10, RequestId::new())
        .await?;
    assert_eq!(rows.len(), 1, "a Holding is the list row boundary");
    assert_eq!(
        rows[0].item_id, selected,
        "highest Item UUID wins deterministically"
    );
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

async fn insert_additional_active_item(
    pool: &PgPool,
    library: LibraryId,
    actor: UserId,
    package: PublicationPackageId,
    manifestation: ManifestationId,
    holding: HoldingId,
    now: OffsetDateTime,
) -> anyhow::Result<ItemId> {
    let upload = UploadId::new();
    let item = ItemId::new();
    let sha = vec![32_u8; 32];
    sqlx::query("INSERT INTO folioharbor.upload_sessions(upload_id,library_id,created_by,file_name,media_type,declared_bytes,dedup_scope,received_bytes,state,storage_key,sha256,expires_at,created_at,updated_at) VALUES($1,$2,$3,'second.epub','application/epub+zip',1,'instance',1,'ready',$4,$5,$6,$6,$6)")
        .bind(upload.as_uuid()).bind(library.as_uuid()).bind(actor.as_uuid())
        .bind(format!("blob:instance-v1:{}:1", "20".repeat(32))).bind(sha).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.items(item_id,holding_id,manifestation_id,package_id,source_upload_id,state,created_at) VALUES($1,$2,$3,$4,$5,'active',$6)")
        .bind(item.as_uuid()).bind(holding.as_uuid()).bind(manifestation.as_uuid())
        .bind(package.as_uuid()).bind(upload.as_uuid()).bind(now).execute(pool).await?;
    Ok(item)
}

async fn seed_named_item(
    pool: &PgPool,
    library: LibraryId,
    actor: UserId,
    now: OffsetDateTime,
    seed: u8,
    title: &str,
) -> anyhow::Result<(
    ItemId,
    PublicationPackageId,
    ManifestationId,
    BlobId,
    HoldingId,
)> {
    let work = WorkId::new();
    let expression = ExpressionId::new();
    let manifestation = ManifestationId::new();
    let blob = BlobId::new();
    let package = PublicationPackageId::new();
    let holding = HoldingId::new();
    let item = ItemId::new();
    let upload = UploadId::new();
    let sha = vec![seed; 32];
    sqlx::query("INSERT INTO folioharbor.works(work_id,primary_title,authors,created_at) VALUES($1,$2,ARRAY['Author']::text[],$3)").bind(work.as_uuid()).bind(title).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.expressions(expression_id,work_id,languages,created_at) VALUES($1,$2,ARRAY['en'],$3)").bind(expression.as_uuid()).bind(work.as_uuid()).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.manifestations(manifestation_id,identifiers,created_at) VALUES($1,ARRAY['id']::text[],$2)").bind(manifestation.as_uuid()).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.manifestation_expressions(manifestation_id,expression_id,expression_order) VALUES($1,$2,0)").bind(manifestation.as_uuid()).bind(expression.as_uuid()).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.blobs(blob_id,storage_namespace,sha256,byte_size,created_at) VALUES($1,'instance-v1',$2,1,$3)").bind(blob.as_uuid()).bind(&sha).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.upload_sessions(upload_id,library_id,created_by,file_name,media_type,declared_bytes,dedup_scope,received_bytes,state,storage_key,sha256,expires_at,created_at,updated_at) VALUES($1,$2,$3,'visible.epub','application/epub+zip',1,'instance',1,'ready',$4,$5,$6,$6,$6)")
        .bind(upload.as_uuid()).bind(library.as_uuid()).bind(actor.as_uuid())
        .bind(format!("blob:instance-v1:{}:1", format!("{seed:02x}").repeat(32))).bind(&sha).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.publication_packages(package_id,manifestation_id,blob_id,parser_profile_version,created_at) VALUES($1,$2,$3,'epub-v1',$4)").bind(package.as_uuid()).bind(manifestation.as_uuid()).bind(blob.as_uuid()).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.holdings(holding_id,library_id,manifestation_id,state,created_at) VALUES($1,$2,$3,'active',$4)").bind(holding.as_uuid()).bind(library.as_uuid()).bind(manifestation.as_uuid()).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.items(item_id,holding_id,manifestation_id,package_id,source_upload_id,state,created_at) VALUES($1,$2,$3,$4,$5,'active',$6)").bind(item.as_uuid()).bind(holding.as_uuid()).bind(manifestation.as_uuid()).bind(package.as_uuid()).bind(upload.as_uuid()).bind(now).execute(pool).await?;
    Ok((item, package, manifestation, blob, holding))
}
