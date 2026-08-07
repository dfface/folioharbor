#![allow(clippy::expect_used, clippy::too_many_lines)]

use std::{collections::BTreeMap, sync::Arc};

use folioharbor_application::ports::{
    ReadingRepository, ReadingRepositoryError, UpdateProgressRecord,
};
use folioharbor_domain::{
    id::{
        BlobId, DeviceId, ExpressionId, HoldingId, ItemId, LibraryId, ManifestationId,
        PublicationPackageId, RequestId, UploadId, UserId, WorkId,
    },
    reader::{
        LocatorExtensionValue, LocatorExtensions, LocatorLocations, LocatorText,
        ReadingUpdateOutcome, ReadiumLocator,
    },
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

fn maximum_locator(progression: f64) -> ReadiumLocator {
    fn dense_text(length: usize, mut state: u64) -> String {
        (0..length)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                char::from(33 + u8::try_from(state % 90).expect("printable"))
            })
            .collect()
    }
    let extensions = (0..16)
        .map(|index| {
            (
                format!("x:{index:02}:{}", dense_text(120, index + 1)),
                LocatorExtensionValue::String(dense_text(1_024, index + 101)),
            )
        })
        .collect::<BTreeMap<_, _>>();
    ReadiumLocator::new(
        dense_text(2_048, 201),
        Some(dense_text(255, 202)),
        LocatorLocations::new(
            Some(progression),
            Some(1),
            Some(progression),
            (0..16)
                .map(|index| dense_text(2_048, index + 301))
                .collect(),
        )
        .expect("locations"),
        Some(
            LocatorText::new(
                Some(dense_text(4_096, 401)),
                Some(dense_text(4_096, 402)),
                Some(dense_text(4_096, 403)),
            )
            .expect("text"),
        ),
        LocatorExtensions::new(1, extensions).expect("extensions"),
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
        | ReadingUpdateOutcome::Conflict {
            global: Some(global),
            ..
        } => Some(global),
        ReadingUpdateOutcome::Conflict { global: None, .. } => None,
    }
    .expect("global state")
}
fn device(outcome: &ReadingUpdateOutcome) -> &folioharbor_domain::reader::DeviceReadingState {
    match outcome {
        ReadingUpdateOutcome::Updated { device, .. }
        | ReadingUpdateOutcome::Conflict { device, .. } => device,
    }
}

async fn recorded_usage(pool: &sqlx::PgPool, user: UserId) -> anyhow::Result<(i64, i64)> {
    Ok(sqlx::query_as(
        "SELECT live_count,live_bytes FROM folioharbor.reading_mutation_usage WHERE user_id=$1",
    )
    .bind(user.as_uuid())
    .fetch_one(pool)
    .await?)
}

