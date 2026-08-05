use folioharbor_postgres::{PgPools, run_migrations};
use folioharbor_test_support::postgres::TestPostgres;
use sqlx::PgPool;

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

    let mut advisory_lock_seen = false;
    for _ in 0..1_000 {
        let lock_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_locks WHERE locktype = 'advisory' AND granted",
        )
        .fetch_one(&pools.worker)
        .await?;
        if lock_count > 0 {
            advisory_lock_seen = true;
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(
        advisory_lock_seen,
        "migration never acquired its advisory lock"
    );

    runner.abort();
    let Err(join_error) = runner.await else {
        anyhow::bail!("aborted migration task completed");
    };
    assert!(join_error.is_cancelled());
    blocker.rollback().await?;

    let retained_lock_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_locks \
         WHERE pid = pg_backend_pid() AND locktype = 'advisory' AND granted",
    )
    .fetch_one(&pools.owner)
    .await?;

    blocker_pool.close().await;
    pools.close().await;
    database.cleanup().await?;
    assert_eq!(retained_lock_count, 0, "pool reused a locked session");
    Ok(())
}
