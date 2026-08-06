#![allow(clippy::expect_used, clippy::too_many_lines)]

use folioharbor_domain::id::{
    BlobId, ExpressionId, HoldingId, ItemId, LibraryId, ManifestationId, PublicationPackageId,
    RequestId, UserId, WorkId,
};
use folioharbor_postgres::{
    DatabaseContext, PgCatalogRepository, PgPools, PgTransactionContext, run_migrations,
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
        seed_item(&pools.owner, library, now).await?;
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
    let visible = catalog
        .find_visible_item(allowed, library, item, version, RequestId::new())
        .await?
        .expect("authorized Item must resolve from its Holding");
    assert_eq!(visible.item_id, item);
    assert_eq!(visible.package_id, intended_package);
    assert_eq!(visible.primary_title, "Visible work");
    assert!(
        catalog
            .find_visible_item(outsider, library, item, version, RequestId::new())
            .await?
            .is_none()
    );
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
    now: OffsetDateTime,
) -> anyhow::Result<(ItemId, PublicationPackageId, ManifestationId, BlobId)> {
    let work = WorkId::new();
    let expression = ExpressionId::new();
    let manifestation = ManifestationId::new();
    let blob = BlobId::new();
    let package = PublicationPackageId::new();
    let holding = HoldingId::new();
    let item = ItemId::new();
    sqlx::query("INSERT INTO folioharbor.works(work_id,primary_title,authors,created_at) VALUES($1,'Visible work',ARRAY[]::text[],$2)").bind(work.as_uuid()).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.expressions(expression_id,work_id,languages,created_at) VALUES($1,$2,ARRAY['en'],$3)").bind(expression.as_uuid()).bind(work.as_uuid()).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.manifestations(manifestation_id,identifiers,created_at) VALUES($1,ARRAY[]::text[],$2)").bind(manifestation.as_uuid()).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.manifestation_expressions(manifestation_id,expression_id,expression_order) VALUES($1,$2,0)").bind(manifestation.as_uuid()).bind(expression.as_uuid()).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.blobs(blob_id,storage_namespace,sha256,byte_size,created_at) VALUES($1,'instance-v1',$2,1,$3)").bind(blob.as_uuid()).bind(vec![7_u8; 32]).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.publication_packages(package_id,manifestation_id,blob_id,parser_profile_version,created_at) VALUES($1,$2,$3,'epub-v1',$4)").bind(package.as_uuid()).bind(manifestation.as_uuid()).bind(blob.as_uuid()).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.holdings(holding_id,library_id,manifestation_id,state,created_at) VALUES($1,$2,$3,'active',$4)").bind(holding.as_uuid()).bind(library.as_uuid()).bind(manifestation.as_uuid()).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.items(item_id,holding_id,manifestation_id,package_id,state,created_at) VALUES($1,$2,$3,$4,'active',$5)").bind(item.as_uuid()).bind(holding.as_uuid()).bind(manifestation.as_uuid()).bind(package.as_uuid()).bind(now).execute(pool).await?;
    Ok((item, package, manifestation, blob))
}
use folioharbor_application::ports::CatalogQueryRepository;
