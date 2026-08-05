use async_trait::async_trait;
use folioharbor_application::ports::{
    IdentityRepository, IdentityRepositoryError, LoginIdentity, NewAccount, NewSession,
    RegisterOutcome, SessionPrincipal, SessionRecord,
};
use folioharbor_domain::{
    id::{SessionId, UserId},
    identity::{AccountStatus, NormalizedEmail, SessionRevocationReason, TokenHash},
    time::OffsetDateTime,
};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct PgIdentityRepository {
    pool: PgPool,
}

impl PgIdentityRepository {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn persistence_error(_: sqlx::Error) -> IdentityRepositoryError {
    IdentityRepositoryError
}

const fn revocation_reason_value(reason: SessionRevocationReason) -> &'static str {
    match reason {
        SessionRevocationReason::Logout => "logout",
        SessionRevocationReason::PasswordReset => "password_reset",
        SessionRevocationReason::UserRevoked => "user_revoked",
    }
}

#[async_trait]
impl IdentityRepository for PgIdentityRepository {
    async fn register(
        &self,
        account: NewAccount,
    ) -> Result<RegisterOutcome, IdentityRepositoryError> {
        let mut tx = self.pool.begin().await.map_err(persistence_error)?;
        let created = sqlx::query_scalar!(
            r#"SELECT folioharbor.identity_register($1, $2, $3, $4, $5, $6, $7, $8) AS "created!""#,
            account.user_id.as_uuid(),
            account.normalized_email.as_str(),
            account.display_email,
            account.password_hash,
            Uuid::now_v7(),
            account.verification_token_hash.as_bytes().as_slice(),
            account.created_at,
            account.verification_expires_at,
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(persistence_error)?;
        tx.commit().await.map_err(persistence_error)?;
        Ok(if created {
            RegisterOutcome::Created
        } else {
            RegisterOutcome::Existing
        })
    }

    async fn verify_email(
        &self,
        token_hash: TokenHash,
        now: OffsetDateTime,
    ) -> Result<Option<UserId>, IdentityRepositoryError> {
        let mut tx = self.pool.begin().await.map_err(persistence_error)?;
        let user_id = sqlx::query_scalar!(
            "SELECT folioharbor.identity_verify_email($1, $2)",
            token_hash.as_bytes().as_slice(),
            now,
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(persistence_error)?;
        tx.commit().await.map_err(persistence_error)?;
        Ok(user_id.map(UserId::from_uuid))
    }

    async fn find_login_identity(
        &self,
        email: &NormalizedEmail,
    ) -> Result<Option<LoginIdentity>, IdentityRepositoryError> {
        let row = sqlx::query!(
            r#"SELECT user_id AS "user_id!", status AS "status!", password_hash AS "password_hash!" FROM folioharbor.identity_find_login($1)"#,
            email.as_str(),
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(persistence_error)?;
        Ok(row.map(|row| LoginIdentity {
            user_id: UserId::from_uuid(row.user_id),
            password_hash: row.password_hash,
            status: match row.status.as_str() {
                "pending_verification" => AccountStatus::PendingVerification,
                "verified" => AccountStatus::Verified,
                _ => AccountStatus::Disabled,
            },
        }))
    }

    async fn create_session(&self, session: NewSession) -> Result<(), IdentityRepositoryError> {
        let mut tx = self.pool.begin().await.map_err(persistence_error)?;
        sqlx::query!(
            "SELECT folioharbor.identity_create_session($1, $2, $3, $4, $5, $6, $7)",
            session.session_id.as_uuid(),
            session.user_id.as_uuid(),
            session.session_token_hash.as_bytes().as_slice(),
            session.csrf_token_hash.as_bytes().as_slice(),
            session.created_at,
            session.idle_expires_at,
            session.absolute_expires_at,
        )
        .execute(&mut *tx)
        .await
        .map_err(persistence_error)?;
        tx.commit().await.map_err(persistence_error)
    }

    async fn authenticate_session(
        &self,
        token_hash: TokenHash,
        now: OffsetDateTime,
        new_idle_expires_at: OffsetDateTime,
    ) -> Result<Option<SessionPrincipal>, IdentityRepositoryError> {
        let row = sqlx::query!(
            r#"SELECT user_id AS "user_id!", session_id AS "session_id!", csrf_token_hash AS "csrf_token_hash!" FROM folioharbor.identity_authenticate_session($1, $2, $3)"#,
            token_hash.as_bytes().as_slice(), now, new_idle_expires_at,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(persistence_error)?;
        Ok(row.map(|row| SessionPrincipal {
            user_id: UserId::from_uuid(row.user_id),
            session_id: SessionId::from_uuid(row.session_id),
            csrf_token_hash: {
                let mut bytes = [0_u8; 32];
                bytes.copy_from_slice(&row.csrf_token_hash);
                TokenHash::from_bytes(bytes)
            },
        }))
    }

    async fn revoke_session(
        &self,
        token_hash: TokenHash,
        now: OffsetDateTime,
        reason: SessionRevocationReason,
    ) -> Result<(), IdentityRepositoryError> {
        let mut tx = self.pool.begin().await.map_err(persistence_error)?;
        sqlx::query!(
            "SELECT folioharbor.identity_revoke_session($1, $2, $3)",
            token_hash.as_bytes().as_slice(),
            now,
            revocation_reason_value(reason),
        )
        .execute(&mut *tx)
        .await
        .map_err(persistence_error)?;
        tx.commit().await.map_err(persistence_error)
    }

    async fn issue_password_reset(
        &self,
        email: &NormalizedEmail,
        token_hash: TokenHash,
        created_at: OffsetDateTime,
        expires_at: OffsetDateTime,
    ) -> Result<bool, IdentityRepositoryError> {
        let mut tx = self.pool.begin().await.map_err(persistence_error)?;
        let inserted = sqlx::query_scalar!(
            r#"SELECT folioharbor.identity_issue_password_reset($1, $2, $3, $4, $5) AS "inserted!""#,
            Uuid::now_v7(), email.as_str(), token_hash.as_bytes().as_slice(), created_at, expires_at,
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(persistence_error)?;
        tx.commit().await.map_err(persistence_error)?;
        Ok(inserted)
    }

    async fn reset_password(
        &self,
        token_hash: TokenHash,
        password_hash: String,
        now: OffsetDateTime,
    ) -> Result<Option<UserId>, IdentityRepositoryError> {
        let mut tx = self.pool.begin().await.map_err(persistence_error)?;
        let user_id = sqlx::query_scalar!(
            "SELECT folioharbor.identity_reset_password($1, $2, $3, $4)",
            token_hash.as_bytes().as_slice(),
            password_hash,
            now,
            revocation_reason_value(SessionRevocationReason::PasswordReset),
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(persistence_error)?;
        tx.commit().await.map_err(persistence_error)?;
        Ok(user_id.map(UserId::from_uuid))
    }

    async fn list_user_sessions(
        &self,
        user_id: UserId,
    ) -> Result<Vec<SessionRecord>, IdentityRepositoryError> {
        let rows = sqlx::query!(r#"SELECT session_id AS "session_id!", created_at AS "created_at!", last_seen_at AS "last_seen_at!", idle_expires_at AS "idle_expires_at!", absolute_expires_at AS "absolute_expires_at!", revoked_at FROM folioharbor.user_sessions WHERE user_id = $1 ORDER BY created_at DESC"#, user_id.as_uuid()).fetch_all(&self.pool).await.map_err(persistence_error)?;
        Ok(rows
            .into_iter()
            .map(|row| SessionRecord {
                session_id: SessionId::from_uuid(row.session_id),
                created_at: row.created_at,
                last_seen_at: row.last_seen_at,
                idle_expires_at: row.idle_expires_at,
                absolute_expires_at: row.absolute_expires_at,
                revoked_at: row.revoked_at,
            })
            .collect())
    }

    async fn revoke_user_session(
        &self,
        user_id: UserId,
        session_id: SessionId,
        now: OffsetDateTime,
        reason: SessionRevocationReason,
    ) -> Result<bool, IdentityRepositoryError> {
        let result = sqlx::query!("UPDATE folioharbor.user_sessions SET revoked_at = $3, revocation_reason = $4, version = version + 1 WHERE user_id = $1 AND session_id = $2 AND revoked_at IS NULL", user_id.as_uuid(), session_id.as_uuid(), now, revocation_reason_value(reason)).execute(&self.pool).await.map_err(persistence_error)?;
        Ok(result.rows_affected() == 1)
    }
}
