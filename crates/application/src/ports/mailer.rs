use async_trait::async_trait;
use folioharbor_domain::identity::NormalizedEmail;
use secrecy::SecretString;
use thiserror::Error;

#[derive(Debug, Error)]
#[error("mail delivery failed")]
pub struct MailError;

#[async_trait]
pub trait Mailer: Send + Sync {
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
}
