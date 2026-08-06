use std::sync::Arc;

use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use secrecy::{ExposeSecret, SecretString};
use sha2::{Digest, Sha256};
use thiserror::Error;
use url::Url;
use zeroize::Zeroize;

use crate::{
    config::{ApplicationSecretRing, PublicUrl},
    ports::{LeaseMail, LeasedMail, MailRepository, Mailer},
};
use folioharbor_domain::{identity::NormalizedEmail, time::OffsetDateTime};
use time::Duration;

use super::{Locale, MailMessage, MailTemplate, enqueue::authenticated_context};

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("public base URL must be an absolute HTTP(S) URL with a host")]
    InvalidBaseUrl,
}

#[derive(Eq, PartialEq)]
pub struct RenderedMail {
    pub subject: String,
    pub text: String,
    pub html: String,
}

impl Drop for RenderedMail {
    fn drop(&mut self) {
        self.subject.zeroize();
        self.text.zeroize();
        self.html.zeroize();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeliveryFailure {
    SmtpStatus(u16),
    Transport,
}

impl DeliveryFailure {
    #[must_use]
    pub const fn smtp_status(status: u16) -> Self {
        Self::SmtpStatus(status)
    }

    #[must_use]
    pub fn is_transient(self) -> bool {
        match self {
            Self::SmtpStatus(status) => (400..500).contains(&status),
            Self::Transport => true,
        }
    }
}

/// Renders both MIME alternatives only when the requested origin matches configuration.
///
/// # Errors
///
/// Returns [`RenderError`] when the base URL is not an HTTP(S) origin or differs from the
/// validated configured origin.
pub fn render_message(
    message: &MailMessage,
    base: &Url,
    configured_base: &PublicUrl,
) -> Result<RenderedMail, RenderError> {
    let configured = configured_base.as_url();
    if !matches!(base.scheme(), "http" | "https")
        || base.host_str().is_none()
        || base.scheme() != configured.scheme()
        || base.host_str() != configured.host_str()
        || base.port_or_known_default() != configured.port_or_known_default()
    {
        return Err(RenderError::InvalidBaseUrl);
    }
    let path = match message.template {
        MailTemplate::Verification => "verify-email",
        MailTemplate::Invitation => "accept-invitation",
        MailTemplate::PasswordReset => "reset-password",
    };
    let mut link = base.join(path).map_err(|_| RenderError::InvalidBaseUrl)?;
    link.query_pairs_mut().append_pair("token", message.token());
    let mut link: String = link.into();
    let (subject, heading, text_intro) = copy(message.locale, message.template);
    let (library_label, role_label) = context_labels(message.locale);
    let context = message
        .invitation_context
        .as_ref()
        .map_or_else(String::new, |(library, role)| {
            format!("\n{library_label}{library}\n{role_label}{role}\n")
        });
    let text = format!("{text_intro}{context}\n{link}\n");
    let html_context =
        message
            .invitation_context
            .as_ref()
            .map_or_else(String::new, |(library, role)| {
                format!(
                    "<p>{library_label}{}<br>{role_label}{}</p>",
                    escape(library),
                    escape(role)
                )
            });
    let html = format!(
        "<!doctype html><html><body><h1>{heading}</h1><p>{}</p>{html_context}<p><a href=\"{}\">{heading}</a></p></body></html>",
        escape(text_intro.trim()),
        escape(&link)
    );
    let rendered = RenderedMail {
        subject: subject.to_owned(),
        text,
        html,
    };
    link.zeroize();
    Ok(rendered)
}

#[derive(Debug, Error)]
#[error("mail delivery pipeline failed")]
pub struct MailDeliveryError;

pub struct DeliverMailJob<'a, R, M> {
    repository: &'a R,
    mailer: &'a M,
    secrets: Arc<ApplicationSecretRing>,
    public_base_url: PublicUrl,
    owner: String,
}

impl<'a, R, M> DeliverMailJob<'a, R, M> {
    #[must_use]
    pub fn new(
        repository: &'a R,
        mailer: &'a M,
        secrets: Arc<ApplicationSecretRing>,
        public_base_url: PublicUrl,
        owner: String,
    ) -> Self {
        Self {
            repository,
            mailer,
            secrets,
            public_base_url,
            owner,
        }
    }
}

impl<R: MailRepository, M: Mailer> DeliverMailJob<'_, R, M> {
    /// Leases and delivers a bounded batch without logging message bodies or links.
    ///
    /// # Errors
    ///
    /// Returns [`MailDeliveryError`] when persistence fails or a lease transition is lost.
    pub async fn run_once(
        &self,
        now: OffsetDateTime,
        limit: u32,
    ) -> Result<usize, MailDeliveryError> {
        let leased = self
            .repository
            .lease(LeaseMail {
                owner: self.owner.clone(),
                now,
                lease_for: Duration::minutes(5),
                limit,
            })
            .await
            .map_err(|_| MailDeliveryError)?;
        let count = leased.len();
        for mail in leased {
            self.deliver_one(mail, now).await?;
        }
        Ok(count)
    }