async fn actual_usage(pool: &sqlx::PgPool, user: UserId) -> anyhow::Result<(i64, i64)> {
    Ok(sqlx::query_as(
        "SELECT count(*)::bigint,COALESCE(sum(pg_column_size(m)+COALESCE(pg_column_size(global_locator),0)+pg_column_size(device_locator)),0)::bigint FROM folioharbor.reading_mutations m WHERE user_id=$1",
    )
    .bind(user.as_uuid())
    .fetch_one(pool)
    .await?)
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
    let concurrent_api = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&database.api_url()?)
        .await?;
    run_migrations(&pools.owner).await?;
    for table in [
        "reading_states",
        "device_reading_states",
        "reading_mutations",
        "reading_mutation_usage",
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
    let manifestation = seed_item(&pools.owner, library, alice, now, 42).await?;
    let device_a = DeviceId::new();
    let device_b = DeviceId::new();
    sqlx::query("INSERT INTO folioharbor.user_devices(device_id,user_id,display_name,created_at,last_seen_at) VALUES($1,$2,'A',$3,$3)").bind(device_a.as_uuid()).bind(alice.as_uuid()).bind(now).execute(&pools.owner).await?;
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

    let direct_mutation = Uuid::now_v7();
    let usage_before_direct = recorded_usage(&pools.owner, alice).await?;
    sqlx::query(
        "INSERT INTO folioharbor.reading_mutations(user_id,client_mutation_id,manifestation_id,device_id,outcome,global_package_id,global_content_unit_id,global_locator,global_version,global_updated_at,device_locator,device_updated_at,created_at,request_fingerprint) SELECT user_id,$3,manifestation_id,device_id,outcome,global_package_id,global_content_unit_id,global_locator,global_version,global_updated_at,device_locator,device_updated_at,clock_timestamp(),request_fingerprint FROM folioharbor.reading_mutations WHERE user_id=$1 AND client_mutation_id=$2",
    )
    .bind(alice.as_uuid())
    .bind(first_mutation)
    .bind(direct_mutation)
    .execute(&pools.owner)
    .await?;
    assert_eq!(
        recorded_usage(&pools.owner, alice).await?,
        actual_usage(&pools.owner, alice).await?,
        "old-writer direct inserts must be counted at the database boundary"
    );
    assert!(recorded_usage(&pools.owner, alice).await?.0 > usage_before_direct.0);
    sqlx::query(
        "DELETE FROM folioharbor.reading_mutations WHERE user_id=$1 AND client_mutation_id=$2",
    )
    .bind(alice.as_uuid())
    .bind(direct_mutation)
    .execute(&pools.owner)
    .await?;
    assert_eq!(
        recorded_usage(&pools.owner, alice).await?,
        usage_before_direct,
        "direct deletes must decrement exact usage"
    );
    let mut immutable_tx = pools.api.begin().await?;
    PgTransactionContext::apply(
        &mut immutable_tx,
        &DatabaseContext::api_without_library(alice, RequestId::new()),
    )
    .await?;
    let direct_update = sqlx::query(
        "UPDATE folioharbor.reading_mutations SET created_at=created_at WHERE user_id=$1 AND client_mutation_id=$2",
    )
    .bind(alice.as_uuid())
    .bind(first_mutation)
    .execute(&mut *immutable_tx)
    .await;
    assert!(
        direct_update.is_err(),
        "the API role must not mutate immutable replay records"
    );
    immutable_tx.rollback().await?;
    let owner_update = sqlx::query(
        "UPDATE folioharbor.reading_mutations SET created_at=created_at WHERE user_id=$1 AND client_mutation_id=$2",
    )
    .bind(alice.as_uuid())
    .bind(first_mutation)
    .execute(&pools.owner)
    .await
    .expect_err("the database must reject owner-level mutation updates");
    assert_eq!(
        owner_update
            .as_database_error()
            .and_then(|error| error.constraint()),
        Some("reading_mutation_immutable")
    );

    let mut usage_tamper_tx = pools.api.begin().await?;
    PgTransactionContext::apply(
        &mut usage_tamper_tx,
        &DatabaseContext::api_without_library(alice, RequestId::new()),
    )
    .await?;
    let usage_tamper = sqlx::query(
        "UPDATE folioharbor.reading_mutation_usage SET live_count=0,live_bytes=0 WHERE user_id=$1",
    )
    .bind(alice.as_uuid())
    .execute(&mut *usage_tamper_tx)
    .await;
    assert!(
        usage_tamper.is_err(),
        "the API role must not tamper with database-owned quota counters"
    );
    usage_tamper_tx.rollback().await?;

    let inconsistent_zero_snapshot = sqlx::query(
        "INSERT INTO folioharbor.reading_mutations(user_id,client_mutation_id,manifestation_id,device_id,outcome,global_locator,global_version,global_updated_at,device_locator,device_updated_at,request_fingerprint) VALUES($1,$2,$3,$4,'updated',NULL,0,NULL,'{}'::jsonb,$5,$6)",
    )
    .bind(alice.as_uuid())
    .bind(Uuid::now_v7())
    .bind(manifestation.as_uuid())
    .bind(device_a.as_uuid())
    .bind(now)
    .bind(vec![0_u8; 32])
    .execute(&pools.owner)
    .await;
    assert!(
        inconsistent_zero_snapshot.is_err(),
        "version zero is valid only for a conflict with no global snapshot"
    );
    let inconsistent_positive_snapshot = sqlx::query(
        "INSERT INTO folioharbor.reading_mutations(user_id,client_mutation_id,manifestation_id,device_id,outcome,global_locator,global_version,global_updated_at,device_locator,device_updated_at,request_fingerprint) VALUES($1,$2,$3,$4,'conflict',NULL,1,NULL,'{}'::jsonb,$5,$6)",
    )
    .bind(alice.as_uuid())
    .bind(Uuid::now_v7())
    .bind(manifestation.as_uuid())
    .bind(device_a.as_uuid())
    .bind(now)
    .bind(vec![0_u8; 32])
    .execute(&pools.owner)
    .await;
    assert!(
        inconsistent_positive_snapshot.is_err(),
        "positive global versions require a complete global snapshot"
    );

    let replay = repository
        .update_progress(command(
            alice,
            manifestation,
            device_a,
            first_mutation,
            0,
            0.2,
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
    let enrolled: String = sqlx::query_scalar(
        "SELECT display_name FROM folioharbor.user_devices WHERE user_id=$1 AND device_id=$2",
    )
    .bind(alice.as_uuid())
    .bind(device_b.as_uuid())
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(
        enrolled, "Web reader",
        "first progress write enrolls the Web device"
    );

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

    let later_mutation = Uuid::now_v7();
    let later = repository
        .update_progress(command(
            alice,
            manifestation,
            device_a,
            later_mutation,
            3,
            0.6,
        ))
        .await?;
    assert_eq!(global(&later).version, 4);
    assert_eq!(
        repository
            .update_progress(command(
                alice,
                manifestation,
                device_a,
                first_mutation,
                0,
                0.2,
            ))
            .await?,
        first
    );
    assert_eq!(
        repository
            .get_progress(alice, manifestation, RequestId::new())
            .await?
            .expect("current")
            .version,
        4,
        "replaying an older accepted mutation cannot rewind current state"
    );

    let old_stale_mutation = Uuid::now_v7();
    let old_stale = repository
        .update_progress(command(
            alice,
            manifestation,
            device_b,
            old_stale_mutation,
            2,
            0.7,
        ))
        .await?;
    assert_eq!(global(&old_stale).version, 4);
    let newest = repository
        .update_progress(command(
            alice,
            manifestation,
            device_a,
            Uuid::now_v7(),
            4,
            0.5,
        ))
        .await?;
    assert_eq!(global(&newest).version, 5);
    assert_eq!(
        repository
            .update_progress(command(
                alice,
                manifestation,
                device_b,
                old_stale_mutation,
                2,
                0.7,
            ))
            .await?,
        old_stale
    );
    assert_eq!(
        repository
            .get_progress(alice, manifestation, RequestId::new())
            .await?
            .expect("current")
            .version,
        5,
        "replaying an older stale mutation cannot change current state"
    );
    assert_eq!(
        repository
            .update_progress(command(
                alice,
                manifestation,
                device_b,
                first_mutation,
                0,
                0.2,
            ))
            .await,
        Err(ReadingRepositoryError::MutationMismatch)
    );
    assert_eq!(
        repository
            .update_progress(command(
                alice,
                manifestation,
                device_a,
                first_mutation,
                0,
                0.21,
            ))
            .await,
        Err(ReadingRepositoryError::MutationMismatch)
    );

    let library_b = LibraryId::new();
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,created_at,updated_at) VALUES($1,'Progress B',$2,$2)").bind(library_b.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.library_memberships(library_id,user_id,role_code,status,joined_at) VALUES($1,$2,'reader','active',$3)").bind(library_b.as_uuid()).bind(alice.as_uuid()).bind(now).execute(&pools.owner).await?;
    let manifestation_b = seed_item(&pools.owner, library_b, alice, now, 43).await?;
    let absent = repository
        .update_progress(command(
            alice,
            manifestation_b,
            device_b,
            Uuid::now_v7(),
            9,
            0.8,
        ))
        .await?;
    assert!(matches!(
        absent,
        ReadingUpdateOutcome::Conflict { global: None, .. }
    ));
    assert_eq!(device(&absent).locator.locations().progression(), Some(0.8));

    let usage_table_exists: bool =
        sqlx::query_scalar("SELECT to_regclass('folioharbor.reading_mutation_usage') IS NOT NULL")
            .fetch_one(&pools.owner)
            .await?;
    assert!(usage_table_exists, "mutation usage must be tracked in O(1)");
    assert_eq!(
        recorded_usage(&pools.owner, alice).await?,
        actual_usage(&pools.owner, alice).await?,
        "migration backfill and repository accounting must match actual rows"
    );

    let device_c = DeviceId::new();
    sqlx::query("INSERT INTO folioharbor.user_devices(device_id,user_id,display_name,created_at,last_seen_at) VALUES($1,$2,'C',$3,$3)")
        .bind(device_c.as_uuid())
        .bind(alice.as_uuid())
        .bind(now)
        .execute(&pools.owner)
        .await?;
    let usage_before_cascade = recorded_usage(&pools.owner, alice).await?;
    repository
        .update_progress(command(
            alice,
            manifestation,
            device_c,
            Uuid::now_v7(),
            0,
            0.64,
        ))
        .await?;
    sqlx::query("DELETE FROM folioharbor.user_devices WHERE user_id=$1 AND device_id=$2")
        .bind(alice.as_uuid())
        .bind(device_c.as_uuid())
        .execute(&pools.owner)
        .await?;
    assert_eq!(
        recorded_usage(&pools.owner, alice).await?,
        usage_before_cascade,
        "device cascades must decrement mutation usage"
    );
    assert_eq!(
        recorded_usage(&pools.owner, alice).await?,
        actual_usage(&pools.owner, alice).await?
    );

    let large_mutation = Uuid::now_v7();
    let large_command = UpdateProgressRecord {
        actor: alice,
        manifestation_id: manifestation_b,
        device_id: device_a,
        client_mutation_id: large_mutation,
        base_version: 0,
        package_id: None,
        content_unit_id: None,
        locator: maximum_locator(0.7),
        request_id: RequestId::new(),
    };
    let large = repository.update_progress(large_command.clone()).await?;
    assert!(matches!(large, ReadingUpdateOutcome::Updated { .. }));
    let large_row_bytes: i64 = sqlx::query_scalar(
        "SELECT (pg_column_size(m)+COALESCE(pg_column_size(global_locator),0)+pg_column_size(device_locator))::bigint FROM folioharbor.reading_mutations m WHERE user_id=$1 AND client_mutation_id=$2",
    )
    .bind(alice.as_uuid())
    .bind(large_mutation)
    .fetch_one(&pools.owner)
    .await?;
    let (_, before_fill_bytes) = actual_usage(&pools.owner, alice).await?;
    let byte_limit = 64_i64 * 1_024 * 1_024;
    let copies = ((byte_limit - before_fill_bytes) / large_row_bytes).max(0);
    sqlx::query(
        "INSERT INTO folioharbor.reading_mutations(user_id,client_mutation_id,manifestation_id,device_id,outcome,global_package_id,global_content_unit_id,global_locator,global_version,global_updated_at,device_locator,device_updated_at,created_at,request_fingerprint) SELECT user_id,md5('large-' || series::text)::uuid,manifestation_id,device_id,outcome,global_package_id,global_content_unit_id,global_locator,global_version,global_updated_at,device_locator,device_updated_at,clock_timestamp(),request_fingerprint FROM folioharbor.reading_mutations CROSS JOIN generate_series(1,$3) series WHERE user_id=$1 AND client_mutation_id=$2",
    )
    .bind(alice.as_uuid())
    .bind(large_mutation)
    .bind(copies)
    .execute(&pools.owner)
    .await?;
    let actual_after_fill = actual_usage(&pools.owner, alice).await?;
    assert_eq!(
        recorded_usage(&pools.owner, alice).await?,
        actual_after_fill,
        "bulk direct inserts must maintain exact byte usage"
    );
    assert!(actual_after_fill.1 <= byte_limit);
    assert!(byte_limit - actual_after_fill.1 < large_row_bytes);
    let state_before_byte_rejection = repository
        .get_progress(alice, manifestation_b, RequestId::new())
        .await?
        .expect("global");
    let device_before_byte_rejection: serde_json::Value = sqlx::query_scalar(
        "SELECT locator FROM folioharbor.device_reading_states WHERE user_id=$1 AND device_id=$2 AND manifestation_id=$3",
    )
    .bind(alice.as_uuid())
    .bind(device_a.as_uuid())
    .bind(manifestation_b.as_uuid())
    .fetch_one(&pools.owner)
    .await?;
    let usage_before_byte_rejection = recorded_usage(&pools.owner, alice).await?;
    let mut over_byte_limit = large_command.clone();
    over_byte_limit.client_mutation_id = Uuid::now_v7();
    over_byte_limit.base_version = state_before_byte_rejection.version;
    over_byte_limit.locator = maximum_locator(0.8);
    let byte_capacity = repository.update_progress(over_byte_limit).await;
    assert!(matches!(
        byte_capacity,
        Err(ReadingRepositoryError::MutationCapacity { retry_after })
            if !retry_after.is_zero()
    ));
    assert_eq!(
        repository
            .get_progress(alice, manifestation_b, RequestId::new())
            .await?
            .expect("global"),
        state_before_byte_rejection,
        "byte-cap rejection must roll back global and device transaction state"
    );
    let device_after_byte_rejection: serde_json::Value = sqlx::query_scalar(
        "SELECT locator FROM folioharbor.device_reading_states WHERE user_id=$1 AND device_id=$2 AND manifestation_id=$3",
    )
    .bind(alice.as_uuid())
    .bind(device_a.as_uuid())
    .bind(manifestation_b.as_uuid())
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(
        device_after_byte_rejection, device_before_byte_rejection,
        "byte-cap rejection must roll back the device position"
    );
    assert_eq!(
        recorded_usage(&pools.owner, alice).await?,
        usage_before_byte_rejection
    );
    assert_eq!(
        recorded_usage(&pools.owner, alice).await?,
        actual_usage(&pools.owner, alice).await?
    );

    assert_eq!(
        repository.update_progress(large_command.clone()).await?,
        large,
        "an exact replay inside the retention window bypasses capacity"
    );
    sqlx::query("DELETE FROM folioharbor.reading_mutations WHERE user_id=$1 AND request_fingerprint=(SELECT request_fingerprint FROM folioharbor.reading_mutations WHERE user_id=$1 AND client_mutation_id=$2) AND client_mutation_id<>$2")
        .bind(alice.as_uuid())
        .bind(large_mutation)
        .execute(&pools.owner)
        .await?;
    let expired_mutation = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO folioharbor.reading_mutations(user_id,client_mutation_id,manifestation_id,device_id,outcome,global_package_id,global_content_unit_id,global_locator,global_version,global_updated_at,device_locator,device_updated_at,created_at,request_fingerprint) SELECT user_id,$3,manifestation_id,device_id,outcome,global_package_id,global_content_unit_id,global_locator,global_version,global_updated_at,device_locator,device_updated_at,clock_timestamp()-interval '31 days',request_fingerprint FROM folioharbor.reading_mutations WHERE user_id=$1 AND client_mutation_id=$2",
    )
    .bind(alice.as_uuid())
    .bind(large_mutation)
    .bind(expired_mutation)
    .execute(&pools.owner)
    .await?;
    let mut expired_command = large_command.clone();
    expired_command.client_mutation_id = expired_mutation;
    let expired_reuse = repository.update_progress(expired_command).await?;
    assert!(matches!(
        expired_reuse,
        ReadingUpdateOutcome::Conflict { .. }
    ));
    assert_ne!(
        expired_reuse, large,
        "expired ids are evaluated as new mutations"
    );

    sqlx::query(
        "INSERT INTO folioharbor.reading_mutations(user_id,client_mutation_id,manifestation_id,device_id,outcome,global_package_id,global_content_unit_id,global_locator,global_version,global_updated_at,device_locator,device_updated_at,created_at,request_fingerprint) SELECT user_id,md5('expired-' || series::text)::uuid,manifestation_id,device_id,outcome,global_package_id,global_content_unit_id,global_locator,global_version,global_updated_at,device_locator,device_updated_at,clock_timestamp()-interval '31 days',request_fingerprint FROM folioharbor.reading_mutations CROSS JOIN generate_series(1,5) series WHERE user_id=$1 AND client_mutation_id=$2",
    )
        .bind(alice.as_uuid())
        .bind(first_mutation)
        .execute(&pools.owner)
        .await?;
    let count_before_prune = actual_usage(&pools.owner, alice).await?.0;
    let prune_trigger = repository
        .update_progress(command(
            alice,
            manifestation_b,
            device_b,
            Uuid::now_v7(),
            1,
            0.81,
        ))
        .await?;
    assert!(matches!(
        prune_trigger,
        ReadingUpdateOutcome::Updated { .. }
    ));
    let count_after_prune = actual_usage(&pools.owner, alice).await?.0;
    assert!(count_after_prune < count_before_prune);
    assert_eq!(
        recorded_usage(&pools.owner, alice).await?,
        actual_usage(&pools.owner, alice).await?,
        "pruning must subtract the exact count and physical row bytes"
    );

    let (live_count, _) = actual_usage(&pools.owner, alice).await?;
    let row_copies = 9_999_i64 - live_count;
    sqlx::query(
        "INSERT INTO folioharbor.reading_mutations(user_id,client_mutation_id,manifestation_id,device_id,outcome,global_package_id,global_content_unit_id,global_locator,global_version,global_updated_at,device_locator,device_updated_at,created_at,request_fingerprint) SELECT user_id,md5('small-' || series::text)::uuid,manifestation_id,device_id,outcome,global_package_id,global_content_unit_id,global_locator,global_version,global_updated_at,device_locator,device_updated_at,clock_timestamp(),request_fingerprint FROM folioharbor.reading_mutations CROSS JOIN generate_series(1,$3) series WHERE user_id=$1 AND client_mutation_id=$2",
    )
    .bind(alice.as_uuid())
    .bind(first_mutation)
    .bind(row_copies)
    .execute(&pools.owner)
    .await?;
    let actual_at_boundary = actual_usage(&pools.owner, alice).await?;
    assert_eq!(
        recorded_usage(&pools.owner, alice).await?,
        actual_at_boundary,
        "bulk direct inserts must maintain exact row usage"
    );
    assert_eq!(actual_at_boundary.0, 9_999);
    assert!(actual_at_boundary.1 < byte_limit);
    let barrier = Arc::new(tokio::sync::Barrier::new(3));
    let mut capacity_tasks = Vec::new();
    for mutation in [Uuid::now_v7(), Uuid::now_v7()] {
        let pool = concurrent_api.clone();
        let barrier = barrier.clone();
        capacity_tasks.push(tokio::spawn(async move {
            let mut tx = pool.begin().await.expect("capacity transaction");
            PgTransactionContext::apply(
                &mut tx,
                &DatabaseContext::api_without_library(alice, RequestId::new()),
            )
            .await
            .expect("capacity transaction context");
            barrier.wait().await;
            let inserted = sqlx::query(
                "INSERT INTO folioharbor.reading_mutations(user_id,client_mutation_id,manifestation_id,device_id,outcome,global_package_id,global_content_unit_id,global_locator,global_version,global_updated_at,device_locator,device_updated_at,created_at,request_fingerprint) SELECT user_id,$3,manifestation_id,device_id,outcome,global_package_id,global_content_unit_id,global_locator,global_version,global_updated_at,device_locator,device_updated_at,clock_timestamp(),request_fingerprint FROM folioharbor.reading_mutations WHERE user_id=$1 AND client_mutation_id=$2",
            )
                .bind(alice.as_uuid())
                .bind(first_mutation)
                .bind(mutation)
                .execute(&mut *tx)
                .await
                .is_ok();
            if inserted {
                tx.commit().await.expect("capacity commit");
            } else {
                tx.rollback().await.expect("capacity rollback");
            }
            inserted
        }));
    }
    barrier.wait().await;
    let capacity_results = [
        capacity_tasks.remove(0).await.expect("task"),
        capacity_tasks.remove(0).await.expect("task"),
    ];
    assert_eq!(capacity_results.iter().filter(|result| **result).count(), 1);
    assert_eq!(
        capacity_results.iter().filter(|result| !**result).count(),
        1
    );
    assert_eq!(actual_usage(&pools.owner, alice).await?.0, 10_000);
    assert_eq!(
        recorded_usage(&pools.owner, alice).await?,
        actual_usage(&pools.owner, alice).await?,
        "concurrent attempts cannot exceed either hard quota"
    );
    let state_at_capacity = repository
        .get_progress(alice, manifestation_b, RequestId::new())
        .await?
        .expect("global");
    let device_at_capacity: serde_json::Value = sqlx::query_scalar(
        "SELECT locator FROM folioharbor.device_reading_states WHERE user_id=$1 AND device_id=$2 AND manifestation_id=$3",
    )
    .bind(alice.as_uuid())
    .bind(device_a.as_uuid())
    .bind(manifestation_b.as_uuid())
    .fetch_one(&pools.owner)
    .await?;
    let row_capacity = repository
        .update_progress(command(
            alice,
            manifestation_b,
            device_a,
            Uuid::now_v7(),
            state_at_capacity.version,
            0.99,
        ))
        .await;
    assert!(matches!(
        row_capacity,
        Err(ReadingRepositoryError::MutationCapacity { retry_after })
            if !retry_after.is_zero()
    ));
    assert_eq!(
        repository
            .get_progress(alice, manifestation_b, RequestId::new())
            .await?
            .expect("global"),
        state_at_capacity,
        "row-cap rejection must not change progress"
    );
    let device_after_capacity: serde_json::Value = sqlx::query_scalar(
        "SELECT locator FROM folioharbor.device_reading_states WHERE user_id=$1 AND device_id=$2 AND manifestation_id=$3",
    )
    .bind(alice.as_uuid())
    .bind(device_a.as_uuid())
    .bind(manifestation_b.as_uuid())
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(
        device_after_capacity, device_at_capacity,
        "row-cap rejection must roll back the device position"
    );

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
    let bob_usage_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM folioharbor.reading_mutation_usage")
            .fetch_one(&mut *bob_tx)
            .await?;
    assert_eq!(bob_usage_count, 0, "usage counters are user-private");
    bob_tx.rollback().await?;

    sqlx::query(
        "UPDATE folioharbor.user_devices SET revoked_at=$3 WHERE user_id=$1 AND device_id=$2",
    )
    .bind(alice.as_uuid())
    .bind(device_b.as_uuid())
    .bind(now)
    .execute(&pools.owner)
    .await?;
    assert_eq!(
        repository
            .update_progress(command(
                alice,
                manifestation_b,
                device_b,
                Uuid::now_v7(),
                0,
                0.2,
            ))
            .await,
        Err(ReadingRepositoryError::NotFound),
        "a revoked device cannot silently enroll itself again"
    );

    sqlx::query("UPDATE folioharbor.library_memberships SET status='removed',removed_at=$3 WHERE library_id=$1 AND user_id=$2").bind(library.as_uuid()).bind(alice.as_uuid()).bind(now).execute(&pools.owner).await?;
    assert_eq!(
        repository
            .update_progress(command(
                alice,
                manifestation_b,
                device_a,
                first_mutation,
                0,
                0.2,
            ))
            .await,
        Err(ReadingRepositoryError::MutationMismatch),
        "authorized B must not disclose the stored A result after A access is lost"
    );
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
    concurrent_api.close().await;
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

