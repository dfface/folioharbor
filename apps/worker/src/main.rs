#![forbid(unsafe_code)]

use std::{collections::BTreeMap, sync::Arc};

use folioharbor_application::ports::JobRepository;
use folioharbor_application::{
    catalog::garbage_collect::CollectGarbage,
    config::{ConfigSources, Settings},
    imports::{CleanupImports, ProcessImportJob, RetrySchedule},
    mail::DeliverMailJob,
};
use folioharbor_epub::{EpubPublicationParser, ParserLimits};
use folioharbor_http::middleware::telemetry::{OperationalMetrics, init_observability};
use folioharbor_postgres::{
    PgCatalogRepository, PgGarbageCollectionRepository, PgImportCleanupRepository,
    PgImportRepository, PgJobRepository, PgMailRepository, connect_worker,
};
use folioharbor_storage_local::LocalBlobStore;
use folioharbor_worker::{
    RunnerConfig, WorkerRunner,
    handlers::{SmtpMailer, WorkerHandlers},
    runtime::{await_iteration_or_drain, spawn_periodic_reporter},
};

fn spawn_metrics_reporter(
    pool: sqlx::PgPool,
    blobs: Arc<dyn folioharbor_application::ports::BlobStore>,
    metrics: OperationalMetrics,
) -> tokio::task::JoinHandle<()> {
    spawn_periodic_reporter(std::time::Duration::from_secs(15), move || {
        let pool = pool.clone();
        let blobs = blobs.clone();
        let metrics = metrics.clone();
        async move {
            metrics
                .record(
                    u64::from(pool.size()),
                    u64::try_from(pool.num_idle()).unwrap_or(u64::MAX),
                    blobs.as_ref(),
                )
                .await;
        }
    })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let settings = Settings::load(ConfigSources {
        environment: std::env::vars().collect::<BTreeMap<_, _>>(),
        ..ConfigSources::default()
    })?;
    let _telemetry = init_observability("folioharbor-worker", &settings.observability)?;
    let database_url = settings
        .database
        .url
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("FOLIOHARBOR_DATABASE_URL is required"))?;
    let storage_root = settings.storage.root.clone();
    let config = RunnerConfig::new(settings.worker.concurrency)
        .ok_or_else(|| anyhow::anyhow!("worker concurrency must be positive"))?;
    let pool = connect_worker(database_url).await?;
    let smtp = SmtpMailer::for_mode(&settings.mail)?;
    let blobs = Arc::new(LocalBlobStore::new(storage_root));
    let metrics_blobs = blobs.clone();
    let metrics_pool = pool.clone();
    let process = Arc::new(ProcessImportJob::new(
        Arc::new(PgImportRepository::new(pool.clone())),
        Arc::new(EpubPublicationParser::new(
            blobs.clone(),
            ParserLimits::default(),
        )),
        Arc::new(PgCatalogRepository::new(pool.clone())),
        RetrySchedule::default(),
    ));
    let worker_id = format!("worker-{}", std::process::id());
    let jobs = Arc::new(PgJobRepository::new(pool.clone()));
    let cleanup = Arc::new(CleanupImports::new(
        Arc::new(PgImportCleanupRepository::new(pool.clone())),
        blobs.clone(),
    ));
    let garbage = Arc::new(
        CollectGarbage::new(
            Arc::new(PgGarbageCollectionRepository::new(pool.clone())),
            blobs,
            worker_id.clone(),
            100,
        )
        .ok_or_else(|| anyhow::anyhow!("garbage collection configuration is invalid"))?,
    );
    let mail_repository = PgMailRepository::new(pool);
    let application_secrets = Arc::new(settings.auth.application_secrets);
    let public_base_url = settings.server.public_base_url;
    let mail_delivery = smtp.as_ref().map(|smtp| {
        DeliverMailJob::new(
            &mail_repository,
            smtp,
            application_secrets,
            public_base_url,
            worker_id.clone(),
        )
    });
    let runner = WorkerRunner::new(
        jobs.clone(),
        Arc::new(WorkerHandlers::with_cleanup_and_garbage(
            process, cleanup, garbage,
        )),
        worker_id.clone(),
        config,
    );
    let metrics_reporter = spawn_metrics_reporter(
        metrics_pool,
        metrics_blobs,
        OperationalMetrics::new("worker"),
    );
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    let result = loop {
        let outcome = await_iteration_or_drain(
            async {
                jobs.ensure_cleanup_jobs(time::OffsetDateTime::now_utc())
                    .await?;
                runner.run_once().await?;
                if let Some(mail_delivery) = &mail_delivery {
                    mail_delivery
                        .run_once(time::OffsetDateTime::now_utc(), 25)
                        .await?;
                }
                Ok::<(), anyhow::Error>(())
            },
            shutdown.as_mut(),
        )
        .await;
        let shutdown_requested = outcome.shutdown_requested();
        let Some(iteration) = outcome.into_inner() else {
            break Ok(());
        };
        if let Err(error) = iteration {
            break Err(error);
        }
        if shutdown_requested {
            break Ok(());
        }
    };
    // Compose's stop_grace_period is the outer hard bound; until then the
    // worker drains its retained batch before stopping this reporter.
    metrics_reporter.abort();
    let _ = metrics_reporter.await;
    result
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            let _ = signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}
