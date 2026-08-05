use folioharbor_postgres::{PgPools, run_migrations};
use folioharbor_test_support::postgres::TestPostgres;
use sqlx::PgPool;

const MIGRATION_LOCK_ID: i64 = 5_066_353_826_641_225_812;

#[tokio::test]
async fn cancelled_migration_does_not_return_locked_session_to_pool() -> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let owner_url = database.owner_url()?;
    let pools =
        PgPools::connect_for_tests(&owner_url, &database.api_url()?, &database.worker_url()?)
            .await?;
    run_migrations(&pools.owner).await?;

    let blocker_pool = PgPool::connect(&owner_url).await?;
    let mut blocker = blocker_pool.begin().await?;
    sqlx::query("LOCK TABLE _sqlx_migrations IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await?;

    let runner_pool = pools.owner.clone();
    let runner = tokio::spawn(async move { run_migrations(&runner_pool).await });

    let mut runner_backend_pid = None;
    for _ in 0..1_000 {
        if let Some(pid) = migration_lock_holder(&pools.worker).await? {
            runner_backend_pid = Some(pid);
            break;
        }
        tokio::task::yield_now().await;
    }
    let Some(runner_backend_pid) = runner_backend_pid else {
        anyhow::bail!("migration never acquired its advisory lock");
    };

    runner.abort();
    let Err(join_error) = runner.await else {
        anyhow::bail!("aborted migration task completed");
    };
    assert!(join_error.is_cancelled());
    blocker.rollback().await?;

    let retained_lock_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_locks \
         WHERE locktype = 'advisory' \
           AND database = (SELECT oid FROM pg_database WHERE datname = current_database()) \
           AND classid::bigint = (($1::bigint >> 32) & 4294967295) \
           AND objid::bigint = ($1::bigint & 4294967295) \
           AND objsubid = 1 \
           AND pid = $2 \
           AND pid = pg_backend_pid() \
           AND granted",
    )
    .bind(MIGRATION_LOCK_ID)
    .bind(runner_backend_pid)
    .fetch_one(&pools.owner)
    .await?;

    blocker_pool.close().await;
    pools.close().await;
    database.cleanup().await?;
    assert_eq!(retained_lock_count, 0, "pool reused a locked session");
    Ok(())
}

async fn migration_lock_holder(pool: &PgPool) -> Result<Option<i32>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT pid FROM pg_locks \
         WHERE locktype = 'advisory' \
           AND database = (SELECT oid FROM pg_database WHERE datname = current_database()) \
           AND classid::bigint = (($1::bigint >> 32) & 4294967295) \
           AND objid::bigint = ($1::bigint & 4294967295) \
           AND objsubid = 1 \
           AND pid IS NOT NULL \
           AND granted \
         LIMIT 1",
    )
    .bind(MIGRATION_LOCK_ID)
    .fetch_optional(pool)
    .await
}
