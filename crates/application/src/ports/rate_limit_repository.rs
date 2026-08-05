use async_trait::async_trait;
use folioharbor_domain::time::OffsetDateTime;
use thiserror::Error;

#[derive(Debug, Error)]
#[error("rate limit persistence failed")]
pub struct RateLimitRepositoryError;

#[derive(Clone, Copy, Debug)]
pub struct TokenBucketClaim {
    pub key: [u8; 32],
    pub purpose: &'static str,
    pub capacity: u32,
    pub refill_per_second: f64,
    pub now: OffsetDateTime,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TokenBucketOutcome {
    Consumed,
    Denied { retry_after_seconds: u64 },
}

#[async_trait]
pub trait RateLimitRepository: Send + Sync {
    async fn consume(
        &self,
        claim: TokenBucketClaim,
    ) -> Result<TokenBucketOutcome, RateLimitRepositoryError>;
}
