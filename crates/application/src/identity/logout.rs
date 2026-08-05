use folioharbor_domain::identity::{SessionRevocationReason, SessionToken};
use secrecy::SecretString;

use crate::{
    error::AppError,
    ports::{Clock, IdentityRepository},
};

use super::internal_error;

pub struct LogoutCommand {
    pub session_token: SecretString,
}
pub struct Logout<'a, R, C> {
    repository: &'a R,
    clock: &'a C,
}

impl<'a, R, C> Logout<'a, R, C> {
    #[must_use]
    pub const fn new(repository: &'a R, clock: &'a C) -> Self {
        Self { repository, clock }
    }
}

impl<R: IdentityRepository, C: Clock> Logout<'_, R, C> {
    /// Revokes a session, treating absent or already-revoked sessions as success.
    ///
    /// # Errors
    ///
    /// Returns [`AppError`] when persistence is unavailable.
    pub async fn execute(&self, command: LogoutCommand) -> Result<(), AppError> {
        self.repository
            .revoke_session(
                SessionToken::parse(command.session_token).hash_for_storage(),
                self.clock.now(),
                SessionRevocationReason::Logout,
            )
            .await
            .map_err(|_| internal_error())
    }
}
