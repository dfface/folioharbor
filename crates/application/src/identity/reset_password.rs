use folioharbor_domain::{
    id::{SessionId, UserId},
    identity::{CsrfToken, NormalizedEmail, PasswordResetToken, SessionToken},
};
use secrecy::{ExposeSecret, SecretString};

use crate::{
    error::{AppError, FieldViolation},
    mail::{Locale, MailIntentSealer, MailMessage, MailTemplate},
    ports::{Clock, IdentityRepository, PasswordHasher, PasswordResetSession, RandomSource},
};

use super::{RESET_LIFETIME, SESSION_ABSOLUTE_LIFETIME, SESSION_IDLE_LIFETIME, internal_error};

pub struct RequestPasswordResetCommand {
    pub email: String,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PasswordResetRequested;

pub struct RequestPasswordReset<'a, R, M, C, N> {
    repository: &'a R,
    mailer: &'a M,
    clock: &'a C,
    random: &'a N,
}
impl<'a, R, M, C, N> RequestPasswordReset<'a, R, M, C, N> {
    #[must_use]
    pub const fn new(repository: &'a R, mailer: &'a M, clock: &'a C, random: &'a N) -> Self {
        Self {
            repository,
            mailer,
            clock,
            random,
        }
    }
}
impl<R: IdentityRepository, M: MailIntentSealer, C: Clock, N: RandomSource>
    RequestPasswordReset<'_, R, M, C, N>
{
    /// Requests a password reset without revealing whether the account exists.
    ///
    /// # Errors
    ///
    /// Returns [`AppError`] when persistence fails.
    pub async fn execute(
        &self,
        command: RequestPasswordResetCommand,
    ) -> Result<PasswordResetRequested, AppError> {
        let Ok(email) = NormalizedEmail::parse(&command.email) else {
            return Ok(PasswordResetRequested);
        };
        let recipient_account_id = self
            .repository
            .mail_recipient_account_id(&email)
            .await
            .map_err(|_| internal_error())?;
        let authenticated_context_id = recipient_account_id.unwrap_or_else(UserId::new);
        let mut bytes = [0_u8; 32];
        self.random.fill(&mut bytes);
        let token = PasswordResetToken::from_random_bytes(bytes);
        let token_hash = token.hash_for_storage();
        let now = self.clock.now();
        let mail = self
            .mailer
            .seal(
                MailMessage::new(
                    Some(authenticated_context_id.as_uuid()),
                    email.clone(),
                    MailTemplate::PasswordReset,
                    Locale::En,
                    token.into_secret(),
                ),
                now,
                now + RESET_LIFETIME,
            )
            .map_err(|_| internal_error())?;
        if recipient_account_id.is_none() {
            return Ok(PasswordResetRequested);
        }
        let _issued = self
            .repository
            .issue_password_reset_with_mail(&email, token_hash, now, now + RESET_LIFETIME, mail)
            .await
            .map_err(|_| internal_error())?;
        Ok(PasswordResetRequested)
    }
}

pub struct CompletePasswordResetCommand {
    pub token: SecretString,
    pub new_password: SecretString,
}
#[derive(Debug)]
pub struct PasswordResetComplete {
    pub user_id: UserId,
    pub session_id: SessionId,
    pub session_token: SecretString,
    pub csrf_token: SecretString,
}

pub struct CompletePasswordReset<'a, R, H, C, N> {
    repository: &'a R,
    password_hasher: &'a H,
    clock: &'a C,
    random: &'a N,
}
impl<'a, R, H, C, N> CompletePasswordReset<'a, R, H, C, N> {
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
impl<R: IdentityRepository, H: PasswordHasher, C: Clock, N: RandomSource>
    CompletePasswordReset<'_, R, H, C, N>
{
    /// Consumes a reset token, replaces the password, and revokes active sessions.
    ///
    /// # Errors
    ///
    /// Returns [`AppError`] for invalid input, an invalid token, or dependency failure.
    pub async fn execute(
        &self,
        command: CompletePasswordResetCommand,
    ) -> Result<PasswordResetComplete, AppError> {
        if command.new_password.expose_secret().is_empty() {
            return Err(AppError::Invalid {
                code: "invalid_password",
                fields: vec![FieldViolation {
                    field: "new_password",
                    code: "required",
                }],
            });
        }
        let hash = PasswordResetToken::parse(command.token).hash_for_storage();
        let password_hash = self
            .password_hasher
            .hash(&command.new_password)
            .map_err(|_| internal_error())?;
        let mut session_bytes = [0_u8; 32];
        let mut csrf_bytes = [0_u8; 32];
        self.random.fill(&mut session_bytes);
        self.random.fill(&mut csrf_bytes);
        let session_token = SessionToken::from_random_bytes(session_bytes);
        let csrf_token = CsrfToken::from_random_bytes(csrf_bytes);
        let session_id = SessionId::new();
        let now = self.clock.now();
        self.repository
            .reset_password(
                hash,
                password_hash,
                PasswordResetSession {
                    session_id,
                    session_token_hash: session_token.hash_for_storage(),
                    csrf_token_hash: csrf_token.hash_for_storage(),
                    created_at: now,
                    idle_expires_at: now + SESSION_IDLE_LIFETIME,
                    absolute_expires_at: now + SESSION_ABSOLUTE_LIFETIME,
                },
                now,
            )
            .await
            .map_err(|_| internal_error())?
            .map(|user_id| PasswordResetComplete {
                user_id,
                session_id,
                session_token: session_token.into_secret(),
                csrf_token: csrf_token.into_secret(),
            })
            .ok_or(AppError::Invalid {
                code: "invalid_or_expired_password_reset_token",
                fields: Vec::new(),
            })
    }
}
