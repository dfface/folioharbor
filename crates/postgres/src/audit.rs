use crate::{DatabaseContext, PgTransactionContext};
use async_trait::async_trait;
use folioharbor_application::{
    audit::{AuditDecision, AuditEvent, AuditSource},
    authorization::ResourceRef,
    ports::{AuditRepositoryError, AuditSink},
};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct PgAuditRepository {
    pool: PgPool,
}
impl PgAuditRepository {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}
fn resource_id(resource: ResourceRef) -> Uuid {
    match resource {
        ResourceRef::Library(id) => id.as_uuid(),
        ResourceRef::Membership { user_id, .. } => user_id.as_uuid(),
        ResourceRef::Invitation { invitation_id, .. } => invitation_id.as_uuid(),
    }
}
#[async_trait]
impl AuditSink for PgAuditRepository {
    async fn record_denial(&self, event: AuditEvent) -> Result<(), AuditRepositoryError> {
        if event.decision != AuditDecision::Denied {
            return Err(AuditRepositoryError);
        }
        let context = match (event.source, event.actor) {
            (AuditSource::Api, Some(actor)) => {
                DatabaseContext::api(actor, event.library_id, event.request_id)
            }
            (AuditSource::Worker, _) => {
                DatabaseContext::worker(event.request_id, Some(event.library_id))
            }
            (AuditSource::Api, None) => return Err(AuditRepositoryError),
        };
        let mut tx = self.pool.begin().await.map_err(|_| AuditRepositoryError)?;
        PgTransactionContext::apply(&mut tx, &context)
            .await
            .map_err(|_| AuditRepositoryError)?;
        sqlx::query!(
            "SELECT folioharbor.audit_record_denial($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)",
            Uuid::now_v7(),
            event.actor.map(folioharbor_domain::id::UserId::as_uuid),
            event
                .effective_actor
                .map(folioharbor_domain::id::UserId::as_uuid),
            event.library_id.as_uuid(),
            event.action.as_str(),
            event.resource.resource_type(),
            resource_id(event.resource),
            event.reason_code,
            event.request_id.as_ulid().to_string(),
            event.source.as_str(),
            event.occurred_at,
            event.network_hmac.as_ref().map(<[u8; 32]>::as_slice),
        )
        .execute(&mut *tx)
        .await
        .map_err(|_| AuditRepositoryError)?;
        tx.commit().await.map_err(|_| AuditRepositoryError)
    }
}
