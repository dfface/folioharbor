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
    /// Ensures the closed set of singleton cleanup jobs exists.
    ///
    /// # Errors
    /// Returns a repository error when durable persistence is unavailable.
    pub async fn ensure_cleanup_jobs(&self, now: OffsetDateTime) -> Result<(), JobRepositoryError> {
        self.repository.ensure_cleanup_jobs(now).await
    }
    /// Enqueues idempotently.
    ///
    /// # Errors
    /// Returns a repository error when durable persistence is unavailable.
    pub async fn enqueue(
        &self,
        id: JobId,
        library: Option<LibraryId>,
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
    /// Pauses an active lease until an operator resolves the durable cause.
    ///
    /// # Errors
    /// Returns a repository error when durable persistence is unavailable.
    pub async fn operator_required(
        &self,
        id: JobId,
        owner: &str,
        now: OffsetDateTime,
        code: &str,
        summary: &str,
    ) -> Result<bool, JobRepositoryError> {
        self.repository
            .operator_required(id, owner, now, code, summary)
            .await
    }
    /// Atomically pauses a leased import together with its retained upload.
    ///
    /// # Errors
    /// Returns a repository error when durable persistence is unavailable.
    pub async fn pause_import_operator_required(
        &self,
        id: JobId,
        owner: &str,
        now: OffsetDateTime,
        code: &str,
        summary: &str,
    ) -> Result<bool, JobRepositoryError> {
        self.repository
            .pause_import_operator_required(id, owner, now, code, summary)
            .await
    }
    /// Atomically resumes an operator-paused import and its retained upload.
    ///
    /// # Errors
    /// Returns a repository error when durable persistence is unavailable.
    pub async fn resume_operator_required(
        &self,
        id: JobId,
        now: OffsetDateTime,
    ) -> Result<bool, JobRepositoryError> {
        self.repository.resume_operator_required(id, now).await
    }
}
