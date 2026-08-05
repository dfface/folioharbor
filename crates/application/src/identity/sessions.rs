use folioharbor_domain::{
    id::SessionId,
    identity::{SessionStatus, TokenHash},
    time::OffsetDateTime,
};
use secrecy::SecretString;

use super::{SESSION_IDLE_LIFETIME, internal_error};
use crate::{
    actor::Actor,
    error::AppError,
    ports::{Clock, IdentityRepository},
};
use folioharbor_domain::identity::{SessionRevocationReason, SessionToken};

#[derive(Debug)]
pub struct AuthenticateSessionCommand {
    pub session_token: SecretString,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AuthenticatedSession {
    pub actor: Actor,
    pub csrf_token_hash: TokenHash,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SafeSession {
    pub session_id: SessionId,
    pub is_current: bool,
    pub status: SessionStatus,
    pub created_at: Option<OffsetDateTime>,
    pub last_seen_at: Option<OffsetDateTime>,
    pub idle_expires_at: Option<OffsetDateTime>,
    pub absolute_expires_at: Option<OffsetDateTime>,
}

impl SafeSession {
    #[must_use]
    pub const fn active(session_id: SessionId, is_current: bool) -> Self {
        Self {
            session_id,
            is_current,
            status: SessionStatus::Active,
            created_at: None,
            last_seen_at: None,
            idle_expires_at: None,
            absolute_expires_at: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RevokeSessionCommand {
    pub actor: Actor,
    pub session_id: SessionId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RevokeSessionOutcome {
    pub revoked_current: bool,
}

pub struct AuthenticateSession<'a, R, C> {
    repository: &'a R,
    clock: &'a C,
}
impl<'a, R, C> AuthenticateSession<'a, R, C> {
    #[must_use]
    pub const fn new(repository: &'a R, clock: &'a C) -> Self {
        Self { repository, clock }
    }
}
impl<R: IdentityRepository, C: Clock> AuthenticateSession<'_, R, C> {
    /// Authenticates an opaque session secret and advances idle activity.
    ///
    /// # Errors
    /// Returns an internal error when persistence is unavailable.
    pub async fn execute(
        &self,
        command: AuthenticateSessionCommand,
    ) -> Result<Option<AuthenticatedSession>, AppError> {
        let now = self.clock.now();
        let hash = SessionToken::parse(command.session_token).hash_for_storage();
        Ok(self
            .repository
            .authenticate_session(hash, now, now + SESSION_IDLE_LIFETIME)
            .await
            .map_err(|_| internal_error())?
            .map(|principal| AuthenticatedSession {
                actor: Actor {
                    user_id: principal.user_id,
                    session_id: principal.session_id,
                },
                csrf_token_hash: principal.csrf_token_hash,
            }))
    }
}
pub struct CurrentSession<'a, R, C> {
    repository: &'a R,
    clock: &'a C,
}
impl<'a, R, C> CurrentSession<'a, R, C> {
    #[must_use]
    pub const fn new(repository: &'a R, clock: &'a C) -> Self {
        Self { repository, clock }
    }
}
impl<R: IdentityRepository, C: Clock> CurrentSession<'_, R, C> {
    /// Returns safe metadata for the actor's current session.
    ///
    /// # Errors
    /// Returns not found for a missing session or internal error on persistence failure.
    pub async fn execute(&self, actor: Actor) -> Result<SafeSession, AppError> {
        self.repository
            .list_user_sessions(actor.user_id)
            .await
            .map_err(|_| internal_error())?
            .into_iter()
            .find(|record| record.session_id == actor.session_id)
            .map(|record| safe(record, actor.session_id, self.clock.now()))
            .ok_or(AppError::NotFound {
                code: "session_not_found",
            })
    }
}
pub struct ListSessions<'a, R, C> {
    repository: &'a R,
    clock: &'a C,
}
impl<'a, R, C> ListSessions<'a, R, C> {
    #[must_use]
    pub const fn new(repository: &'a R, clock: &'a C) -> Self {
        Self { repository, clock }
    }
}
impl<R: IdentityRepository, C: Clock> ListSessions<'_, R, C> {
    /// Lists only safe metadata for sessions owned by the actor.
    ///
    /// # Errors
    /// Returns an internal error when persistence is unavailable.
    pub async fn execute(&self, actor: Actor) -> Result<Vec<SafeSession>, AppError> {
        let now = self.clock.now();
        Ok(self
            .repository
            .list_user_sessions(actor.user_id)
            .await
            .map_err(|_| internal_error())?
            .into_iter()
            .map(|record| safe(record, actor.session_id, now))
            .collect())
    }
}
pub struct RevokeSession<'a, R, C> {
    repository: &'a R,
    clock: &'a C,
}
impl<'a, R, C> RevokeSession<'a, R, C> {
    #[must_use]
    pub const fn new(repository: &'a R, clock: &'a C) -> Self {
        Self { repository, clock }
    }
}
impl<R: IdentityRepository, C: Clock> RevokeSession<'_, R, C> {
    /// Revokes a session only when it belongs to the actor.
    ///
    /// # Errors
    /// Returns not found for a foreign/missing session or internal error on persistence failure.
    pub async fn execute(
        &self,
        command: RevokeSessionCommand,
    ) -> Result<RevokeSessionOutcome, AppError> {
        let found = self
            .repository
            .revoke_user_session(
                command.actor.user_id,
                command.session_id,
                self.clock.now(),
                SessionRevocationReason::UserRevoked,
            )
            .await
            .map_err(|_| internal_error())?;
        if !found {
            return Err(AppError::NotFound {
                code: "session_not_found",
            });
        }
        Ok(RevokeSessionOutcome {
            revoked_current: command.session_id == command.actor.session_id,
        })
    }
}
fn safe(
    record: crate::ports::SessionRecord,
    current: SessionId,
    now: OffsetDateTime,
) -> SafeSession {
    let status = SessionStatus::at(
        record.revoked_at,
        record.last_seen_at,
        record.idle_expires_at,
        record.absolute_expires_at,
        now,
    );
    SafeSession {
        session_id: record.session_id,
        is_current: record.session_id == current,
        status,
        created_at: Some(record.created_at),
        last_seen_at: Some(record.last_seen_at),
        idle_expires_at: Some(record.idle_expires_at),
        absolute_expires_at: Some(record.absolute_expires_at),
    }
}
