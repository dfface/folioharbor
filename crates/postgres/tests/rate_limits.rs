use folioharbor_application::ports::{RateLimitRepository, TokenBucketClaim, TokenBucketOutcome};
use folioharbor_domain::time::OffsetDateTime;
use folioharbor_postgres::{PgRateLimitRepository, connect_api, connect_owner, run_migrations};
use folioharbor_test_support::postgres::TestPostgres;
use secrecy::SecretString;
use sqlx::Row as _;
use std::sync::Arc;

#[tokio::test]
async fn token_bucket_is_durable_hmac_keyed_and_concurrency_safe() -> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let owner_url = SecretString::from(database.owner_url()?);
    let api_url = SecretString::from(database.api_url()?);
    let owner = connect_owner(&owner_url).await?;
    run_migrations(&owner).await?;
    owner.close().await;
    let pool = connect_api(&api_url).await?;
    let repository = Arc::new(PgRateLimitRepository::new(pool.clone()));
    let key = [42_u8; 32];
    let now = OffsetDateTime::from_unix_timestamp(1_800_000_000)?;
    let mut tasks = Vec::new();
    for _ in 0..20 {
        let repository = Arc::clone(&repository);
        tasks.push(tokio::spawn(async move {
            repository
                .consume(TokenBucketClaim {
                    key,
                    purpose: "login",
                    capacity: 3,
                    refill_per_second: 0.01,
                    now,
                })
                .await
        }));
    }
    let mut consumed = 0;
    let mut denied = 0;
    for task in tasks {
        match task.await?? {
            TokenBucketOutcome::Consumed => consumed += 1,
            TokenBucketOutcome::Denied {
                retry_after_seconds,
            } => {
                assert!(retry_after_seconds > 0);
                denied += 1;
            }
        }
    }
    assert_eq!(consumed, 3);
    assert_eq!(denied, 17);
    let rows = sqlx::query("SELECT bucket_key, purpose FROM folioharbor.auth_rate_limit_buckets")
        .fetch_all(&pool)
        .await?;
    assert_eq!(rows.len(), 1);
    let stored: Vec<u8> = rows[0].get("bucket_key");
    let purpose: String = rows[0].get("purpose");
    assert_eq!(stored, key);
    assert!(!purpose.contains('@'));
    assert!(!purpose.contains('.'));
    pool.close().await;
    database.cleanup().await?;
    Ok(())
}
