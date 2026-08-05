use folioharbor_domain::{
    id::UserId,
    identity::{NormalizedEmail, PasswordResetToken},
};
use secrecy::{ExposeSecret, SecretString};

use crate::{
    error::{AppError, FieldViolation},
    ports::{Clock, IdentityRepository, Mailer, PasswordHasher, RandomSource},
};

use super::{RESET_LIFETIME, internal_error};

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
impl<R: IdentityRepository, M: Mailer, C: Clock, N: RandomSource>
    RequestPasswordReset<'_, R, M, C, N>
{
    /// Requests a password reset without revealing whether the account exists.
    ///
    /// # Errors
    ///
    /// Returns [`AppError`] when persistence or mail delivery fails.
    pub async fn execute(
        &self,
        command: RequestPasswordResetCommand,
    ) -> Result<PasswordResetRequested, AppError> {
        let Ok(email) = NormalizedEmail::parse(&command.email) else {
            return Ok(PasswordResetRequested);
        };
        let mut bytes = [0_u8; 32];
        self.random.fill(&mut bytes);
        let token = PasswordResetToken::from_random_bytes(bytes);
        let now = self.clock.now();
        let issued = self
            .repository
            .issue_password_reset(&email, token.hash_for_storage(), now, now + RESET_LIFETIME)
            .await
            .map_err(|_| internal_error())?;
        if issued {
            self.mailer
                .send_password_reset(&email, token.into_secret())
                .await
                .map_err(|_| internal_error())?;
        }
        Ok(PasswordResetRequested)
    }
}

pub struct CompletePasswordResetCommand {
    pub token: SecretString,
    pub new_password: SecretString,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PasswordResetComplete {
    pub user_id: UserId,
}

pub struct CompletePasswordReset<'a, R, H, C> {
    repository: &'a R,
    password_hasher: &'a H,
    clock: &'a C,
}
impl<'a, R, H, C> CompletePasswordReset<'a, R, H, C> {
    #[must_use]
    pub const fn new(repository: &'a R, password_hasher: &'a H, clock: &'a C) -> Self {
        Self {
            repository,
            password_hasher,
            clock,
        }
    }
}
impl<R: IdentityRepository, H: PasswordHasher, C: Clock> CompletePasswordReset<'_, R, H, C> {
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
        self.repository
            .reset_password(hash, password_hash, self.clock.now())
            .await
            .map_err(|_| internal_error())?
            .map(|user_id| PasswordResetComplete { user_id })
            .ok_or(AppError::Invalid {
                code: "invalid_or_expired_password_reset_token",
                fields: Vec::new(),
            })
    }
}
