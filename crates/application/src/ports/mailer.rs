use async_trait::async_trait;
use folioharbor_domain::{id::LibraryId, identity::NormalizedEmail, libraries::role::RoleCode};
use secrecy::SecretString;
use thiserror::Error;

#[derive(Debug, Error)]
#[error("mail delivery failed")]
pub struct MailError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LibraryInvitationContext {
    pub library_id: LibraryId,
    pub role: RoleCode,
}

#[async_trait]
pub trait Mailer: Send + Sync {
    async fn preflight_library_invitation(&self) -> Result<(), MailError>;

    async fn send_verification(
        &self,
        email: &NormalizedEmail,
        token: SecretString,
    ) -> Result<(), MailError>;

    async fn send_password_reset(
        &self,
        email: &NormalizedEmail,
        token: SecretString,
    ) -> Result<(), MailError>;

    async fn send_library_invitation(
        &self,
        recipient: &NormalizedEmail,
        context: LibraryInvitationContext,
        token: SecretString,
    ) -> Result<(), MailError>;
}
