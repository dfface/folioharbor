use folioharbor_application::ports::{QuotaOutcome, QuotaRepository};
use folioharbor_domain::{
    id::{LibraryId, RequestId, UploadId, UserId},
    imports::quota::ByteCount,
};
use folioharbor_postgres::{
    DatabaseContext, PgPools, PgQuotaRepository, PgTransactionContext, run_migrations,
};
use folioharbor_test_support::postgres::TestPostgres;
use sqlx::PgPool;
use time::{Duration, OffsetDateTime};

async fn seed_library(
    pools: &PgPools,
    library: LibraryId,
    owner: UserId,
    quota: i64,
    now: OffsetDateTime,
) -> anyhow::Result<()> {
    sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,$2,$2,'verified',$3,$3) ON CONFLICT DO NOTHING")
        .bind(owner.as_uuid()).bind(format!("{owner:?}@test")).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,quota_limit_bytes,created_at,updated_at) VALUES($1,'Quota test',$2,$3,$3)")
        .bind(library.as_uuid()).bind(quota).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.library_memberships(library_id,user_id,role_code,status,joined_at) VALUES($1,$2,'owner','active',$3)")
        .bind(library.as_uuid()).bind(owner.as_uuid()).bind(now).execute(&pools.owner).await?;
    Ok(())
}

