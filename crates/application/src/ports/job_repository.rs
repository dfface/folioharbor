use async_trait::async_trait;
use folioharbor_domain::{
    id::{JobId, LibraryId, RequestId},
    imports::job::{JobInput, JobKind, LeasedJob},
    time::OffsetDateTime,
};
use thiserror::Error;
use time::Duration;

pub struct LeaseJobs {
    pub owner: String,
    pub now: OffsetDateTime,
    pub lease_for: Duration,
    pub limit: u32,
    pub request_id: RequestId,
}

#[derive(Debug, Error)]
#[error("job persistence failed")]
pub struct JobRepositoryError;

#[async_trait]
pub trait JobRepository: Send + Sync {
    async fn ensure_cleanup_jobs(&self, now: OffsetDateTime) -> Result<(), JobRepositoryError>;
    async fn enqueue(
        &self,
        id: JobId,
        library: Option<LibraryId>,
        kind: JobKind,
        input: JobInput,
        idempotency_key: &str,
        run_at: OffsetDateTime,
    ) -> Result<JobId, JobRepositoryError>;
    async fn lease(&self, request: LeaseJobs) -> Result<Vec<LeasedJob>, JobRepositoryError>;
    async fn heartbeat(
        &self,
        id: JobId,
        owner: &str,
        now: OffsetDateTime,
        lease_for: Duration,
    ) -> Result<bool, JobRepositoryError>;
    async fn succeed(
        &self,
        id: JobId,
        owner: &str,
        now: OffsetDateTime,
    ) -> Result<bool, JobRepositoryError>;
    async fn retry(
        &self,
        id: JobId,
        owner: &str,
        now: OffsetDateTime,
        next_run: OffsetDateTime,
        code: &str,
        summary: &str,
    ) -> Result<bool, JobRepositoryError>;
    async fn fail(
        &self,
        id: JobId,
        owner: &str,
        now: OffsetDateTime,
        code: &str,
        summary: &str,
    ) -> Result<bool, JobRepositoryError>;
    async fn operator_required(
        &self,
        id: JobId,
        owner: &str,
        now: OffsetDateTime,
        code: &str,
        summary: &str,
    ) -> Result<bool, JobRepositoryError>;
}
