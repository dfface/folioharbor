use std::{
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration as StdDuration,
    time::Instant,
};

use async_trait::async_trait;
use folioharbor_application::{
    imports::JobFailure,
    ports::{JobRepository, JobRepositoryError, LeaseJobs},
};
use folioharbor_domain::{id::RequestId, imports::job::LeasedJob};
use folioharbor_http::middleware::telemetry::{MetricAttributes, TelemetryMetrics};
use opentelemetry::{
    propagation::{Extractor, TextMapPropagator as _},
    trace::TraceContextExt as _,
};
use time::{Duration, OffsetDateTime};
use tokio::sync::Semaphore;
use tracing::Instrument as _;
use tracing_opentelemetry::OpenTelemetrySpanExt as _;

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
        let backlog = self.jobs.backlog(now).await?;
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
        for (state, depth) in [
            ("runnable", backlog.runnable),
            ("scheduled_retry", backlog.scheduled_retry),
        ] {
            if let Ok(attributes) = MetricAttributes::try_new([("state", state)]) {
                TelemetryMetrics.record_queue_depth(depth, &attributes);
            }
        }
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
    let request_id = job.origin_request_id.unwrap_or_else(RequestId::new);
    let span = tracing::info_span!(
        "job",
        job_id = %job.job_id.as_uuid(),
        request_id = %request_id.as_ulid(),
        trace_id = tracing::field::Empty,
        kind = job.kind.as_str(),
        attempt = job.attempt
    );
    if let Some(traceparent) = job.origin_traceparent.as_deref() {
        let parent = opentelemetry_sdk::propagation::TraceContextPropagator::new()
            .extract(&TraceparentExtractor(traceparent));
        span.set_parent(parent);
    }
    let span_context = span.context().span().span_context().clone();
    if span_context.is_valid() {
        span.record("trace_id", span_context.trace_id().to_string());
    }
    tracing::info!(
        parent: &span,
        job_id = %job.job_id.as_uuid(),
        request_id = %request_id.as_ulid(),
        "job leased"
    );
    let started = Instant::now();
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
    let (outcome, is_error, is_retry) = match &result {
        Ok(()) => ("succeeded", false, false),
        Err(JobFailure::Transient { .. }) => ("retry", true, true),
        Err(JobFailure::Permanent { .. }) => ("failed", true, false),
        Err(JobFailure::OperatorRequired { .. }) => ("operator_required", true, false),
    };
    if let Ok(attributes) =
        MetricAttributes::try_new([("job_kind", job.kind.as_str()), ("outcome", outcome)])
    {
        TelemetryMetrics.record_job(started.elapsed().as_secs_f64(), is_error, &attributes);
        if is_retry {
            TelemetryMetrics.record_retry(&attributes);
        }
    }
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
            if job.kind == folioharbor_domain::imports::job::JobKind::ImportEpub {
                jobs.pause_import_operator_required(job.job_id, &owner, now, code, &summary)
                    .await?
            } else {
                jobs.operator_required(job.job_id, &owner, now, code, &summary)
                    .await?
            }
        }
    };
    if !changed {
        return Err(JobRepositoryError);
    }
    Ok(())
}

struct TraceparentExtractor<'a>(&'a str);