async fn assert_rls_case(
    pool: &PgPool,
    context: Option<&DatabaseContext>,
    expected: i64,
) -> anyhow::Result<()> {
    let mut transaction = pool.begin().await?;
    if let Some(context) = context {
        PgTransactionContext::apply(&mut transaction, context).await?;
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM folioharbor.quota_reservations")
        .fetch_one(&mut *transaction)
        .await?;
    assert_eq!(count, expected);
    assert!(
        sqlx::query("DELETE FROM folioharbor.quota_reservations")
            .execute(&mut *transaction)
            .await
            .is_err()
    );
    transaction.rollback().await?;
    Ok(())
}

#[tokio::test]
async fn new_libraries_receive_the_five_gibibyte_default_quota() -> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_migrations(&pools.owner).await?;
    let now = OffsetDateTime::now_utc();
    let library = LibraryId::new();
    sqlx::query(
        "INSERT INTO folioharbor.libraries(library_id,name,created_at,updated_at) VALUES($1,'Default quota',$2,$2)",
    )
    .bind(library.as_uuid())
    .bind(now)
    .execute(&pools.owner)
    .await?;
    let limit: i64 = sqlx::query_scalar(
        "SELECT quota_limit_bytes FROM folioharbor.libraries WHERE library_id=$1",
    )
    .bind(library.as_uuid())
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(limit, 5 * 1024 * 1024 * 1024);
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn concurrent_reservations_never_exceed_the_locked_library_quota() -> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_migrations(&pools.owner).await?;
    let now = OffsetDateTime::now_utc();
    let library = LibraryId::new();
    seed_library(&pools, library, UserId::new(), 100, now).await?;
    let repository = PgQuotaRepository::new(pools.worker.clone());
    let first_repo = repository.clone();
    let second_repo = repository.clone();
    let first = tokio::spawn(async move {
        first_repo
            .reserve(
                library,
                UploadId::new(),
                ByteCount::new(60),
                now + Duration::hours(1),
            )
            .await
    });
    let second = tokio::spawn(async move {
        second_repo
            .reserve(
                library,
                UploadId::new(),
                ByteCount::new(60),
                now + Duration::hours(1),
            )
            .await
    });
    let outcomes = [first.await??, second.await??];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == QuotaOutcome::Applied)
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == QuotaOutcome::Exceeded)
            .count(),
        1
    );
    let counters: (i64, i64) = sqlx::query_as("SELECT quota_used_bytes,quota_reserved_bytes FROM folioharbor.libraries WHERE library_id=$1").bind(library.as_uuid()).fetch_one(&pools.owner).await?;
    assert_eq!(counters, (0, 60));
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn resize_consume_release_are_atomic_and_usage_is_per_library() -> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_migrations(&pools.owner).await?;
    let now = OffsetDateTime::now_utc();
    let first_library = LibraryId::new();
    let second_library = LibraryId::new();
    seed_library(&pools, first_library, UserId::new(), 100, now).await?;
    seed_library(&pools, second_library, UserId::new(), 100, now).await?;
    let repository = PgQuotaRepository::new(pools.worker.clone());
    let first_upload = UploadId::new();
    assert_eq!(
        repository
            .reserve(
                first_library,
                first_upload,
                ByteCount::new(40),
                now + Duration::hours(1)
            )
            .await?,
        QuotaOutcome::Applied
    );
    assert_eq!(
        repository
            .resize_reservation(first_library, first_upload, ByteCount::new(70))
            .await?,
        QuotaOutcome::Applied
    );
    assert_eq!(
        repository
            .resize_reservation(first_library, first_upload, ByteCount::new(101))
            .await?,
        QuotaOutcome::Exceeded
    );
    assert_eq!(
        repository.consume(first_library, first_upload).await?,
        QuotaOutcome::Applied
    );
    assert_eq!(
        repository.release(first_library, first_upload).await?,
        QuotaOutcome::NotActive
    );

    let second_upload = UploadId::new();
    assert_eq!(
        repository
            .reserve(
                second_library,
                second_upload,
                ByteCount::new(70),
                now + Duration::hours(1)
            )
            .await?,
        QuotaOutcome::Applied
    );
    assert_eq!(
        repository.consume(second_library, second_upload).await?,
        QuotaOutcome::Applied
    );
    let usages: Vec<i64> = sqlx::query_scalar("SELECT quota_used_bytes FROM folioharbor.libraries WHERE library_id IN ($1,$2) ORDER BY library_id")
        .bind(first_library.as_uuid()).bind(second_library.as_uuid()).fetch_all(&pools.owner).await?;
    assert_eq!(
        usages,
        vec![70, 70],
        "physical dedup must not collapse per-library logical usage"
    );

    let released_upload = UploadId::new();
    assert_eq!(
        repository
            .reserve(
                first_library,
                released_upload,
                ByteCount::new(20),
                now + Duration::hours(1)
            )
            .await?,
        QuotaOutcome::Applied
    );
    assert_eq!(
        repository.release(first_library, released_upload).await?,
        QuotaOutcome::Applied
    );
    let counters: (i64, i64) = sqlx::query_as("SELECT quota_used_bytes,quota_reserved_bytes FROM folioharbor.libraries WHERE library_id=$1").bind(first_library.as_uuid()).fetch_one(&pools.owner).await?;
    assert_eq!(counters, (70, 0));
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn quota_reservation_rls_requires_the_exact_library_context() -> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_migrations(&pools.owner).await?;
    let now = OffsetDateTime::now_utc();
    let user = UserId::new();
    let library = LibraryId::new();
    seed_library(&pools, library, user, 100, now).await?;
    let repository = PgQuotaRepository::new(pools.worker.clone());
    repository
        .reserve(
            library,
            UploadId::new(),
            ByteCount::new(10),
            now + Duration::hours(1),
        )
        .await?;

    for (context, expected) in [
        (None, 0_i64),
        (
            Some(DatabaseContext::api(
                user,
                LibraryId::new(),
                RequestId::new(),
            )),
            0,
        ),
        (
            Some(DatabaseContext::api(user, library, RequestId::new())),
            1,
        ),
    ] {
        assert_rls_case(&pools.api, context.as_ref(), expected).await?;
    }
    for (context, expected) in [
        (None, 0_i64),
        (
            Some(DatabaseContext::worker(
                RequestId::new(),
                Some(LibraryId::new()),
            )),
            0,
        ),
        (
            Some(DatabaseContext::worker(RequestId::new(), Some(library))),
            1,
        ),
    ] {
        assert_rls_case(&pools.worker, context.as_ref(), expected).await?;
    }
    let mut impersonation = pools.api.begin().await?;
    PgTransactionContext::apply(
        &mut impersonation,
        &DatabaseContext::worker(RequestId::new(), Some(library)),
    )
    .await?;
    let impersonated_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM folioharbor.quota_reservations")
            .fetch_one(&mut *impersonation)
            .await?;
    assert_eq!(impersonated_count, 0);
    impersonation.rollback().await?;
    for pool in [&pools.api, &pools.worker] {
        assert!(
            sqlx::query("ALTER TABLE folioharbor.quota_reservations DISABLE ROW LEVEL SECURITY")
                .execute(pool)
                .await
                .is_err()
        );
    }
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}
