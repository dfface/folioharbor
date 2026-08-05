use std::time::Duration;

use async_trait::async_trait;
use hmac::{Hmac, Mac as _};
use secrecy::{ExposeSecret as _, SecretString};
use sha2::Sha256;

use crate::{
    error::AppError,
    ports::{Clock, RateLimitRepository, TokenBucketClaim, TokenBucketOutcome},
};
use folioharbor_domain::time::OffsetDateTime;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RateLimitPurpose {
    Registration,
    Login,
    Verification,
    Invitation,
    PasswordReset,
}
impl RateLimitPurpose {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Registration => "registration",
            Self::Login => "login",
            Self::Verification => "verification",
            Self::Invitation => "invitation",
            Self::PasswordReset => "password_reset",
        }
    }
    const fn policy(self) -> (u32, f64) {
        match self {
            Self::Registration | Self::PasswordReset => (5, 1.0 / 300.0),
            Self::Login | Self::Verification | Self::Invitation => (10, 1.0 / 60.0),
        }
    }
}

pub struct CheckRateLimit {
    pub purpose: RateLimitPurpose,
    pub normalized_identifier: String,
    pub ip_prefix: String,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RateLimitDecision {
    Allowed,
    Denied { retry_after: Duration },
}

#[async_trait]
pub trait RateLimitUseCase: Send + Sync {
    async fn check_rate_limit(
        &self,
        command: CheckRateLimit,
    ) -> Result<RateLimitDecision, AppError>;
}

pub struct DurableRateLimiter<R, C> {
    repository: R,
    hmac_secret: SecretString,
    clock: C,
}
impl<R, C> DurableRateLimiter<R, C> {
    #[must_use]
    pub const fn new(repository: R, hmac_secret: SecretString, clock: C) -> Self {
        Self {
            repository,
            hmac_secret,
            clock,
        }
    }
}

impl<R: RateLimitRepository, C> DurableRateLimiter<R, C> {
    /// Applies the purpose-specific durable token bucket at a supplied instant.
    ///
    /// # Errors
    /// Returns dependency unavailable when HMAC or persistence operations fail.
    pub async fn check_at(
        &self,
        command: CheckRateLimit,
        now: OffsetDateTime,
    ) -> Result<RateLimitDecision, AppError> {
        let mut mac = Hmac::<Sha256>::new_from_slice(self.hmac_secret.expose_secret().as_bytes())
            .map_err(|_| AppError::DependencyUnavailable {
            code: "rate_limit_key_invalid",
        })?;
        mac.update(command.purpose.as_str().as_bytes());
        mac.update(&[0]);
        mac.update(command.normalized_identifier.as_bytes());
        mac.update(&[0]);
        mac.update(command.ip_prefix.as_bytes());
        let mut key = [0_u8; 32];
        key.copy_from_slice(&mac.finalize().into_bytes());
        let (capacity, refill_per_second) = command.purpose.policy();
        match self
            .repository
            .consume(TokenBucketClaim {
                key,
                purpose: command.purpose.as_str(),
                capacity,
                refill_per_second,
                now,
            })
            .await
            .map_err(|_| AppError::DependencyUnavailable {
                code: "rate_limit_unavailable",
            })? {
            TokenBucketOutcome::Consumed => Ok(RateLimitDecision::Allowed),
            TokenBucketOutcome::Denied {
                retry_after_seconds,
            } => Ok(RateLimitDecision::Denied {
                retry_after: Duration::from_secs(retry_after_seconds),
            }),
        }
    }
}

#[async_trait]
impl<R: RateLimitRepository, C: Clock> RateLimitUseCase for DurableRateLimiter<R, C> {
    async fn check_rate_limit(
        &self,
        command: CheckRateLimit,
    ) -> Result<RateLimitDecision, AppError> {
        self.check_at(command, self.clock.now()).await
    }
}