    async fn deliver_one(
        &self,
        mail: LeasedMail,
        now: OffsetDateTime,
    ) -> Result<(), MailDeliveryError> {
        let prepared = prepare_message(&mail, &self.secrets, &self.public_base_url);
        let (recipient, rendered) = match prepared {
            Ok(prepared) => prepared,
            Err(code) => {
                return self.mark_failed(mail.mail_id, now, code).await;
            }
        };
        match self
            .mailer
            .deliver(&recipient, &mail.idempotency_key, &rendered)
            .await
        {
            Ok(()) => {
                let changed = self
                    .repository
                    .mark_sent(mail.mail_id, &self.owner, now)
                    .await
                    .map_err(|_| MailDeliveryError)?;
                changed.then_some(()).ok_or(MailDeliveryError)
            }
            Err(error) if error.is_transient() && mail.attempt < 5 => {
                let changed = self
                    .repository
                    .retry(
                        mail.mail_id,
                        &self.owner,
                        now,
                        retry_at(now, mail.attempt),
                        error.code(),
                    )
                    .await
                    .map_err(|_| MailDeliveryError)?;
                changed.then_some(()).ok_or(MailDeliveryError)
            }
            Err(error) => self.mark_failed(mail.mail_id, now, error.code()).await,
        }
    }

    async fn mark_failed(
        &self,
        mail_id: uuid::Uuid,
        now: OffsetDateTime,
        code: &'static str,
    ) -> Result<(), MailDeliveryError> {
        let changed = self
            .repository
            .mark_failed(mail_id, &self.owner, now, code)
            .await
            .map_err(|_| MailDeliveryError)?;
        changed.then_some(()).ok_or(MailDeliveryError)
    }
}

fn retry_at(now: OffsetDateTime, attempt: u32) -> OffsetDateTime {
    let seconds = match attempt {
        0 | 1 => 60,
        2 => 5 * 60,
        3 => 30 * 60,
        _ => 2 * 60 * 60,
    };
    now + Duration::seconds(seconds)
}

fn prepare_message(
    mail: &LeasedMail,
    secrets: &ApplicationSecretRing,
    public_base_url: &PublicUrl,
) -> Result<(NormalizedEmail, RenderedMail), &'static str> {
    if mail.template_version != 1 {
        return Err("unsupported_template_version");
    }
    let template = match mail.template_code.as_str() {
        "verification" => MailTemplate::Verification,
        "invitation" => MailTemplate::Invitation,
        "password_reset" => MailTemplate::PasswordReset,
        _ => return Err("unsupported_template"),
    };
    let locale = match mail.locale.as_str() {
        "en" => Locale::En,
        "zh-CN" => Locale::ZhCn,
        _ => return Err("unsupported_locale"),
    };
    let recipient =
        NormalizedEmail::parse(&mail.delivery_address).map_err(|_| "invalid_delivery_address")?;
    let secret = secrets
        .find_for_decryption(&mail.encryption_key_id)
        .ok_or("missing_encryption_key")?;
    if mail.nonce.len() != 12 {
        return Err("invalid_encryption_nonce");
    }
    let mut key = Sha256::digest(secret.secret().expose_secret().as_bytes());
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| "mail_decryption_failed")?;
    let context = authenticated_context(
        &mail.template_code,
        mail.template_version,
        &mail.locale,
        mail.recipient_account_id,
        &mail.delivery_address,
        mail.invitation_library_id,
        mail.invitation_role.as_deref(),
    );
    let decrypted = cipher.decrypt(
        Nonce::from_slice(&mail.nonce),
        Payload {
            msg: &mail.token_ciphertext,
            aad: context.as_bytes(),
        },
    );
    key.zeroize();
    let plaintext = decrypted.map_err(|_| "mail_decryption_failed")?;
    let token = match String::from_utf8(plaintext) {
        Ok(token) => token,
        Err(error) => {
            let mut bytes = error.into_bytes();
            bytes.zeroize();
            return Err("mail_decryption_failed");
        }
    };
    let mut message = MailMessage::new(
        mail.recipient_account_id,
        recipient.clone(),
        template,
        locale,
        SecretString::from(token.into_boxed_str()),
    );
    if template == MailTemplate::Invitation {
        let library_id = mail
            .invitation_library_id
            .ok_or("invalid_template_context")?;
        let role = mail
            .invitation_role
            .as_deref()
            .ok_or("invalid_template_context")?;
        message.set_invitation_context(&library_id.to_string(), role);
    }
    let rendered = render_message(&message, public_base_url.as_url(), public_base_url)
        .map_err(|_| "invalid_public_base_url")?;
    Ok((recipient, rendered))
}

fn copy(locale: Locale, template: MailTemplate) -> (&'static str, &'static str, &'static str) {
    match (locale, template) {
        (Locale::En, MailTemplate::Verification) => (
            "Verify your email",
            "Verify your email",
            "Verify your email with this single-use link.",
        ),
        (Locale::En, MailTemplate::Invitation) => (
            "You are invited",
            "You are invited",
            "Use this single-use link to accept your invitation.",
        ),
        (Locale::En, MailTemplate::PasswordReset) => (
            "Reset your password",
            "Reset your password",
            "Use this single-use link to reset your password.",
        ),
        (Locale::ZhCn, MailTemplate::Verification) => (
            "验证您的邮箱",
            "验证您的邮箱",
            "请使用此一次性链接验证您的邮箱。",
        ),
        (Locale::ZhCn, MailTemplate::Invitation) => (
            "您收到了邀请",
            "您收到了邀请",
            "请使用此一次性链接接受邀请。",
        ),
        (Locale::ZhCn, MailTemplate::PasswordReset) => (
            "重置您的密码",
            "重置您的密码",
            "请使用此一次性链接重置密码。",
        ),
    }
}

const fn context_labels(locale: Locale) -> (&'static str, &'static str) {
    match locale {
        Locale::En => ("Library: ", "Role: "),
        Locale::ZhCn => ("图书馆：", "角色："),
    }
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
