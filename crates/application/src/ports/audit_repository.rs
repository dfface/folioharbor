use async_trait::async_trait;
use thiserror::Error;

use crate::audit::AuditEvent;

#[derive(Debug, Error)]
#[error("audit persistence failed")]
pub struct AuditRepositoryError;

#[async_trait]
pub trait AuditSink: Send + Sync {
    /// Persists a denial outside the rejected mutation's transaction.
    async fn record_denial(&self, event: AuditEvent) -> Result<(), AuditRepositoryError>;
}
