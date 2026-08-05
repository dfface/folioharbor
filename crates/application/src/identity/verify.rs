use folioharbor_domain::{id::UserId, identity::EmailVerificationToken};
use secrecy::SecretString;

use crate::{
    error::AppError,
    ports::{Clock, IdentityRepository},
};

use super::internal_error;

pub struct VerifyEmailCommand {
    pub token: SecretString,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VerifiedAccount {
    pub user_id: UserId,
}

pub struct VerifyEmail<'a, R, C> {
    repository: &'a R,
    clock: &'a C,
}

impl<'a, R, C> VerifyEmail<'a, R, C> {
    #[must_use]
    pub const fn new(repository: &'a R, clock: &'a C) -> Self {
        Self { repository, clock }
    }
}

impl<R: IdentityRepository, C: Clock> VerifyEmail<'_, R, C> {
    /// Consumes an unexpired single-use verification token.
    ///
    /// # Errors
    ///
    /// Returns [`AppError`] when the token is invalid or persistence fails.
    pub async fn execute(&self, command: VerifyEmailCommand) -> Result<VerifiedAccount, AppError> {
        let hash = EmailVerificationToken::parse(command.token).hash_for_storage();
        self.repository
            .verify_email(hash, self.clock.now())
            .await
            .map_err(|_| internal_error())?
            .map(|user_id| VerifiedAccount { user_id })
            .ok_or(AppError::Invalid {
                code: "invalid_or_expired_verification_token",
                fields: Vec::new(),
            })
    }
}
