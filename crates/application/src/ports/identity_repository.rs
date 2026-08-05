use async_trait::async_trait;
use folioharbor_domain::{
    id::{SessionId, UserId},
    identity::{AccountStatus, NormalizedEmail, SessionRevocationReason, TokenHash},
    time::OffsetDateTime,
};
use thiserror::Error;

#[derive(Debug, Error)]
#[error("identity persistence failed")]
pub struct IdentityRepositoryError;

#[derive(Debug)]
pub struct NewAccount {
    pub user_id: UserId,
    pub normalized_email: NormalizedEmail,
    pub display_email: String,
    pub password_hash: String,
    pub verification_token_hash: TokenHash,
    pub created_at: OffsetDateTime,
    pub verification_expires_at: OffsetDateTime,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegisterOutcome {
    Created,
    Existing,
}

#[derive(Clone, Debug)]
pub struct LoginIdentity {
    pub user_id: UserId,
    pub password_hash: String,
    pub status: AccountStatus,
}

#[derive(Debug)]
pub struct NewSession {
    pub session_id: SessionId,
    pub user_id: UserId,
    pub session_token_hash: TokenHash,
    pub csrf_token_hash: TokenHash,
    pub created_at: OffsetDateTime,
    pub idle_expires_at: OffsetDateTime,
    pub absolute_expires_at: OffsetDateTime,
}

#[derive(Debug)]
pub struct PasswordResetSession {
    pub session_id: SessionId,
    pub session_token_hash: TokenHash,
    pub csrf_token_hash: TokenHash,
    pub created_at: OffsetDateTime,
    pub idle_expires_at: OffsetDateTime,
    pub absolute_expires_at: OffsetDateTime,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionPrincipal {
    pub user_id: UserId,
    pub session_id: SessionId,
    pub csrf_token_hash: TokenHash,
}

#[derive(Clone, Copy, Debug)]
pub struct SessionRecord {
    pub session_id: SessionId,
    pub created_at: OffsetDateTime,
    pub last_seen_at: OffsetDateTime,
    pub idle_expires_at: OffsetDateTime,
    pub absolute_expires_at: OffsetDateTime,
    pub revoked_at: Option<OffsetDateTime>,
}

#[async_trait]
pub trait IdentityRepository: Send + Sync {
    /// Atomically inserts an account, credential, and verification token.
    async fn register(
        &self,
        account: NewAccount,
    ) -> Result<RegisterOutcome, IdentityRepositoryError>;

    /// Atomically consumes one unexpired token and verifies its account.
    async fn verify_email(
        &self,
        token_hash: TokenHash,
        now: OffsetDateTime,
    ) -> Result<Option<UserId>, IdentityRepositoryError>;

    async fn find_login_identity(
        &self,
        email: &NormalizedEmail,
    ) -> Result<Option<LoginIdentity>, IdentityRepositoryError>;

    /// Atomically inserts a session with both hashed bearer secrets.
    async fn create_session(&self, session: NewSession) -> Result<(), IdentityRepositoryError>;

    /// Atomically validates expiry and advances idle activity.
    async fn authenticate_session(
        &self,
        token_hash: TokenHash,
        now: OffsetDateTime,
        new_idle_expires_at: OffsetDateTime,
    ) -> Result<Option<SessionPrincipal>, IdentityRepositoryError>;

    /// Atomically revokes the matching session; already absent/revoked is success.
    async fn revoke_session(
        &self,
        token_hash: TokenHash,
        now: OffsetDateTime,
        reason: SessionRevocationReason,
    ) -> Result<(), IdentityRepositoryError>;

    /// Atomically creates a reset token only if the normalized account exists.
    async fn issue_password_reset(
        &self,
        email: &NormalizedEmail,
        token_hash: TokenHash,
        created_at: OffsetDateTime,
        expires_at: OffsetDateTime,
    ) -> Result<bool, IdentityRepositoryError>;

    /// Atomically consumes the token, replaces the password, and revokes all sessions.
    async fn reset_password(
        &self,
        token_hash: TokenHash,
        password_hash: String,
        session: PasswordResetSession,
        now: OffsetDateTime,
    ) -> Result<Option<UserId>, IdentityRepositoryError>;

    async fn list_user_sessions(
        &self,
        _user_id: UserId,
    ) -> Result<Vec<SessionRecord>, IdentityRepositoryError> {
        Err(IdentityRepositoryError)
    }

    async fn revoke_user_session(
        &self,
        _user_id: UserId,
        _session_id: SessionId,
        _now: OffsetDateTime,
        _reason: SessionRevocationReason,
    ) -> Result<bool, IdentityRepositoryError> {
        Err(IdentityRepositoryError)
    }
}
