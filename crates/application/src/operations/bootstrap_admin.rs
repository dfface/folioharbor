use async_trait::async_trait;
use folioharbor_domain::{id::UserId, identity::NormalizedEmail, time::OffsetDateTime};
use secrecy::{ExposeSecret as _, SecretString};

use crate::ports::{Clock, PasswordHasher};

#[derive(Debug)]
pub struct BootstrapAdminCommand {
    pub email: String,
    pub password: SecretString,
}

#[derive(Debug)]
pub struct NewSystemAdministrator {
    pub user_id: UserId,
    pub normalized_email: NormalizedEmail,
    pub display_email: String,
    pub password_hash: String,
    pub created_at: OffsetDateTime,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootstrapAdminOutcome {
    Created,
    AlreadyAdministrator,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("system administrator bootstrap persistence failed")]
pub struct BootstrapAdminRepositoryError;

#[async_trait]
pub trait BootstrapAdminRepository: Send + Sync {
    /// Atomically creates a verified account, password credential, and system-admin row.
    async fn bootstrap_admin(
        &self,
        administrator: NewSystemAdministrator,
    ) -> Result<BootstrapAdminOutcome, BootstrapAdminRepositoryError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum BootstrapAdminError {
    #[error("administrator email is invalid")]
    InvalidEmail,
    #[error("administrator password is required")]
    PasswordRequired,
    #[error("administrator password could not be secured")]
    PasswordHash,
    #[error("administrator bootstrap is unavailable")]
    Repository,
}

pub struct BootstrapAdmin<'a, R, H, C> {
    repository: &'a R,
    password_hasher: &'a H,
    clock: &'a C,
}

impl<'a, R, H, C> BootstrapAdmin<'a, R, H, C> {
    #[must_use]
    pub const fn new(repository: &'a R, password_hasher: &'a H, clock: &'a C) -> Self {
        Self {
            repository,
            password_hasher,
            clock,
        }
    }
}

impl<R: BootstrapAdminRepository, H: PasswordHasher, C: Clock> BootstrapAdmin<'_, R, H, C> {
    /// Creates the first verified system administrator without library membership.
    ///
    /// # Errors
    ///
    /// Returns a safe validation or dependency classification without including credentials.
    pub async fn execute(
        &self,
        command: BootstrapAdminCommand,
    ) -> Result<BootstrapAdminOutcome, BootstrapAdminError> {
        let normalized_email = NormalizedEmail::parse(&command.email)
            .map_err(|_| BootstrapAdminError::InvalidEmail)?;
        if command.password.expose_secret().is_empty() {
            return Err(BootstrapAdminError::PasswordRequired);
        }
        let password_hash = self
            .password_hasher
            .hash(&command.password)
            .map_err(|_| BootstrapAdminError::PasswordHash)?;
        self.repository
            .bootstrap_admin(NewSystemAdministrator {
                user_id: UserId::new(),
                normalized_email,
                display_email: command.email.trim().to_owned(),
                password_hash,
                created_at: self.clock.now(),
            })
            .await
            .map_err(|_| BootstrapAdminError::Repository)
    }
}
