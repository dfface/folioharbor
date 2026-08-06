#![allow(clippy::expect_used, clippy::too_many_lines)]

use std::sync::Arc;

use folioharbor_application::ports::{
    ReadingRepository, ReadingRepositoryError, UpdateProgressRecord,
};
use folioharbor_domain::{
    id::{
        BlobId, DeviceId, ExpressionId, HoldingId, ItemId, LibraryId, ManifestationId,
        PublicationPackageId, RequestId, UploadId, UserId, WorkId,
    },
    reader::{LocatorExtensions, LocatorLocations, ReadingUpdateOutcome, ReadiumLocator},
};
use folioharbor_postgres::{
    DatabaseContext, PgPools, PgReadingRepository, PgTransactionContext, run_migrations,
};
use folioharbor_test_support::postgres::TestPostgres;
use time::OffsetDateTime;
use uuid::Uuid;

fn locator(progression: f64) -> ReadiumLocator {
    ReadiumLocator::new(
        "OPS/chapter.xhtml".to_owned(),
        Some("application/xhtml+xml".to_owned()),
        LocatorLocations::new(Some(progression), None, None, Vec::new()).expect("locations"),
        None,
        LocatorExtensions::empty_v1(),
    )
    .expect("locator")
}

fn command(
    actor: UserId,
    manifestation: ManifestationId,
    device_id: DeviceId,
    mutation: Uuid,
    base_version: u64,
    progression: f64,
) -> UpdateProgressRecord {
    UpdateProgressRecord {
        actor,
        manifestation_id: manifestation,
        device_id,
        client_mutation_id: mutation,
        base_version,
        package_id: None,
        content_unit_id: None,
        locator: locator(progression),
        request_id: RequestId::new(),
    }
}

fn global(outcome: &ReadingUpdateOutcome) -> &folioharbor_domain::reader::ReadingProgress {
    match outcome {
        ReadingUpdateOutcome::Updated { global, .. }
        | ReadingUpdateOutcome::Conflict { global, .. } => global,
    }
}
fn device(outcome: &ReadingUpdateOutcome) -> &folioharbor_domain::reader::DeviceReadingState {
    match outcome {
        ReadingUpdateOutcome::Updated { device, .. }
        | ReadingUpdateOutcome::Conflict { device, .. } => device,
    }
}

#[tokio::test]
async fn atomic_progress_sync_is_idempotent_conflict_safe_private_and_retained()
-> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_migrations(&pools.owner).await?;
    for table in [
        "reading_states",
        "device_reading_states",
        "reading_mutations",
    ] {
        let forced:bool=sqlx::query_scalar("SELECT relforcerowsecurity FROM pg_class JOIN pg_namespace ON pg_namespace.oid=pg_class.relnamespace WHERE nspname='folioharbor' AND relname=$1").bind(table).fetch_one(&pools.owner).await?;
        assert!(forced, "{table} must force user RLS");
        let worker_can_read: bool = sqlx::query_scalar(
            "SELECT has_table_privilege('folioharbor_worker',format('folioharbor.%I',$1),'SELECT')",
        )
        .bind(table)
        .fetch_one(&pools.owner)
        .await?;
        assert!(!worker_can_read, "worker must not read {table}");
    }
    let api_can_gate:bool=sqlx::query_scalar("SELECT has_function_privilege('folioharbor_api','folioharbor.progress_manifestation_readable(uuid,uuid)','EXECUTE')").fetch_one(&pools.owner).await?;
    let worker_can_gate:bool=sqlx::query_scalar("SELECT has_function_privilege('folioharbor_worker','folioharbor.progress_manifestation_readable(uuid,uuid)','EXECUTE')").fetch_one(&pools.owner).await?;
    assert!(api_can_gate);
    assert!(!worker_can_gate);
    let now = OffsetDateTime::now_utc();
    let alice = UserId::new();
    let bob = UserId::new();
    let library = LibraryId::new();
    for (user, email) in [
        (alice, "alice-progress@test.invalid"),
        (bob, "bob-progress@test.invalid"),
    ] {
        sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,$2,$2,'verified',$3,$3)").bind(user.as_uuid()).bind(email).bind(now).execute(&pools.owner).await?;
    }
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,created_at,updated_at) VALUES($1,'Progress',$2,$2)").bind(library.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.library_memberships(library_id,user_id,role_code,status,joined_at) VALUES($1,$2,'reader','active',$3)").bind(library.as_uuid()).bind(alice.as_uuid()).bind(now).execute(&pools.owner).await?;
    let manifestation = seed_item(&pools.owner, library, alice, now).await?;
    let device_a = DeviceId::new();
    let device_b = DeviceId::new();
    for (device, name) in [(device_a, "A"), (device_b, "B")] {
        sqlx::query("INSERT INTO folioharbor.user_devices(device_id,user_id,display_name,created_at,last_seen_at) VALUES($1,$2,$3,$4,$4)").bind(device.as_uuid()).bind(alice.as_uuid()).bind(name).bind(now).execute(&pools.owner).await?;
    }
    let repository = Arc::new(PgReadingRepository::new(pools.api.clone()));
    let first_mutation = Uuid::now_v7();
    let first = repository
        .update_progress(command(
            alice,
            manifestation,
            device_a,
            first_mutation,
            0,
            0.2,
        ))
        .await?;
    assert!(matches!(first, ReadingUpdateOutcome::Updated { .. }));
    assert_eq!(global(&first).version, 1);

    let replay = repository
        .update_progress(command(
            alice,
            manifestation,
            device_a,
            first_mutation,
            0,
            0.99,
        ))
        .await?;
    assert_eq!(
        replay, first,
        "mutation replay must return its original result"
    );

    let smaller = repository
        .update_progress(command(
            alice,
            manifestation,
            device_a,
            Uuid::now_v7(),
            1,
            0.1,
        ))
        .await?;
    assert_eq!(global(&smaller).version, 2);
    assert_eq!(
        global(&smaller).locator.locations().progression(),
        Some(0.1),
        "server version, not largest percentage, orders writes"
    );

    let stale = repository
        .update_progress(command(
            alice,
            manifestation,
            device_b,
            Uuid::now_v7(),
            1,
            0.95,
        ))
        .await?;
    assert!(matches!(stale, ReadingUpdateOutcome::Conflict { .. }));
    assert_eq!(global(&stale).locator.locations().progression(), Some(0.1));
    assert_eq!(device(&stale).locator.locations().progression(), Some(0.95));

    let barrier = Arc::new(tokio::sync::Barrier::new(3));
    let mut tasks = Vec::new();
    for (device_id, progression) in [(device_a, 0.3), (device_b, 0.4)] {
        let repository = repository.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            repository
                .update_progress(command(
                    alice,
                    manifestation,
                    device_id,
                    Uuid::now_v7(),
                    2,
                    progression,
                ))
                .await
        }));
    }
    barrier.wait().await;
    let results = [
        tasks.remove(0).await.expect("task")?,
        tasks.remove(0).await.expect("task")?,
    ];
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, ReadingUpdateOutcome::Updated { .. }))
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, ReadingUpdateOutcome::Conflict { .. }))
            .count(),
        1
    );
    assert!(results.iter().all(|result| global(result).version == 3));

    let mut bob_tx = pools.api.begin().await?;
    PgTransactionContext::apply(
        &mut bob_tx,
        &DatabaseContext::api_without_library(bob, RequestId::new()),
    )
    .await?;
    let bob_count: i64 = sqlx::query_scalar("SELECT count(*) FROM folioharbor.reading_states")
        .fetch_one(&mut *bob_tx)
        .await?;
    assert_eq!(
        bob_count, 0,
        "forced user RLS must hide another user's state"
    );
    bob_tx.rollback().await?;

    sqlx::query("UPDATE folioharbor.library_memberships SET status='removed',removed_at=$3 WHERE library_id=$1 AND user_id=$2").bind(library.as_uuid()).bind(alice.as_uuid()).bind(now).execute(&pools.owner).await?;
    assert_eq!(
        repository
            .get_progress(alice, manifestation, RequestId::new())
            .await,
        Err(ReadingRepositoryError::NotFound)
    );
    let retained: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM folioharbor.reading_states WHERE user_id=$1 AND manifestation_id=$2",
    )
    .bind(alice.as_uuid())
    .bind(manifestation.as_uuid())
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(retained, 1);
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