impl Extractor for TraceparentExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        key.eq_ignore_ascii_case("traceparent").then_some(self.0)
    }

    fn keys(&self) -> Vec<&str> {
        vec!["traceparent"]
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    use folioharbor_application::ports::JobBacklog;
    use folioharbor_domain::{
        id::{JobId, LibraryId, RequestId, UploadId},
        imports::job::{JobInput, JobKind},
    };
    use folioharbor_http::middleware::telemetry::build_observability_subscriber;
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
    use tracing::instrument::WithSubscriber as _;

    use super::*;
    use crate::runtime::{await_iteration_or_drain, spawn_periodic_reporter};

    static RUNNER_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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

        async fn backlog(&self, _: OffsetDateTime) -> Result<JobBacklog, JobRepositoryError> {
            Ok(JobBacklog {
                runnable: u64::try_from(
                    self.leased
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .as_ref()
                        .map_or(0, Vec::len),
                )
                .unwrap_or(u64::MAX),
                scheduled_retry: 0,
            })
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

        async fn pause_import_operator_required(
            &self,
            _: JobId,
            _: &str,
            _: OffsetDateTime,
            _: &str,
            _: &str,
        ) -> Result<bool, JobRepositoryError> {
            unreachable!("fixture jobs succeed")
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

    struct BlockingDispatcher {
        active: Arc<AtomicUsize>,
        max_active: Arc<AtomicUsize>,
        released: Arc<AtomicBool>,
        changed: Arc<tokio::sync::Notify>,
    }

    #[async_trait]
    impl JobDispatcher for BlockingDispatcher {
        async fn dispatch(&self, _: LeasedJob) -> Result<(), JobFailure> {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_active.fetch_max(active, Ordering::SeqCst);
            self.changed.notify_waiters();
            while !self.released.load(Ordering::SeqCst) {
                self.changed.notified().await;
            }
            self.active.fetch_sub(1, Ordering::SeqCst);
            self.changed.notify_waiters();
            Ok(())
        }
    }

    fn leased(job_id: JobId) -> LeasedJob {
        LeasedJob {
            job_id,
            library_id: Some(LibraryId::new()),
            kind: JobKind::ImportEpub,
            input: JobInput::upload_v1(UploadId::new().as_uuid().to_string()),
            origin_request_id: None,
            origin_traceparent: None,
            attempt: 1,
            lease_expires_at: OffsetDateTime::now_utc() + Duration::minutes(5),
        }
    }

    #[tokio::test]
    async fn first_task_error_waits_for_all_spawned_tasks_before_returning() {
        let _test_guard = RUNNER_TEST_LOCK.lock().await;
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

    #[tokio::test(start_paused = true)]
    #[allow(clippy::expect_used)]
    async fn live_metrics_never_cancel_jobs_and_shutdown_drains_at_the_concurrency_cap() {
        let _test_guard = RUNNER_TEST_LOCK.lock().await;
        let first = JobId::new();
        let second = JobId::new();
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let released = Arc::new(AtomicBool::new(false));
        let changed = Arc::new(tokio::sync::Notify::new());
        let runner = Arc::new(WorkerRunner::new(
            Arc::new(TwoJobRepository {
                leased: Mutex::new(Some(vec![leased(first), leased(second)])),
                failing_job: JobId::new(),
            }),
            Arc::new(BlockingDispatcher {
                active: Arc::clone(&active),
                max_active: Arc::clone(&max_active),
                released: Arc::clone(&released),
                changed: Arc::clone(&changed),
            }),
            "draining-worker".to_owned(),
            RunnerConfig::new(1).expect("one slot"),
        ));
        let reports = Arc::new(AtomicUsize::new(0));
        let reporter = spawn_periodic_reporter(StdDuration::from_secs(15), {
            let reports = Arc::clone(&reports);
            move || {
                reports.fetch_add(1, Ordering::SeqCst);
                std::future::ready(())
            }
        });
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let worker = tokio::spawn(async move {
            await_iteration_or_drain(runner.run_once(), async {
                let _ = shutdown_rx.await;
            })
            .await
        });

        while active.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        for _ in 0..3 {
            tokio::time::advance(StdDuration::from_secs(16)).await;
            tokio::task::yield_now().await;
        }
        assert_eq!(active.load(Ordering::SeqCst), 1);
        assert_eq!(max_active.load(Ordering::SeqCst), 1);
        assert!(reports.load(Ordering::SeqCst) >= 4);

        shutdown_tx.send(()).expect("signal shutdown");
        tokio::task::yield_now().await;
        assert!(
            !worker.is_finished(),
            "shutdown must retain and drain the in-flight batch"
        );

        released.store(true, Ordering::SeqCst);
        changed.notify_waiters();
        let outcome = worker.await.expect("worker join");
        assert!(outcome.shutdown_requested());
        assert_eq!(
            outcome
                .into_inner()
                .expect("iteration started")
                .expect("batch succeeds"),
            2
        );
        assert_eq!(max_active.load(Ordering::SeqCst), 1);
        reporter.abort();
        let _ = reporter.await;
    }

    #[tokio::test]
    async fn leased_job_span_is_a_child_of_the_persisted_api_context()
    -> Result<(), Box<dyn std::error::Error>> {
        let _test_guard = RUNNER_TEST_LOCK.lock().await;
        let exporter = InMemorySpanExporter::default();
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let subscriber =
            build_observability_subscriber(provider.tracer("worker-runner-test"), "warn")?;
        let mut job = leased(JobId::new());
        let origin_request_id = RequestId::new();
        job.origin_request_id = Some(origin_request_id);
        job.origin_traceparent =
            Some("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".to_owned());
        let jobs = Arc::new(TwoJobRepository {
            leased: Mutex::new(None),
            failing_job: JobId::new(),
        });
        let dispatcher = Arc::new(DelayedDispatcher {
            delayed_job: JobId::new(),
            delayed_finished: Arc::new(AtomicBool::new(false)),
        });

        run_leased(
            jobs,
            dispatcher,
            "trace-worker".to_owned(),
            Duration::minutes(5),
            job,
        )
        .with_subscriber(subscriber)
        .await?;
        provider.force_flush()?;
        let spans = exporter.get_finished_spans()?;
        let span = spans
            .iter()
            .find(|span| span.name == "job")
            .ok_or_else(|| std::io::Error::other("job span was not exported"))?;
        assert_eq!(
            span.span_context.trace_id().to_string(),
            "4bf92f3577b34da6a3ce929d0e0e4736"
        );
        assert_eq!(span.parent_span_id.to_string(), "00f067aa0ba902b7");
        assert!(span.attributes.iter().any(|attribute| {
            attribute.key.as_str() == "request_id"
                && attribute.value.to_string() == origin_request_id.as_ulid().to_string()
        }));
        Ok(())
    }
}
