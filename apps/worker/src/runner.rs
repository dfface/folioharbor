use std::{
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration as StdDuration,
};

use async_trait::async_trait;
use folioharbor_application::{
    imports::JobFailure,
    ports::{JobRepository, JobRepositoryError, LeaseJobs},
};
use folioharbor_domain::{id::RequestId, imports::job::LeasedJob};
use time::{Duration, OffsetDateTime};
use tokio::sync::Semaphore;
use tracing::Instrument as _;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RunnerConfig {
    concurrency: usize,
}

impl RunnerConfig {
    #[must_use]
    pub const fn new(concurrency: usize) -> Option<Self> {
        if concurrency == 0 {
            None
        } else {
            Some(Self { concurrency })
        }
    }

    #[must_use]
    pub const fn concurrency(self) -> usize {
        self.concurrency
    }
}

impl Default for RunnerConfig {
    fn default() -> Self {
        let available = std::thread::available_parallelism().map_or(1, usize::from);
        Self {
            concurrency: (available / 2).max(1),
        }
    }
}

#[async_trait]
pub trait JobDispatcher: Send + Sync {
    async fn dispatch(&self, job: LeasedJob) -> Result<(), JobFailure>;
}

pub struct WorkerRunner {
    jobs: Arc<dyn JobRepository>,
    dispatcher: Arc<dyn JobDispatcher>,
    owner: String,
    config: RunnerConfig,
    lease_for: Duration,
    empty_polls: AtomicU32,
}

impl WorkerRunner {
    #[must_use]
    pub fn new(
        jobs: Arc<dyn JobRepository>,
        dispatcher: Arc<dyn JobDispatcher>,
        owner: String,
        config: RunnerConfig,
    ) -> Self {
        Self {
            jobs,
            dispatcher,
            owner,
            config,
            lease_for: Duration::minutes(5),
            empty_polls: AtomicU32::new(0),
        }
    }

