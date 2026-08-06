use async_trait::async_trait;
use folioharbor_domain::time::OffsetDateTime;
use thiserror::Error;
use uuid::Uuid;

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
    pub next_run_at: OffsetDateTime,
    pub expires_at: OffsetDateTime,
}

#[async_trait]
pub trait MailRepository: Send + Sync {
    /// Persists one delivery intent. An existing idempotency key returns its mail id.
    async fn enqueue(&self, entry: NewMailOutboxEntry) -> Result<Uuid, MailRepositoryError>;
}
