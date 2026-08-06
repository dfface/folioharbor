use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use folioharbor_domain::{identity::NormalizedEmail, time::OffsetDateTime};
use secrecy::{ExposeSecret, SecretString};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use thiserror::Error;

use crate::{config::ApplicationSecretRing, ports::NewMailOutboxEntry};

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
    pub(crate) invitation_library_id: Option<uuid::Uuid>,
}

#[derive(Clone)]
pub struct MailOutbox {
    secrets: Arc<ApplicationSecretRing>,
}

#[derive(Debug, Error)]
pub enum MailOutboxError {
    #[error("mail outbox encryption failed")]
    Encryption,
    #[error("mail intent context is invalid")]
    InvalidContext,
}

pub trait MailIntentSealer: Send + Sync {
    /// Encrypts a mail intent for persistence in the business transaction.
    ///
    /// # Errors
    ///
    /// Returns [`MailOutboxError`] when the template context is incomplete or encryption fails.
    fn seal(
        &self,
        message: MailMessage,
        now: OffsetDateTime,
        expires_at: OffsetDateTime,
    ) -> Result<NewMailOutboxEntry, MailOutboxError>;
}

impl MailOutbox {
    #[must_use]
    pub const fn new(secrets: Arc<ApplicationSecretRing>) -> Self {
        Self { secrets }
    }

    /// Encrypts a one-time token for a combined business/outbox repository operation.
    ///
    /// # Errors
    ///
    /// Returns [`MailOutboxError`] when the template context is incomplete or encryption fails.
    pub fn enqueue(
        &self,
        message: MailMessage,
        now: OffsetDateTime,
        expires_at: OffsetDateTime,
    ) -> Result<NewMailOutboxEntry, MailOutboxError> {
        self.seal(message, now, expires_at)
    }
}

impl MailIntentSealer for MailOutbox {
    fn seal(
        &self,
        message: MailMessage,
        now: OffsetDateTime,
        expires_at: OffsetDateTime,
    ) -> Result<NewMailOutboxEntry, MailOutboxError> {
        if message.template == MailTemplate::Invitation
            && (message.invitation_library_id.is_none() || message.invitation_context.is_none())
        {
            return Err(MailOutboxError::InvalidContext);
        }
        let current = self.secrets.current_for_encryption();
        let mut key = Sha256::digest(current.secret().expose_secret().as_bytes());
        let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| MailOutboxError::Encryption)?;
        let mut nonce_bytes = [0_u8; 12];
        getrandom::fill(&mut nonce_bytes).map_err(|_| MailOutboxError::Encryption)?;
        let nonce = Nonce::from_slice(&nonce_bytes);
        let context = authenticated_context(
            message.template.code(),
            1,
            message.locale.as_str(),
            message.recipient_account_id(),
            message.recipient().as_str(),
            message.invitation_library_id,
            message
                .invitation_context
                .as_ref()
                .map(|(_, role)| role.as_str()),
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
        Ok(NewMailOutboxEntry {
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
            invitation_library_id: message.invitation_library_id,
            invitation_role: message
                .invitation_context
                .as_ref()
                .map(|(_, role)| role.clone()),
            next_run_at: now,
            expires_at,
        })
    }
}

pub(crate) fn authenticated_context(
    template_code: &str,
    template_version: u16,
    locale: &str,
    recipient_account_id: Option<uuid::Uuid>,
    delivery_address: &str,
    invitation_library_id: Option<uuid::Uuid>,
    invitation_role: Option<&str>,
) -> String {
    format!(
        "mail|{template_code}|{template_version}|{locale}|{}|{delivery_address}|{}|{}",
        recipient_account_id.map_or_else(String::new, |id| id.to_string()),
        invitation_library_id.map_or_else(String::new, |id| id.to_string()),
        invitation_role.unwrap_or_default(),
    )
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
            invitation_library_id: None,
        }
    }

    pub fn set_invitation_context(&mut self, library_name: &str, role: &str) {
        self.invitation_context = Some((library_name.to_owned(), role.to_owned()));
    }

    pub fn set_invitation_repository_context(&mut self, library_id: uuid::Uuid, role: &str) {
        self.invitation_library_id = Some(library_id);
        self.invitation_context = Some((library_id.to_string(), role.to_owned()));
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

    #[must_use]
    pub const fn template(&self) -> MailTemplate {
        self.template
    }

    #[must_use]
    pub const fn locale(&self) -> Locale {
        self.locale
    }

    #[must_use]
    pub const fn invitation_library_id(&self) -> Option<uuid::Uuid> {
        self.invitation_library_id
    }

    #[must_use]
    pub fn invitation_role(&self) -> Option<&str> {
        self.invitation_context
            .as_ref()
            .map(|(_, role)| role.as_str())
    }
}