    /// Leases a bounded batch and runs it with semaphore backpressure.
    ///
    /// # Errors
    /// Returns when queue persistence or lease ownership fails.
    pub async fn run_once(&self) -> Result<usize, JobRepositoryError> {
        let now = OffsetDateTime::now_utc();
        let jobs = self
            .jobs
            .lease(LeaseJobs {
                owner: self.owner.clone(),
                now,
                lease_for: self.lease_for,
                limit: u32::try_from(self.config.concurrency).unwrap_or(u32::MAX),
                request_id: RequestId::new(),
            })
            .await?;
        let count = jobs.len();
        let semaphore = Arc::new(Semaphore::new(self.config.concurrency));
        let mut tasks = Vec::with_capacity(count);
        for job in jobs {
            let permit = Arc::clone(&semaphore)
                .acquire_owned()
                .await
                .map_err(|_| JobRepositoryError)?;
            let jobs = Arc::clone(&self.jobs);
            let dispatcher = Arc::clone(&self.dispatcher);
            let owner = self.owner.clone();
            let lease_for = self.lease_for;
            tasks.push(tokio::spawn(async move {
                let _permit = permit;
                run_leased(jobs, dispatcher, owner, lease_for, job).await
            }));
        }
        let mut first_error = None;
        for task in tasks {
            let result = task
                .await
                .map_err(|_| JobRepositoryError)
                .and_then(|result| result);
            if first_error.is_none() {
                first_error = result.err();
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        if count == 0 {
            let poll = self.empty_polls.fetch_add(1, Ordering::Relaxed).min(6);
            tokio::time::sleep(StdDuration::from_millis(100_u64 << poll)).await;
        } else {
            self.empty_polls.store(0, Ordering::Relaxed);
        }
        Ok(count)
    }
}

async fn run_leased(
    jobs: Arc<dyn JobRepository>,
    dispatcher: Arc<dyn JobDispatcher>,
    owner: String,
    lease_for: Duration,
    job: LeasedJob,
) -> Result<(), JobRepositoryError> {
    let span = tracing::info_span!("job", job_id = %job.job_id.as_uuid(), kind = job.kind.as_str(), attempt = job.attempt);
    let mut operation = Box::pin(dispatcher.dispatch(job.clone()).instrument(span));
    let heartbeat_every =
        StdDuration::from_secs(u64::try_from((lease_for.whole_seconds() / 3).max(1)).unwrap_or(1));
    let mut heartbeat = tokio::time::interval(heartbeat_every);
    heartbeat.tick().await;
    let result = loop {
        tokio::select! {
            result = &mut operation => break result,
            _ = heartbeat.tick() => {
                let now = OffsetDateTime::now_utc();
                if !jobs.heartbeat(job.job_id, &owner, now, lease_for).await? {
                    return Err(JobRepositoryError);
                }
            }
        }
    };
    let now = OffsetDateTime::now_utc();
    let changed = match result {
        Ok(()) => jobs.succeed(job.job_id, &owner, now).await?,
        Err(JobFailure::Transient { code, retry_at }) => {
            jobs.retry(
                job.job_id,
                &owner,
                now,
                retry_at,
                code,
                "transient dependency failure",
            )
            .await?
        }
        Err(JobFailure::Permanent { code, summary }) => {
            jobs.fail(job.job_id, &owner, now, code, &summary).await?
        }
        Err(JobFailure::OperatorRequired { code, summary }) => {
            jobs.operator_required(job.job_id, &owner, now, code, &summary)
                .await?
        }
    };
    if !changed {
        return Err(JobRepositoryError);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    };

    use folioharbor_domain::{
        id::{JobId, LibraryId, UploadId},
        imports::job::{JobInput, JobKind},
    };

    use super::*;

    struct TwoJobRepository {
        leased: Mutex<Option<Vec<LeasedJob>>>,
        failing_job: JobId,
    }

    #[async_trait]
    impl JobRepository for TwoJobRepository {
        async fn ensure_cleanup_jobs(&self, _: OffsetDateTime) -> Result<(), JobRepositoryError> {
            Ok(())
        }

        async fn enqueue(
            &self,
            id: JobId,
            _: Option<LibraryId>,
            _: JobKind,
            _: JobInput,
            _: &str,
            _: OffsetDateTime,
        ) -> Result<JobId, JobRepositoryError> {
            Ok(id)
        }

        async fn lease(&self, _: LeaseJobs) -> Result<Vec<LeasedJob>, JobRepositoryError> {
            Ok(self
                .leased
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
                .unwrap_or_default())
        }

        async fn heartbeat(
            &self,
            _: JobId,
            _: &str,
            _: OffsetDateTime,
            _: Duration,
        ) -> Result<bool, JobRepositoryError> {
            Ok(true)
        }

        async fn succeed(
            &self,
            id: JobId,
            _: &str,
            _: OffsetDateTime,
        ) -> Result<bool, JobRepositoryError> {
            if id == self.failing_job {
                Err(JobRepositoryError)
            } else {
                Ok(true)
            }
        }

        async fn retry(
            &self,
            _: JobId,
            _: &str,
            _: OffsetDateTime,
            _: OffsetDateTime,
            _: &str,
            _: &str,
        ) -> Result<bool, JobRepositoryError> {
            unreachable!("fixture jobs succeed")
        }

        async fn fail(
            &self,
            _: JobId,
            _: &str,
            _: OffsetDateTime,
            _: &str,
            _: &str,
        ) -> Result<bool, JobRepositoryError> {
            unreachable!("fixture jobs succeed")
        }

        async fn operator_required(
            &self,
            _: JobId,
            _: &str,
            _: OffsetDateTime,
            _: &str,
            _: &str,
        ) -> Result<bool, JobRepositoryError> {
            unreachable!("fixture jobs succeed")
        }

        async fn resume_operator_required(
            &self,
            _: JobId,
            _: OffsetDateTime,
        ) -> Result<bool, JobRepositoryError> {
            unreachable!("fixture does not resume jobs")
        }
    }

    struct DelayedDispatcher {
        delayed_job: JobId,
        delayed_finished: Arc<AtomicBool>,
    }

    #[async_trait]
    impl JobDispatcher for DelayedDispatcher {
        async fn dispatch(&self, job: LeasedJob) -> Result<(), JobFailure> {
            if job.job_id == self.delayed_job {
                tokio::time::sleep(StdDuration::from_millis(100)).await;
                self.delayed_finished.store(true, Ordering::SeqCst);
            }
            Ok(())
        }
    }

    fn leased(job_id: JobId) -> LeasedJob {
        LeasedJob {
            job_id,
            library_id: Some(LibraryId::new()),
            kind: JobKind::ImportEpub,
            input: JobInput::upload_v1(UploadId::new().as_uuid().to_string()),
            attempt: 1,
            lease_expires_at: OffsetDateTime::now_utc() + Duration::minutes(5),
        }
    }

    #[tokio::test]
    async fn first_task_error_waits_for_all_spawned_tasks_before_returning() {
        let failing_job = JobId::new();
        let delayed_job = JobId::new();
        let delayed_finished = Arc::new(AtomicBool::new(false));
        let jobs = Arc::new(TwoJobRepository {
            leased: Mutex::new(Some(vec![leased(failing_job), leased(delayed_job)])),
            failing_job,
        });
        let dispatcher = Arc::new(DelayedDispatcher {
            delayed_job,
            delayed_finished: Arc::clone(&delayed_finished),
        });
        let runner = WorkerRunner::new(
            jobs,
            dispatcher,
            "structured-worker".into(),
            RunnerConfig { concurrency: 2 },
        );

        assert!(runner.run_once().await.is_err());
        assert!(
            delayed_finished.load(Ordering::SeqCst),
            "run_once returned while a spawned task could still mutate durable state"
        );
    }
}