async fn seed_item(
    pool: &sqlx::PgPool,
    library: LibraryId,
    actor: UserId,
    now: OffsetDateTime,
) -> anyhow::Result<ManifestationId> {
    let work = WorkId::new();
    let expression = ExpressionId::new();
    let manifestation = ManifestationId::new();
    let blob = BlobId::new();
    let package = PublicationPackageId::new();
    let holding = HoldingId::new();
    let item = ItemId::new();
    let upload = UploadId::new();
    sqlx::query("INSERT INTO folioharbor.works(work_id,primary_title,authors,created_at) VALUES($1,'Progress',ARRAY[]::text[],$2)").bind(work.as_uuid()).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.expressions(expression_id,work_id,languages,created_at) VALUES($1,$2,ARRAY['en'],$3)").bind(expression.as_uuid()).bind(work.as_uuid()).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.manifestations(manifestation_id,identifiers,created_at) VALUES($1,ARRAY[]::text[],$2)").bind(manifestation.as_uuid()).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.manifestation_expressions(manifestation_id,expression_id,expression_order) VALUES($1,$2,0)").bind(manifestation.as_uuid()).bind(expression.as_uuid()).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.blobs(blob_id,storage_namespace,sha256,byte_size,created_at) VALUES($1,'instance-v1',$2,1,$3)").bind(blob.as_uuid()).bind(vec![42_u8;32]).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.upload_sessions(upload_id,library_id,created_by,file_name,media_type,declared_bytes,dedup_scope,received_bytes,state,storage_key,sha256,expires_at,created_at,updated_at) VALUES($1,$2,$3,'progress.epub','application/epub+zip',1,'instance',1,'ready',$4,$5,$6,$6,$6)").bind(upload.as_uuid()).bind(library.as_uuid()).bind(actor.as_uuid()).bind(format!("blob:instance-v1:{}:1","2a".repeat(32))).bind(vec![42_u8;32]).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.publication_packages(package_id,manifestation_id,blob_id,parser_profile_version,created_at) VALUES($1,$2,$3,'epub-v1',$4)").bind(package.as_uuid()).bind(manifestation.as_uuid()).bind(blob.as_uuid()).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.holdings(holding_id,library_id,manifestation_id,state,created_at) VALUES($1,$2,$3,'active',$4)").bind(holding.as_uuid()).bind(library.as_uuid()).bind(manifestation.as_uuid()).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.items(item_id,holding_id,manifestation_id,package_id,source_upload_id,state,created_at) VALUES($1,$2,$3,$4,$5,'active',$6)").bind(item.as_uuid()).bind(holding.as_uuid()).bind(manifestation.as_uuid()).bind(package.as_uuid()).bind(upload.as_uuid()).bind(now).execute(pool).await?;
    Ok(manifestation)
}
