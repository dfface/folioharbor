#![allow(clippy::expect_used, clippy::too_many_lines)]

use folioharbor_domain::id::{
    BlobId, ExpressionId, HoldingId, ItemId, LibraryId, ManifestationId, PublicationPackageId,
    RequestId, UploadId, UserId, WorkId,
};
use folioharbor_postgres::{
    DatabaseContext, PgAuthorizationRepository, PgCatalogRepository, PgPools,
    PgReaderCatalogRepository, PgTransactionContext, run_migrations,
};
use folioharbor_test_support::postgres::TestPostgres;
use sqlx::PgPool;
use std::sync::Arc;
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
    let index_definition: String = sqlx::query_scalar(
        "SELECT indexdef FROM pg_indexes WHERE schemaname='folioharbor' AND indexname='holdings_active_library_keyset_idx'",
    )
    .fetch_one(&pools.owner)
    .await?;
    assert!(index_definition.contains("(library_id, holding_id DESC)"));
    assert!(index_definition.contains("WHERE (state = 'active'::text)"));
    let canonical_index: String = sqlx::query_scalar(
        "SELECT indexdef FROM pg_indexes WHERE schemaname='folioharbor' AND indexname='items_active_holding_canonical_idx'",
    )
    .fetch_one(&pools.owner)
    .await?;
    assert!(canonical_index.contains("(holding_id, created_at DESC, item_id DESC)"));
    assert!(canonical_index.contains("WHERE (state = 'active'::text)"));
    let (security_definer, settings): (bool, Option<Vec<String>>) = sqlx::query_as(
        "SELECT prosecdef,proconfig FROM pg_proc JOIN pg_namespace ON pg_namespace.oid=pronamespace WHERE nspname='folioharbor' AND proname='catalog_item_projection_visible'",
    )
    .fetch_one(&pools.owner)
    .await?;
    assert!(security_definer);
    assert!(settings.is_some_and(|values| values.iter().any(|value| value == "search_path=\"\"")));
    let api_can_execute: bool = sqlx::query_scalar(
        "SELECT has_function_privilege('folioharbor_api','folioharbor.catalog_item_projection_visible(uuid,uuid,uuid,bigint)','EXECUTE')",
    )
    .fetch_one(&pools.owner)
    .await?;
    let worker_can_execute: bool = sqlx::query_scalar(
        "SELECT has_function_privilege('folioharbor_worker','folioharbor.catalog_item_projection_visible(uuid,uuid,uuid,bigint)','EXECUTE')",
    )
    .fetch_one(&pools.owner)
    .await?;
    assert!(api_can_execute);
    assert!(!worker_can_execute);
    let (reader_security_definer, reader_settings): (bool, Option<Vec<String>>) =
        sqlx::query_as(
            "SELECT prosecdef,proconfig FROM pg_proc JOIN pg_namespace ON pg_namespace.oid=pronamespace WHERE nspname='folioharbor' AND proname='reader_publication_visible'",
        )
        .fetch_one(&pools.owner)
        .await?;
    assert!(reader_security_definer);
    assert!(
        reader_settings
            .is_some_and(|values| values.iter().any(|value| value == "search_path=\"\""))
    );
    let api_can_read: bool = sqlx::query_scalar(
        "SELECT has_function_privilege('folioharbor_api','folioharbor.reader_publication_visible(uuid,uuid)','EXECUTE')",
    )
    .fetch_one(&pools.owner)
    .await?;
    let worker_can_read: bool = sqlx::query_scalar(
        "SELECT has_function_privilege('folioharbor_worker','folioharbor.reader_publication_visible(uuid,uuid)','EXECUTE')",
    )
    .fetch_one(&pools.owner)
    .await?;
    assert!(api_can_read);
    assert!(!worker_can_read);
    let public_can_read: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM pg_proc function JOIN pg_namespace namespace ON namespace.oid=function.pronamespace CROSS JOIN LATERAL aclexplode(function.proacl) privilege WHERE namespace.nspname='folioharbor' AND function.proname='reader_publication_visible' AND privilege.grantee=0 AND privilege.privilege_type='EXECUTE')",
    )
    .fetch_one(&pools.owner)
    .await?;
    assert!(!public_can_read);
    let reader_definition: String = sqlx::query_scalar(
        "SELECT pg_get_functiondef(function.oid) FROM pg_proc function JOIN pg_namespace namespace ON namespace.oid=function.pronamespace WHERE namespace.nspname='folioharbor' AND function.proname='reader_publication_visible'",
    )
    .fetch_one(&pools.owner)
    .await?;
    assert!(reader_definition.contains("JOIN LATERAL"));
    assert!(reader_definition.contains("ORDER BY candidate.storage_key"));
    let (access_security_definer, access_settings): (bool, Option<Vec<String>>) = sqlx::query_as(
        "SELECT prosecdef,proconfig FROM pg_proc JOIN pg_namespace ON pg_namespace.oid=pronamespace WHERE nspname='folioharbor' AND proname='reader_item_read_access'",
    )
    .fetch_one(&pools.owner)
    .await?;
    assert!(access_security_definer);
    assert!(
        access_settings
            .is_some_and(|values| values.iter().any(|value| value == "search_path=\"\""))
    );
    let api_can_access: bool = sqlx::query_scalar("SELECT has_function_privilege('folioharbor_api','folioharbor.reader_item_read_access(uuid,uuid)','EXECUTE')").fetch_one(&pools.owner).await?;
    let worker_can_access: bool = sqlx::query_scalar("SELECT has_function_privilege('folioharbor_worker','folioharbor.reader_item_read_access(uuid,uuid)','EXECUTE')").fetch_one(&pools.owner).await?;
    assert!(api_can_access);
    assert!(!worker_can_access);
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
    let stable_storage_key = "blob:000-reader-copy";
    sqlx::query("INSERT INTO folioharbor.blob_locations(blob_id,storage_key,state,created_at,updated_at) VALUES($1,$2,'ready',$3,$3)")
        .bind(blob.as_uuid())
        .bind(stable_storage_key)
        .bind(now)
        .execute(&pools.owner)
        .await?;
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
    let reader_catalog = Arc::new(PgReaderCatalogRepository::new(pools.api.clone()));
    let grant = Authorization::new(&PgAuthorizationRepository::new(pools.api.clone()))
        .require(allowed, Action::ViewLibrary, ResourceRef::Library(library))
        .await?;
    let listed = catalog
        .list_visible_items(grant, library, None, 10, RequestId::new())
        .await?;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].authors, ["Ursula Writer"]);
    assert_eq!(listed[0].languages, ["fr"]);
    assert_eq!(listed[0].identifiers, ["isbn:978-test"]);
    assert_eq!(listed[0].media_type, "application/octet-stream");
    let visible = catalog
        .find_visible_item(grant, library, item, RequestId::new())
        .await?
        .expect("authorized Item must resolve from its Holding");
    assert_eq!(visible.item_id, item);
    assert_eq!(visible.package_id, intended_package);
    assert_eq!(visible.primary_title, "Visible work");
    assert_eq!(visible.authors, ["Ursula Writer"]);
    assert_eq!(visible.languages, ["fr"]);
    assert_eq!(visible.identifiers, ["isbn:978-test"]);
    assert_eq!(visible.media_type, "application/octet-stream");
    sqlx::query(
        "INSERT INTO folioharbor.publication_resources(package_id,resource_order,normalized_href,media_type,source_blob_id) SELECT $1,number,format('OPS/resource-%s.css',number),'text/css',$2 FROM generate_series(1,4095) number",
    )
    .bind(intended_package.as_uuid())
    .bind(blob.as_uuid())
    .execute(&pools.owner)
    .await?;
    let barrier = Arc::new(tokio::sync::Barrier::new(9));
    let mut reader_tasks = Vec::new();
    for _ in 0..8 {
        let barrier = barrier.clone();
        let reader_catalog = reader_catalog.clone();
        reader_tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            reader_catalog
                .find_readable_publication(allowed, item, RequestId::new())
                .await
        }));
    }
    barrier.wait().await;
    let mut readable = None;
    for task in reader_tasks {
        readable = task.await.expect("reader task")?.or(readable);
    }
    let readable = readable.expect("active member can read the package projection");
    assert_eq!(readable.manifestation_id, manifestation);
    assert_eq!(readable.package_id, intended_package);
    assert_eq!(readable.resources[0].normalized_href, "OPS/chapter.xhtml");
    assert_eq!(
        readable.reading_order[0].normalized_href,
        "OPS/chapter.xhtml"
    );
    assert_eq!(readable.toc[0].label, "Chapter");
    assert_eq!(readable.storage_key.as_str(), stable_storage_key);
    assert_eq!(readable.resources.len(), 4096);
    let cache_metrics = reader_catalog.cache_metrics();
    assert_eq!(cache_metrics.entries, 1);
    assert!(cache_metrics.bytes > 0);
    assert_eq!(cache_metrics.access_checks, 8);
    assert_eq!(cache_metrics.projection_loads, 1);
    assert_eq!(cache_metrics.inflight_loads, 0);

    sqlx::query(
        "INSERT INTO folioharbor.roles(role_code,display_name) VALUES('reader-test','Reader test')",
    )
    .execute(&pools.owner)
    .await?;
    sqlx::query(
        "INSERT INTO folioharbor.role_permissions(role_code,permission_code) VALUES('reader-test','holding.view')",
    )
    .execute(&pools.owner)
    .await?;
    sqlx::query("UPDATE folioharbor.library_memberships SET role_code='reader-test',version=version+1 WHERE library_id=$1 AND user_id=$2")
        .bind(library.as_uuid()).bind(allowed.as_uuid()).execute(&pools.owner).await?;
    assert!(
        reader_catalog
            .find_readable_publication(allowed, item, RequestId::new())
            .await?
            .is_none(),
        "holding.view without item.read must not authorize EPUB reading"
    );
    sqlx::query("DELETE FROM folioharbor.role_permissions WHERE role_code='reader-test'")
        .execute(&pools.owner)
        .await?;
    sqlx::query(
        "INSERT INTO folioharbor.role_permissions(role_code,permission_code) VALUES('reader-test','item.read')",
    )
    .execute(&pools.owner)
    .await?;
    assert!(
        reader_catalog
            .find_readable_publication(allowed, item, RequestId::new())
            .await?
            .is_some(),
        "item.read must authorize independently of holding.view"
    );
    sqlx::query("DELETE FROM folioharbor.role_permissions WHERE role_code='reader-test' AND permission_code='item.read'")
        .execute(&pools.owner)
        .await?;
    assert!(
        reader_catalog
            .find_readable_publication(allowed, item, RequestId::new())
            .await?
            .is_none(),
        "revoking item.read must stop the next reader request"
    );
    sqlx::query("UPDATE folioharbor.library_memberships SET status='removed',removed_at=$3,version=version+1 WHERE library_id=$1 AND user_id=$2")
        .bind(library.as_uuid()).bind(allowed.as_uuid()).bind(now).execute(&pools.owner).await?;
    assert!(
        reader_catalog
            .find_readable_publication(allowed, item, RequestId::new())
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
    sqlx::query("INSERT INTO folioharbor.works(work_id,primary_title,authors,created_at) VALUES($1,'Visible work',ARRAY['Ursula Writer']::text[],$2)").bind(work.as_uuid()).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.expressions(expression_id,work_id,languages,created_at) VALUES($1,$2,ARRAY['fr'],$3)").bind(expression.as_uuid()).bind(work.as_uuid()).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.manifestations(manifestation_id,identifiers,created_at) VALUES($1,ARRAY['isbn:978-test']::text[],$2)").bind(manifestation.as_uuid()).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.manifestation_expressions(manifestation_id,expression_id,expression_order) VALUES($1,$2,0)").bind(manifestation.as_uuid()).bind(expression.as_uuid()).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.blobs(blob_id,storage_namespace,sha256,byte_size,created_at) VALUES($1,'instance-v1',$2,1,$3)").bind(blob.as_uuid()).bind(vec![7_u8; 32]).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.blob_locations(blob_id,storage_key,state,created_at,updated_at) VALUES($1,$2,'ready',$3,$3)")
        .bind(blob.as_uuid()).bind(format!("blob:instance-v1:{}:1", "07".repeat(32))).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.upload_sessions(upload_id,library_id,created_by,file_name,media_type,declared_bytes,dedup_scope,received_bytes,state,storage_key,sha256,expires_at,created_at,updated_at) VALUES($1,$2,$3,'visible.epub','application/octet-stream',1,'instance',1,'ready',$4,$5,$6,$6,$6)")
        .bind(upload.as_uuid()).bind(library.as_uuid()).bind(actor.as_uuid())
        .bind(format!("blob:instance-v1:{}:1", "07".repeat(32))).bind(vec![7_u8; 32]).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.publication_packages(package_id,manifestation_id,blob_id,parser_profile_version,created_at) VALUES($1,$2,$3,'epub-v1',$4)").bind(package.as_uuid()).bind(manifestation.as_uuid()).bind(blob.as_uuid()).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.publication_resources(package_id,resource_order,normalized_href,media_type,source_blob_id) VALUES($1,0,'OPS/chapter.xhtml','application/xhtml+xml',$2)")
        .bind(package.as_uuid()).bind(blob.as_uuid()).execute(pool).await?;
    let unit = folioharbor_domain::id::ContentUnitId::new();
    sqlx::query("INSERT INTO folioharbor.content_units(content_unit_id,package_id,locator_href,created_at) VALUES($1,$2,'OPS/chapter.xhtml',$3)")
        .bind(unit.as_uuid()).bind(package.as_uuid()).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.manifestation_units(manifestation_id,package_id,content_unit_id,spine_order,linear) VALUES($1,$2,$3,0,true)")
        .bind(manifestation.as_uuid()).bind(package.as_uuid()).bind(unit.as_uuid()).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.package_toc_entries(package_id,toc_order,label,locator_href) VALUES($1,0,'Chapter','OPS/chapter.xhtml#start')")
        .bind(package.as_uuid()).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.holdings(holding_id,library_id,manifestation_id,state,created_at) VALUES($1,$2,$3,'active',$4)").bind(holding.as_uuid()).bind(library.as_uuid()).bind(manifestation.as_uuid()).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.items(item_id,holding_id,manifestation_id,package_id,source_upload_id,state,created_at) VALUES($1,$2,$3,$4,$5,'active',$6)").bind(item.as_uuid()).bind(holding.as_uuid()).bind(manifestation.as_uuid()).bind(package.as_uuid()).bind(upload.as_uuid()).bind(now).execute(pool).await?;
    Ok((item, package, manifestation, blob))
}
use folioharbor_application::{
    authorization::{Action, Authorization, ResourceRef},
    ports::{CatalogQueryRepository, ReaderCatalogRepository},
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
    let (_, _, _, _, holding) =
        seed_named_item(&pools.owner, library, actor, now, 31, "One Holding").await?;
    sqlx::query("DROP INDEX folioharbor.items_one_active_per_holding")
        .execute(&pools.owner)
        .await?;
    let selected = ItemId::from_uuid(uuid::Uuid::nil());
    let selected = insert_additional_active_item(
        &pools.owner,
        holding,
        selected,
        now + time::Duration::seconds(1),
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
        "latest Item creation time wins deterministically"
    );
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

async fn insert_additional_active_item(
    pool: &PgPool,
    holding: HoldingId,
    item: ItemId,
    now: OffsetDateTime,
) -> anyhow::Result<ItemId> {
    let (library, actor, package, manifestation): (uuid::Uuid, uuid::Uuid, uuid::Uuid, uuid::Uuid) =
        sqlx::query_as(
            "SELECT holding.library_id,upload.created_by,item.package_id,item.manifestation_id FROM folioharbor.holdings holding JOIN folioharbor.items item USING(holding_id) JOIN folioharbor.upload_sessions upload ON upload.upload_id=item.source_upload_id WHERE holding.holding_id=$1",
        )
        .bind(holding.as_uuid())
        .fetch_one(pool)
        .await?;
    let upload = UploadId::new();
    let sha = vec![32_u8; 32];
    sqlx::query("INSERT INTO folioharbor.upload_sessions(upload_id,library_id,created_by,file_name,media_type,declared_bytes,dedup_scope,received_bytes,state,storage_key,sha256,expires_at,created_at,updated_at) VALUES($1,$2,$3,'second.epub','application/epub+zip',1,'instance',1,'ready',$4,$5,$6,$6,$6)")
        .bind(upload.as_uuid()).bind(library).bind(actor)
        .bind(format!("blob:instance-v1:{}:1", "20".repeat(32))).bind(sha).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.items(item_id,holding_id,manifestation_id,package_id,source_upload_id,state,created_at) VALUES($1,$2,$3,$4,$5,'active',$6)")
        .bind(item.as_uuid()).bind(holding.as_uuid()).bind(manifestation)
        .bind(package).bind(upload.as_uuid()).bind(now).execute(pool).await?;
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
    let holding = HoldingId::from_uuid(uuid::Uuid::from_u128(u128::from(seed)));
    let item = ItemId::from_uuid(uuid::Uuid::from_u128(1_000 + u128::from(seed)));
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
