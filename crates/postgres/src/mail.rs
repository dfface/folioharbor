use async_trait::async_trait;
use folioharbor_application::ports::{
    LeaseMail, LeasedMail, MailRepository, MailRepositoryError, NewMailOutboxEntry,
};
use folioharbor_domain::id::RequestId;
use sqlx::{PgConnection, PgPool};
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
        let mail_id = entry.mail_id;
        let mut transaction = self.pool.begin().await.map_err(|_| MailRepositoryError)?;
        insert_mail(&mut transaction, entry)
            .await
            .map_err(|_| MailRepositoryError)?;
        transaction
            .commit()
            .await
            .map_err(|_| MailRepositoryError)?;
        Ok(mail_id)
    }

    async fn lease(&self, request: LeaseMail) -> Result<Vec<LeasedMail>, MailRepositoryError> {
        let mut transaction = self.pool.begin().await.map_err(|_| MailRepositoryError)?;
        crate::PgTransactionContext::apply(
            &mut transaction,
            &crate::DatabaseContext::worker(RequestId::new(), None),
        )
        .await
        .map_err(|_| MailRepositoryError)?;
        sqlx::query(
            "UPDATE folioharbor.mail_outbox SET state='expired',token_ciphertext=''::bytea,lease_owner=NULL,lease_expires_at=NULL,last_error_code='intent_expired',updated_at=$1 WHERE state IN ('pending','retry_wait','leased') AND expires_at <= $1",
        )
        .bind(request.now)
        .execute(&mut *transaction)
        .await
        .map_err(|_| MailRepositoryError)?;
        let rows: Vec<(
            Uuid,
            Option<Uuid>,
            String,
            String,
            i32,
            String,
            Vec<u8>,
            String,
            Vec<u8>,
            String,
            Option<Uuid>,
            Option<String>,
            i32,
            time::OffsetDateTime,
            time::OffsetDateTime,
        )> = sqlx::query_as(
            "WITH candidates AS (SELECT mail_id FROM folioharbor.mail_outbox WHERE expires_at > $1 AND next_run_at <= $1 AND (state IN ('pending','retry_wait') OR (state='leased' AND lease_expires_at <= $1)) ORDER BY next_run_at,created_at FOR UPDATE SKIP LOCKED LIMIT $2) UPDATE folioharbor.mail_outbox m SET state='leased',lease_owner=$3,lease_expires_at=$4,attempt_count=m.attempt_count+1,last_error_code=NULL,updated_at=$1 FROM candidates c WHERE m.mail_id=c.mail_id RETURNING m.mail_id,m.recipient_account_id,m.delivery_address,m.template_code,m.template_version,m.locale,m.token_ciphertext,m.encryption_key_id,m.token_nonce,m.idempotency_key,m.invitation_library_id,m.invitation_role,m.attempt_count,m.expires_at,m.lease_expires_at",
        )
        .bind(request.now)
        .bind(i64::from(request.limit))
        .bind(&request.owner)
        .bind(request.now + request.lease_for)
        .fetch_all(&mut *transaction)
        .await
        .map_err(|_| MailRepositoryError)?;
        transaction
            .commit()
            .await
            .map_err(|_| MailRepositoryError)?;
        rows.into_iter()
            .map(
                |(
                    mail_id,
                    recipient_account_id,
                    delivery_address,
                    template_code,
                    template_version,
                    locale,
                    token_ciphertext,
                    encryption_key_id,
                    nonce,
                    idempotency_key,
                    invitation_library_id,
                    invitation_role,
                    attempt,
                    expires_at,
                    lease_expires_at,
                )| {
                    Ok(LeasedMail {
                        mail_id,
                        recipient_account_id,
                        delivery_address,
                        template_code,
                        template_version: u16::try_from(template_version)
                            .map_err(|_| MailRepositoryError)?,
                        locale,
                        token_ciphertext,
                        encryption_key_id,
                        nonce,
                        idempotency_key,
                        invitation_library_id,
                        invitation_role,
                        attempt: u32::try_from(attempt).map_err(|_| MailRepositoryError)?,
                        expires_at,
                        lease_expires_at,
                    })
                },
            )
            .collect()
    }

    async fn mark_sent(
        &self,
        mail_id: Uuid,
        owner: &str,
        now: time::OffsetDateTime,
    ) -> Result<bool, MailRepositoryError> {
        terminal_transition(&self.pool, mail_id, owner, now, "sent", None).await
    }

    async fn retry(
        &self,
        mail_id: Uuid,
        owner: &str,
        now: time::OffsetDateTime,
        next_run_at: time::OffsetDateTime,
        code: &str,
    ) -> Result<bool, MailRepositoryError> {
        let mut transaction = self.pool.begin().await.map_err(|_| MailRepositoryError)?;
        crate::PgTransactionContext::apply(
            &mut transaction,
            &crate::DatabaseContext::worker(RequestId::new(), None),
        )
        .await
        .map_err(|_| MailRepositoryError)?;
        let changed = sqlx::query(
            "UPDATE folioharbor.mail_outbox SET state='retry_wait',lease_owner=NULL,lease_expires_at=NULL,next_run_at=$4,last_error_code=$5,updated_at=$3 WHERE mail_id=$1 AND state='leased' AND lease_owner=$2 AND lease_expires_at>$3",
        )
        .bind(mail_id)
        .bind(owner)
        .bind(now)
        .bind(next_run_at)
        .bind(code)
        .execute(&mut *transaction)
        .await
        .map_err(|_| MailRepositoryError)?
        .rows_affected()
            == 1;
        transaction
            .commit()
            .await
            .map_err(|_| MailRepositoryError)?;
        Ok(changed)
    }

    async fn mark_failed(
        &self,
        mail_id: Uuid,
        owner: &str,
        now: time::OffsetDateTime,
        code: &str,
    ) -> Result<bool, MailRepositoryError> {
        terminal_transition(&self.pool, mail_id, owner, now, "failed", Some(code)).await
    }
}

