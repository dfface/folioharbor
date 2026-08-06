#![forbid(unsafe_code)]

use std::{env, path::PathBuf, sync::Arc};

use folioharbor_application::imports::{
    CleanupCursor, CleanupImports, ProcessImportJob, RetrySchedule,
};
use folioharbor_epub::{EpubPublicationParser, ParserLimits};
use folioharbor_postgres::{
    PgCatalogRepository, PgImportCleanupRepository, PgImportRepository, PgJobRepository,
    connect_worker,
};
use folioharbor_storage_local::LocalBlobStore;
use folioharbor_worker::{RunnerConfig, WorkerRunner, handlers::WorkerHandlers};
use secrecy::SecretString;
use time::OffsetDateTime;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let database_url = SecretString::from(env::var("FOLIOHARBOR_DATABASE_URL")?.into_boxed_str());
    let storage_root = PathBuf::from(env::var("FOLIOHARBOR_STORAGE_ROOT")?);
    let configured = env::var("FOLIOHARBOR_WORKER_CONCURRENCY")
        .ok()
        .map(|value| value.parse::<usize>())
        .transpose()?;
    let config = match configured {
        Some(value) => RunnerConfig::new(value)
            .ok_or_else(|| anyhow::anyhow!("worker concurrency must be positive"))?,
        None => RunnerConfig::default(),
    };
    let pool = connect_worker(&database_url).await?;
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
    let runner = WorkerRunner::new(
        Arc::new(PgJobRepository::new(pool.clone())),
        Arc::new(WorkerHandlers::new(process)),
        worker_id.clone(),
        config,
    );
    let cleanup = CleanupImports::new(Arc::new(PgImportCleanupRepository::new(pool)), blobs);
    loop {
        let boundary = OffsetDateTime::now_utc();
        let cursor = CleanupCursor::new(boundary, 100)
            .ok_or_else(|| anyhow::anyhow!("cleanup batch is invalid"))?;
        cleanup.run(&worker_id, cursor).await?;
        runner.run_once().await?;
    }
}
