use async_trait::async_trait;
use folioharbor_domain::identity::NormalizedEmail;
use thiserror::Error;

#[derive(Debug, Error)]
#[error("mail delivery failed")]
pub struct MailError {
    transient: bool,
    code: &'static str,
}

impl MailError {
    #[must_use]
    pub const fn transient(code: &'static str) -> Self {
        Self {
            transient: true,
            code,
        }
    }

    #[must_use]
    pub const fn permanent(code: &'static str) -> Self {
        Self {
            transient: false,
            code,
        }
    }

    #[must_use]
    pub const fn is_transient(&self) -> bool {
        self.transient
    }

    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.code
    }
}

#[async_trait]
pub trait Mailer: Send + Sync {
    async fn deliver(
        &self,
        recipient: &NormalizedEmail,
        idempotency_key: &str,
        message: &crate::mail::RenderedMail,
    ) -> Result<(), MailError>;
}
