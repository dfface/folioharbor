use folioharbor_domain::{
    id::{SessionId, UserId},
    identity::{AccountStatus, CsrfToken, NormalizedEmail, SessionToken},
};
use secrecy::SecretString;

use crate::{
    error::AppError,
    ports::{Clock, IdentityRepository, NewSession, PasswordHasher, RandomSource},
};

use super::{SESSION_ABSOLUTE_LIFETIME, SESSION_IDLE_LIFETIME, internal_error};

pub struct LoginCommand {
    pub email: String,
    pub password: SecretString,
}

#[derive(Debug)]
pub struct IssuedSession {
    pub user_id: UserId,
    pub session_id: SessionId,
    pub session_token: SecretString,
    pub csrf_token: SecretString,
}

pub struct Login<'a, R, H, C, N> {
    repository: &'a R,
    password_hasher: &'a H,
    clock: &'a C,
    random: &'a N,
}

impl<'a, R, H, C, N> Login<'a, R, H, C, N> {
    #[must_use]
    pub const fn new(
        repository: &'a R,
        password_hasher: &'a H,
        clock: &'a C,
        random: &'a N,
    ) -> Self {
        Self {
            repository,
            password_hasher,
            clock,
            random,
        }
    }
}

impl<R: IdentityRepository, H: PasswordHasher, C: Clock, N: RandomSource> Login<'_, R, H, C, N> {
    /// Validates local credentials and issues a persisted opaque session.
    ///
    /// # Errors
    ///
    /// Returns [`AppError`] for invalid credentials, unverified accounts, or dependency failure.
    pub async fn execute(&self, command: LoginCommand) -> Result<IssuedSession, AppError> {
        let Ok(email) = NormalizedEmail::parse(&command.email) else {
            self.password_hasher.verify_dummy(&command.password);
            return Err(AppError::Unauthenticated);
        };
        let identity = self
            .repository
            .find_login_identity(&email)
            .await
            .map_err(|_| internal_error())?;
        let Some(identity) = identity else {
            self.password_hasher.verify_dummy(&command.password);
            return Err(AppError::Unauthenticated);
        };
        if !self
            .password_hasher
            .verify(&command.password, &identity.password_hash)
        {
            return Err(AppError::Unauthenticated);
        }
        if identity.status != AccountStatus::Verified {
            return Err(AppError::Forbidden {
                code: "email_verification_required",
            });
        }
        let mut session_bytes = [0_u8; 32];
        let mut csrf_bytes = [0_u8; 32];
        self.random.fill(&mut session_bytes);
        self.random.fill(&mut csrf_bytes);
        let session_token = SessionToken::from_random_bytes(session_bytes);
        let csrf_token = CsrfToken::from_random_bytes(csrf_bytes);
        let session_id = SessionId::new();
        let now = self.clock.now();
        self.repository
            .create_session(NewSession {
                session_id,
                user_id: identity.user_id,
                session_token_hash: session_token.hash_for_storage(),
                csrf_token_hash: csrf_token.hash_for_storage(),
                created_at: now,
                idle_expires_at: now + SESSION_IDLE_LIFETIME,
                absolute_expires_at: now + SESSION_ABSOLUTE_LIFETIME,
            })
            .await
            .map_err(|_| internal_error())?;
        Ok(IssuedSession {
            user_id: identity.user_id,
            session_id,
            session_token: session_token.into_secret(),
            csrf_token: csrf_token.into_secret(),
        })
    }
}