async fn seed_item(
    pool: &sqlx::PgPool,
    library: LibraryId,
    actor: UserId,
    now: OffsetDateTime,
    seed: u8,
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
    sqlx::query("INSERT INTO folioharbor.blobs(blob_id,storage_namespace,sha256,byte_size,created_at) VALUES($1,'instance-v1',$2,1,$3)").bind(blob.as_uuid()).bind(vec![seed;32]).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.upload_sessions(upload_id,library_id,created_by,file_name,media_type,declared_bytes,dedup_scope,received_bytes,state,storage_key,sha256,expires_at,created_at,updated_at) VALUES($1,$2,$3,'progress.epub','application/epub+zip',1,'instance',1,'ready',$4,$5,$6,$6,$6)").bind(upload.as_uuid()).bind(library.as_uuid()).bind(actor.as_uuid()).bind(format!("blob:instance-v1:{}:1",format!("{seed:02x}").repeat(32))).bind(vec![seed;32]).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.publication_packages(package_id,manifestation_id,blob_id,parser_profile_version,created_at) VALUES($1,$2,$3,'epub-v1',$4)").bind(package.as_uuid()).bind(manifestation.as_uuid()).bind(blob.as_uuid()).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.holdings(holding_id,library_id,manifestation_id,state,created_at) VALUES($1,$2,$3,'active',$4)").bind(holding.as_uuid()).bind(library.as_uuid()).bind(manifestation.as_uuid()).bind(now).execute(pool).await?;
    sqlx::query("INSERT INTO folioharbor.items(item_id,holding_id,manifestation_id,package_id,source_upload_id,state,created_at) VALUES($1,$2,$3,$4,$5,'active',$6)").bind(item.as_uuid()).bind(holding.as_uuid()).bind(manifestation.as_uuid()).bind(package.as_uuid()).bind(upload.as_uuid()).bind(now).execute(pool).await?;
    Ok(manifestation)
}
