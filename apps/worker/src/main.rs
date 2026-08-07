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
use folioharbor_http::middleware::telemetry::{
    init_observability, record_live_operational_metrics,
};
use folioharbor_postgres::{
    PgCatalogRepository, PgGarbageCollectionRepository, PgImportCleanupRepository,
    PgImportRepository, PgJobRepository, PgMailRepository, connect_worker,
};
use folioharbor_storage_local::LocalBlobStore;
use folioharbor_worker::{
    RunnerConfig, WorkerRunner,
    handlers::{SmtpMailer, WorkerHandlers},
};

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
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    let mut metrics_interval = tokio::time::interval(std::time::Duration::from_secs(15));
    metrics_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            result = async {
                jobs.ensure_cleanup_jobs(time::OffsetDateTime::now_utc()).await?;
                runner.run_once().await?;
                if let Some(mail_delivery) = &mail_delivery {
                    mail_delivery.run_once(time::OffsetDateTime::now_utc(), 25).await?;
                }
                Ok::<(), anyhow::Error>(())
            } => result?,
            _ = metrics_interval.tick() => {
                record_live_operational_metrics(
                    u64::from(metrics_pool.size()),
                    u64::try_from(metrics_pool.num_idle()).unwrap_or(u64::MAX),
                    "worker",
                    metrics_blobs.as_ref(),
                ).await;
            }
            () = &mut shutdown => break,
        }
    }
    Ok(())
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
