use folioharbor_domain::{
    id::{JobId, LibraryId},
    imports::job::{JobInput, JobKind, LeasedJob},
    time::OffsetDateTime,
};
use time::Duration;

use crate::ports::{JobRepository, JobRepositoryError, LeaseJobs};

pub struct JobQueue<R> {
    repository: R,
}
impl<R> JobQueue<R> {
    #[must_use]
    pub const fn new(repository: R) -> Self {
        Self { repository }
    }
    #[must_use]
    pub const fn repository(&self) -> &R {
        &self.repository
    }
}
impl<R: JobRepository> JobQueue<R> {
    /// Enqueues idempotently.
    ///
    /// # Errors
    /// Returns a repository error when durable persistence is unavailable.
    pub async fn enqueue(
        &self,
        id: JobId,
        library: LibraryId,
        kind: JobKind,
        input: JobInput,
        key: &str,
        run_at: OffsetDateTime,
    ) -> Result<JobId, JobRepositoryError> {
        self.repository
            .enqueue(id, library, kind, input, key, run_at)
            .await
    }
    /// Leases runnable jobs without waiting on jobs leased concurrently.
    ///
    /// # Errors
    /// Returns a repository error when durable persistence is unavailable.
    pub async fn lease(&self, request: LeaseJobs) -> Result<Vec<LeasedJob>, JobRepositoryError> {
        self.repository.lease(request).await
    }
    /// Extends an active owned lease.
    ///
    /// # Errors
    /// Returns a repository error when durable persistence is unavailable.
    pub async fn heartbeat(
        &self,
        id: JobId,
        owner: &str,
        now: OffsetDateTime,
        lease_for: Duration,
    ) -> Result<bool, JobRepositoryError> {
        self.repository.heartbeat(id, owner, now, lease_for).await
    }
    /// Marks an active owned lease successful.
    ///
    /// # Errors
    /// Returns a repository error when durable persistence is unavailable.
    pub async fn succeed(
        &self,
        id: JobId,
        owner: &str,
        now: OffsetDateTime,
    ) -> Result<bool, JobRepositoryError> {
        self.repository.succeed(id, owner, now).await
    }
    /// Schedules an active owned lease for retry.
    ///
    /// # Errors
    /// Returns a repository error when durable persistence is unavailable.
    pub async fn retry(
        &self,
        id: JobId,
        owner: &str,
        now: OffsetDateTime,
        next_run: OffsetDateTime,
        code: &str,
        summary: &str,
    ) -> Result<bool, JobRepositoryError> {
        self.repository
            .retry(id, owner, now, next_run, code, summary)
            .await
    }
    /// Marks an active owned lease permanently failed.
    ///
    /// # Errors
    /// Returns a repository error when durable persistence is unavailable.
    pub async fn fail(
        &self,
        id: JobId,
        owner: &str,
        now: OffsetDateTime,
        code: &str,
        summary: &str,
    ) -> Result<bool, JobRepositoryError> {
        self.repository.fail(id, owner, now, code, summary).await
    }
}
