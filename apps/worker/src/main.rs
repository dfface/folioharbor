#![forbid(unsafe_code)]

use std::{collections::BTreeMap, sync::Arc};

use folioharbor_application::ports::JobRepository;
use folioharbor_application::{
    config::{ConfigSources, Settings},
    imports::{CleanupImports, ProcessImportJob, RetrySchedule},
    mail::DeliverMailJob,
};
use folioharbor_epub::{EpubPublicationParser, ParserLimits};
use folioharbor_postgres::{
    PgCatalogRepository, PgImportCleanupRepository, PgImportRepository, PgJobRepository,
    PgMailRepository, connect_worker,
};
use folioharbor_storage_local::LocalBlobStore;
use folioharbor_worker::{
    RunnerConfig, WorkerRunner,
    handlers::{SmtpMailer, WorkerHandlers},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let settings = Settings::load(ConfigSources {
        environment: std::env::vars().collect::<BTreeMap<_, _>>(),
        ..ConfigSources::default()
    })?;
    let database_url = settings
        .database
        .url
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("FOLIOHARBOR_DATABASE_URL is required"))?;
    let storage_root = settings.storage.root.clone();
    let config = RunnerConfig::new(settings.worker.concurrency)
        .ok_or_else(|| anyhow::anyhow!("worker concurrency must be positive"))?;
    let pool = connect_worker(&database_url).await?;
    let smtp = SmtpMailer::new(&settings.mail)?;
    let blobs = Arc::new(LocalBlobStore::new(storage_root));
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
        blobs,
    ));
    let mail_repository = PgMailRepository::new(pool);
    let mail_delivery = DeliverMailJob::new(
        &mail_repository,
        &smtp,
        Arc::new(settings.auth.application_secrets),
        settings.server.public_base_url,
        worker_id.clone(),
    );
    let runner = WorkerRunner::new(
        jobs.clone(),
        Arc::new(WorkerHandlers::with_cleanup(process, cleanup)),
        worker_id.clone(),
        config,
    );
    loop {
        jobs.ensure_cleanup_jobs(time::OffsetDateTime::now_utc())
            .await?;
        runner.run_once().await?;
        mail_delivery
            .run_once(time::OffsetDateTime::now_utc(), 25)
            .await?;
    }
}
