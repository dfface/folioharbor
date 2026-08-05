use async_trait::async_trait;
use folioharbor_application::ports::{
    RateLimitRepository, RateLimitRepositoryError, TokenBucketClaim, TokenBucketOutcome,
};
use sqlx::PgPool;

#[derive(Clone, Debug)]
pub struct PgRateLimitRepository {
    pool: PgPool,
}
impl PgRateLimitRepository {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl RateLimitRepository for PgRateLimitRepository {
    async fn consume(
        &self,
        claim: TokenBucketClaim,
    ) -> Result<TokenBucketOutcome, RateLimitRepositoryError> {
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        fn calculate(
            claim: TokenBucketClaim,
            stored: f64,
            updated: folioharbor_domain::time::OffsetDateTime,
        ) -> (f64, TokenBucketOutcome) {
            let elapsed = (claim.now.unix_timestamp() - updated.unix_timestamp()).max(0) as f64;
            let available =
                (stored + elapsed * claim.refill_per_second).min(f64::from(claim.capacity));
            if available >= 1.0 {
                (available - 1.0, TokenBucketOutcome::Consumed)
            } else {
                let seconds = ((1.0 - available) / claim.refill_per_second)
                    .ceil()
                    .max(1.0) as u64;
                (
                    available,
                    TokenBucketOutcome::Denied {
                        retry_after_seconds: seconds,
                    },
                )
            }
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| RateLimitRepositoryError)?;
        sqlx::query!("INSERT INTO folioharbor.auth_rate_limit_buckets (bucket_key, purpose, tokens, updated_at) VALUES ($1, $2, $3, $4) ON CONFLICT (bucket_key) DO NOTHING", claim.key.as_slice(), claim.purpose, f64::from(claim.capacity), claim.now).execute(&mut *tx).await.map_err(|_| RateLimitRepositoryError)?;
        let row = sqlx::query!(r#"SELECT tokens AS "tokens!", updated_at AS "updated_at!" FROM folioharbor.auth_rate_limit_buckets WHERE bucket_key = $1 FOR UPDATE"#, claim.key.as_slice()).fetch_one(&mut *tx).await.map_err(|_| RateLimitRepositoryError)?;
        let (tokens, outcome) = calculate(claim, row.tokens, row.updated_at);
        sqlx::query!("UPDATE folioharbor.auth_rate_limit_buckets SET tokens = $2, updated_at = $3, version = version + 1 WHERE bucket_key = $1", claim.key.as_slice(), tokens, claim.now).execute(&mut *tx).await.map_err(|_| RateLimitRepositoryError)?;
        tx.commit().await.map_err(|_| RateLimitRepositoryError)?;
        Ok(outcome)
    }
}
