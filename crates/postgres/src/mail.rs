use async_trait::async_trait;
use folioharbor_application::ports::{MailRepository, MailRepositoryError, NewMailOutboxEntry};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct PgMailRepository {
    pool: PgPool,
}

impl PgMailRepository {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl MailRepository for PgMailRepository {
    async fn enqueue(&self, entry: NewMailOutboxEntry) -> Result<Uuid, MailRepositoryError> {
        sqlx::query_scalar(
            "INSERT INTO folioharbor.mail_outbox (mail_id,recipient_account_id,delivery_address,template_code,template_version,locale,token_ciphertext,encryption_key_id,token_nonce,idempotency_key,state,attempt_count,next_run_at,expires_at,created_at,updated_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,'pending',0,$11,$12,clock_timestamp(),clock_timestamp()) ON CONFLICT (idempotency_key) DO UPDATE SET idempotency_key=EXCLUDED.idempotency_key RETURNING mail_id",
        )
        .bind(entry.mail_id)
        .bind(entry.recipient_account_id)
        .bind(entry.delivery_address)
        .bind(entry.template_code)
        .bind(i32::from(entry.template_version))
        .bind(entry.locale)
        .bind(entry.token_ciphertext)
        .bind(entry.encryption_key_id)
        .bind(entry.nonce)
        .bind(entry.idempotency_key)
        .bind(entry.next_run_at)
        .bind(entry.expires_at)
        .fetch_one(&self.pool)
        .await
        .map_err(|_| MailRepositoryError)
    }
}
