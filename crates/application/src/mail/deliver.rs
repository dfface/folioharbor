use super::{Locale, MailMessage, MailTemplate};
use thiserror::Error;
use url::Url;

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("public base URL must be an absolute HTTP(S) URL with a host")]
    InvalidBaseUrl,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderedMail {
    pub subject: String,
    pub text: String,
    pub html: String,
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

pub fn render_message(message: &MailMessage, base: &Url) -> Result<RenderedMail, RenderError> {
    if !matches!(base.scheme(), "http" | "https") || base.host_str().is_none() {
        return Err(RenderError::InvalidBaseUrl);
    }
    let path = match message.template {
        MailTemplate::Verification => "verify-email",
        MailTemplate::Invitation => "accept-invitation",
        MailTemplate::PasswordReset => "reset-password",
    };
    let mut link = base.join(path).map_err(|_| RenderError::InvalidBaseUrl)?;
    link.query_pairs_mut().append_pair("token", message.token());
    let link: String = link.into();
    let (subject, heading, text_intro) = copy(message.locale, message.template);
    let context = message
        .invitation_context
        .as_ref()
        .map_or_else(String::new, |(library, role)| {
            format!("\nLibrary: {library}\nRole: {role}\n")
        });
    let text = format!("{text_intro}{context}\n{link}\n");
    let html_context =
        message
            .invitation_context
            .as_ref()
            .map_or_else(String::new, |(library, role)| {
                format!(
                    "<p>Library: {}<br>Role: {}</p>",
                    escape(library),
                    escape(role)
                )
            });
    let html = format!(
        "<!doctype html><html><body><h1>{heading}</h1><p>{}</p>{html_context}<p><a href=\"{}\">{heading}</a></p></body></html>",
        escape(text_intro.trim()),
        escape(&link)
    );
    Ok(RenderedMail {
        subject: subject.to_owned(),
        text,
        html,
    })
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

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
