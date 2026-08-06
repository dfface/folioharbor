use async_trait::async_trait;
use folioharbor_domain::time::OffsetDateTime;
use thiserror::Error;
use uuid::Uuid;

#[derive(Clone)]
pub struct LeasedMail {
    pub mail_id: Uuid,
    pub recipient_account_id: Option<Uuid>,
    pub delivery_address: String,
    pub template_code: String,
    pub template_version: u16,
    pub locale: String,
    pub token_ciphertext: Vec<u8>,
    pub encryption_key_id: String,
    pub nonce: Vec<u8>,
    pub idempotency_key: String,
    pub invitation_library_id: Option<Uuid>,
    pub invitation_role: Option<String>,
    pub attempt: u32,
    pub expires_at: OffsetDateTime,
    pub lease_expires_at: OffsetDateTime,
}

pub struct LeaseMail {
    pub owner: String,
    pub now: OffsetDateTime,
    pub lease_for: time::Duration,
    pub limit: u32,
}

#[derive(Debug, Error)]
#[error("mail outbox persistence failed")]
pub struct MailRepositoryError;

pub struct NewMailOutboxEntry {
    pub mail_id: Uuid,
    pub recipient_account_id: Option<Uuid>,
    pub delivery_address: String,
    pub template_code: &'static str,
    pub template_version: u16,
    pub locale: &'static str,
    pub token_ciphertext: Vec<u8>,
    pub encryption_key_id: String,
    pub nonce: Vec<u8>,
    pub idempotency_key: String,
    pub invitation_library_id: Option<Uuid>,
    pub invitation_role: Option<String>,
    pub next_run_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
}

#[async_trait]
pub trait MailRepository: Send + Sync {
    /// Persists one delivery intent. An existing idempotency key returns its mail id.
    async fn enqueue(&self, entry: NewMailOutboxEntry) -> Result<Uuid, MailRepositoryError>;

    async fn lease(&self, _request: LeaseMail) -> Result<Vec<LeasedMail>, MailRepositoryError> {
        Err(MailRepositoryError)
    }

    async fn mark_sent(
        &self,
        _mail_id: Uuid,
        _owner: &str,
        _now: OffsetDateTime,
    ) -> Result<bool, MailRepositoryError> {
        Err(MailRepositoryError)
    }

    async fn retry(
        &self,
        _mail_id: Uuid,
        _owner: &str,
        _now: OffsetDateTime,
        _next_run_at: OffsetDateTime,
        _code: &str,
    ) -> Result<bool, MailRepositoryError> {
        Err(MailRepositoryError)
    }

    async fn mark_failed(
        &self,
        _mail_id: Uuid,
        _owner: &str,
        _now: OffsetDateTime,
        _code: &str,
    ) -> Result<bool, MailRepositoryError> {
        Err(MailRepositoryError)
    }
}
