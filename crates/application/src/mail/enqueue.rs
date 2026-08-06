use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use folioharbor_domain::{identity::NormalizedEmail, time::OffsetDateTime};
use secrecy::{ExposeSecret, SecretString};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    config::ApplicationSecretRing,
    ports::{MailRepository, MailRepositoryError, NewMailOutboxEntry},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Locale {
    En,
    ZhCn,
}

impl Locale {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::ZhCn => "zh-CN",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MailTemplate {
    Verification,
    Invitation,
    PasswordReset,
}

impl MailTemplate {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Verification => "verification",
            Self::Invitation => "invitation",
            Self::PasswordReset => "password_reset",
        }
    }
}

pub struct MailMessage {
    pub(crate) mail_id: uuid::Uuid,
    pub(crate) recipient_account_id: Option<uuid::Uuid>,
    pub(crate) recipient: NormalizedEmail,
    pub(crate) template: MailTemplate,
    pub(crate) locale: Locale,
    pub(crate) token: SecretString,
    pub(crate) invitation_context: Option<(String, String)>,
}

pub struct MailOutbox<'a, R> {
    repository: &'a R,
    secrets: &'a ApplicationSecretRing,
}

#[derive(Debug, Error)]
pub enum MailOutboxError {
    #[error("mail outbox encryption failed")]
    Encryption,
    #[error(transparent)]
    Repository(#[from] MailRepositoryError),
}

impl<'a, R> MailOutbox<'a, R> {
    #[must_use]
    pub const fn new(repository: &'a R, secrets: &'a ApplicationSecretRing) -> Self {
        Self {
            repository,
            secrets,
        }
    }
}

impl<R: MailRepository> MailOutbox<'_, R> {
    /// Encrypts a one-time token with authenticated template and recipient context before persistence.
    pub async fn enqueue(
        &self,
        message: MailMessage,
        now: OffsetDateTime,
        expires_at: OffsetDateTime,
    ) -> Result<uuid::Uuid, MailOutboxError> {
        let current = self.secrets.current_for_encryption();
        let mut key = Sha256::digest(current.secret().expose_secret().as_bytes());
        let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| MailOutboxError::Encryption)?;
        let mut nonce_bytes = [0_u8; 12];
        getrandom::fill(&mut nonce_bytes).map_err(|_| MailOutboxError::Encryption)?;
        let nonce = Nonce::from_slice(&nonce_bytes);
        let context = format!(
            "{}:{}:{}",
            message.template.code(),
            message
                .recipient_account_id()
                .map_or_else(String::new, |id| id.to_string()),
            message.recipient().as_str()
        );
        let ciphertext = cipher
            .encrypt(
                nonce,
                Payload {
                    msg: message.token().as_bytes(),
                    aad: context.as_bytes(),
                },
            )
            .map_err(|_| MailOutboxError::Encryption)?;
        key.fill(0);
        self.repository
            .enqueue(NewMailOutboxEntry {
                mail_id: message.mail_id(),
                recipient_account_id: message.recipient_account_id(),
                delivery_address: message.recipient().as_str().to_owned(),
                template_code: message.template.code(),
                template_version: 1,
                locale: message.locale.as_str(),
                token_ciphertext: ciphertext,
                encryption_key_id: current.key_id().as_str().to_owned(),
                nonce: nonce.to_vec(),
                idempotency_key: message.idempotency_key(),
                next_run_at: now,
                expires_at,
            })
            .await
            .map_err(MailOutboxError::from)
    }
}

impl MailMessage {
    #[must_use]
    pub fn new(
        recipient_account_id: Option<uuid::Uuid>,
        recipient: NormalizedEmail,
        template: MailTemplate,
        locale: Locale,
        token: SecretString,
    ) -> Self {
        Self {
            mail_id: uuid::Uuid::now_v7(),
            recipient_account_id,
            recipient,
            template,
            locale,
            token,
            invitation_context: None,
        }
    }

    pub fn set_invitation_context(&mut self, library_name: &str, role: &str) {
        self.invitation_context = Some((library_name.to_owned(), role.to_owned()));
    }

    /// Stable for redelivery of this intent and intentionally token-free.
    #[must_use]
    pub fn idempotency_key(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.mail_id.as_bytes());
        format!("mail:{}:{:x}", self.template.code(), hasher.finalize())
    }

    #[must_use]
    pub(crate) fn token(&self) -> &str {
        self.token.expose_secret()
    }

    #[must_use]
    pub const fn recipient_account_id(&self) -> Option<uuid::Uuid> {
        self.recipient_account_id
    }

    #[must_use]
    pub fn recipient(&self) -> &NormalizedEmail {
        &self.recipient
    }

    #[must_use]
    pub const fn mail_id(&self) -> uuid::Uuid {
        self.mail_id
    }
}
