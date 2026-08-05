use folioharbor_domain::id::{LibraryId, RequestId, UserId};
use folioharbor_postgres::{DatabaseContext, PgPools, PgTransactionContext, run_migrations};
use folioharbor_test_support::postgres::TestPostgres;

#[tokio::test]
async fn transaction_context_is_cleared_after_commit_and_pool_checkout() -> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_migrations(&pools.owner).await?;

    let user_id = UserId::new();
    let library_id = LibraryId::new();
    let request_id = RequestId::new();
    let context = DatabaseContext::api(user_id, library_id, request_id);

    let mut transaction = pools.api.begin().await?;
    PgTransactionContext::apply(&mut transaction, &context).await?;
    let applied: (Option<uuid::Uuid>, Option<uuid::Uuid>, Option<String>, bool) = sqlx::query_as(
        "SELECT folioharbor.current_user_id(), folioharbor.current_library_id(), \
         folioharbor.current_request_id(), folioharbor.is_worker()",
    )
    .fetch_one(&mut *transaction)
    .await?;
    assert_eq!(applied.0, Some(user_id.as_uuid()));
    assert_eq!(applied.1, Some(library_id.as_uuid()));
    let expected_request_id = request_id.as_ulid().to_string();
    assert_eq!(applied.2.as_deref(), Some(expected_request_id.as_str()));
    assert!(!applied.3);
    transaction.commit().await?;

    let retained: (Option<String>, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT NULLIF(current_setting('app.user_id', true), ''), \
         NULLIF(current_setting('app.library_id', true), ''), \
         NULLIF(current_setting('app.request_id', true), '')",
    )
    .fetch_one(&pools.api)
    .await?;
    assert_eq!(retained, (None, None, None));

    pools.close().await;
    database.cleanup().await?;
    Ok(())
}