async fn terminal_transition(
    pool: &PgPool,
    mail_id: Uuid,
    owner: &str,
    now: time::OffsetDateTime,
    state: &str,
    code: Option<&str>,
) -> Result<bool, MailRepositoryError> {
    let mut transaction = pool.begin().await.map_err(|_| MailRepositoryError)?;
    crate::PgTransactionContext::apply(
        &mut transaction,
        &crate::DatabaseContext::worker(RequestId::new(), None),
    )
    .await
    .map_err(|_| MailRepositoryError)?;
    let changed = sqlx::query(
        "UPDATE folioharbor.mail_outbox SET state=$4,token_ciphertext=''::bytea,lease_owner=NULL,lease_expires_at=NULL,last_error_code=$5,delivered_at=CASE WHEN $4='sent' THEN $3 ELSE NULL END,updated_at=$3 WHERE mail_id=$1 AND state='leased' AND lease_owner=$2 AND lease_expires_at>$3",
    )
    .bind(mail_id)
    .bind(owner)
    .bind(now)
    .bind(state)
    .bind(code)
    .execute(&mut *transaction)
    .await
    .map_err(|_| MailRepositoryError)?
    .rows_affected()
        == 1;
    transaction
        .commit()
        .await
        .map_err(|_| MailRepositoryError)?;
    Ok(changed)
}

pub(crate) async fn insert_mail(
    connection: &mut PgConnection,
    entry: NewMailOutboxEntry,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO folioharbor.mail_outbox (mail_id,recipient_account_id,delivery_address,template_code,template_version,locale,token_ciphertext,encryption_key_id,token_nonce,idempotency_key,invitation_library_id,invitation_role,state,attempt_count,next_run_at,expires_at,created_at,updated_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,'pending',0,$13,$14,clock_timestamp(),clock_timestamp()) ON CONFLICT (idempotency_key) DO NOTHING",
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
    .bind(entry.invitation_library_id)
    .bind(entry.invitation_role)
    .bind(entry.next_run_at)
    .bind(entry.expires_at)
    .execute(connection)
    .await?;
    Ok(())
}
