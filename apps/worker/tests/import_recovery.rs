#![allow(clippy::expect_used, clippy::too_many_lines)]

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use folioharbor_application::{
    imports::{JobFailure, ProcessImportJob, RetrySchedule},
    ports::{BlobStore, JobRepository, LeaseJobs},
};
use folioharbor_domain::{
    id::{JobId, LibraryId, RequestId, UploadId, UserId},
    imports::{
        blob::{BlobIdentity, ByteCount, DedupScope, Sha256Digest, StorageKey, StorageNamespace},
        job::{JobInput, JobKind},
    },
};
use folioharbor_epub::{EpubPublicationParser, ParserLimits};
use folioharbor_postgres::{
    PgCatalogRepository, PgImportRepository, PgJobRepository, PgPools, run_migrations,
};
use folioharbor_storage_local::LocalBlobStore;
use folioharbor_test_support::postgres::TestPostgres;
use folioharbor_worker::{JobDispatcher, RunnerConfig, WorkerRunner, handlers::WorkerHandlers};
use sha2::{Digest as _, Sha256};
use time::{Duration, OffsetDateTime};

#[path = "../../../tests/fixtures/epub/generate-fixtures.rs"]
mod fixtures;

#[test]
fn concurrency_one_is_valid_and_default_preserves_backpressure() {
    assert_eq!(
        RunnerConfig::new(1).expect("one worker slot").concurrency(),
        1
    );
    assert!(RunnerConfig::default().concurrency() >= 1);
    assert!(RunnerConfig::new(0).is_none());
}

struct FailureDispatcher(Mutex<HashMap<String, Option<JobFailure>>>);

#[async_trait]
impl JobDispatcher for FailureDispatcher {
    async fn dispatch(
        &self,
        job: folioharbor_domain::imports::job::LeasedJob,
    ) -> Result<(), JobFailure> {
        self.0
            .lock()
            .expect("failure fixture")
            .get_mut(&job.input.upload_id)
            .and_then(Option::take)
            .map_or(Ok(()), Err)
    }
}

#[tokio::test]
async fn runner_maps_all_closed_failures_to_durable_queue_states_and_retries_transient_once()
-> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_migrations(&pools.owner).await?;
    let now = OffsetDateTime::now_utc();
    let actor = UserId::new();
    let library = LibraryId::new();
    sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,'failures@test.invalid','failures@test.invalid','verified',$2,$2)").bind(actor.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,created_at,updated_at) VALUES($1,'Failures',$2,$2)").bind(library.as_uuid()).bind(now).execute(&pools.owner).await?;
    let uploads = [UploadId::new(), UploadId::new(), UploadId::new()];
    let ids = [JobId::new(), JobId::new(), JobId::new()];
    let jobs = Arc::new(PgJobRepository::new(pools.worker.clone()));
    for (index, upload) in uploads.into_iter().enumerate() {
        jobs.enqueue(
            ids[index],
            library,
            JobKind::ImportEpub,
            JobInput::upload_v1(upload.as_uuid().to_string()),
            &format!("failure:{index}"),
            now,
        )
        .await?;
    }
    let dispatcher = Arc::new(FailureDispatcher(Mutex::new(HashMap::from([
        (
            uploads[0].as_uuid().to_string(),
            Some(JobFailure::Permanent {
                code: "invalid_epub",
                summary: "publication is malformed".into(),
            }),
        ),
        (
            uploads[1].as_uuid().to_string(),
            Some(JobFailure::Transient {
                code: "blob_io_unavailable",
                retry_at: now + Duration::minutes(5),
            }),
        ),
        (
            uploads[2].as_uuid().to_string(),
            Some(JobFailure::OperatorRequired {
                code: "schema_incompatible",
                summary: "operator action required".into(),
            }),
        ),
    ]))));
    let runner = WorkerRunner::new(
        jobs.clone(),
        dispatcher.clone(),
        "failure-worker".into(),
        RunnerConfig::new(3).expect("three slots"),
    );
    assert_eq!(runner.run_once().await?, 3);
    let first_states: Vec<(uuid::Uuid, String, Option<String>)> = sqlx::query_as("SELECT job_id,state,error_code FROM folioharbor.background_jobs WHERE job_id=ANY($1) ORDER BY job_id")
        .bind(ids.map(folioharbor_domain::id::JobId::as_uuid).to_vec()).fetch_all(&pools.owner).await?;
    assert!(first_states.contains(&(
        ids[0].as_uuid(),
        "failed".into(),
        Some("invalid_epub".into())
    )));
    assert!(first_states.contains(&(
        ids[1].as_uuid(),
        "retry_wait".into(),
        Some("blob_io_unavailable".into())
    )));
    assert!(first_states.contains(&(
        ids[2].as_uuid(),
        "failed".into(),
        Some("schema_incompatible".into())
    )));
    sqlx::query("UPDATE folioharbor.background_jobs SET next_run_at=$2 WHERE job_id=$1")
        .bind(ids[1].as_uuid())
        .bind(OffsetDateTime::now_utc())
        .execute(&pools.owner)
        .await?;
    let restarted = WorkerRunner::new(
        jobs,
        dispatcher,
        "failure-worker-restarted".into(),
        RunnerConfig::new(1).expect("one slot"),
    );
    assert_eq!(restarted.run_once().await?, 1);
    let transient_state: String =
        sqlx::query_scalar("SELECT state FROM folioharbor.background_jobs WHERE job_id=$1")
            .bind(ids[1].as_uuid())
            .fetch_one(&pools.owner)
            .await?;
    assert_eq!(transient_state, "succeeded");
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn restart_after_catalog_commit_reconciles_once_and_finishes_the_leased_job()
-> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_migrations(&pools.owner).await?;
    let now = OffsetDateTime::now_utc();
    let actor = UserId::new();
    let library = LibraryId::new();
    let upload = UploadId::new();
    let payload = fixtures::valid_epub()?;
    let payload_bytes = i64::try_from(payload.len())?;
    let digest = Sha256Digest::from_bytes(Sha256::digest(&payload).into());
    let identity = BlobIdentity::new(
        StorageNamespace::for_scope(DedupScope::Instance, library, upload),
        digest,
        ByteCount::new(payload.len() as u64),
    );
    let directory = tempfile::tempdir()?;
    let blobs: Arc<dyn BlobStore> = Arc::new(LocalBlobStore::new(directory.path()));
    let staging = StorageKey::from_opaque(format!("staging:{}", "ab".repeat(32)));
    blobs.create_staging_for(&staging).await?;
    blobs.append(&staging, &payload).await?;
    let stored = blobs.promote(&staging, &identity).await?;
    sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,'worker@test.invalid','worker@test.invalid','verified',$2,$2)")
        .bind(actor.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,quota_reserved_bytes,created_at,updated_at) VALUES($1,'Worker',$2,$3,$3)")
        .bind(library.as_uuid()).bind(payload_bytes).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.upload_sessions(upload_id,library_id,created_by,file_name,media_type,declared_bytes,received_bytes,state,dedup_scope,storage_key,sha256,expires_at,created_at,updated_at) VALUES($1,$2,$3,'book.epub','application/epub+zip',$4,$4,'queued','instance',$5,$6,$7,$8,$8)")
        .bind(upload.as_uuid()).bind(library.as_uuid()).bind(actor.as_uuid()).bind(payload_bytes)
        .bind(stored.key.as_str()).bind(digest.as_bytes().to_vec()).bind(now+Duration::hours(1)).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.quota_reservations(upload_id,library_id,reserved_bytes,expires_at,state) VALUES($1,$2,$3,$4,'active')")
        .bind(upload.as_uuid()).bind(library.as_uuid()).bind(payload_bytes).bind(now+Duration::hours(1)).execute(&pools.owner).await?;
    let jobs = Arc::new(PgJobRepository::new(pools.worker.clone()));
    let job_id = JobId::new();
    jobs.enqueue(
        job_id,
        library,
        JobKind::ImportEpub,
        JobInput::upload_v1(upload.as_uuid().to_string()),
        &format!("import:{}", upload.as_uuid()),
        now,
    )
    .await?;
    let leased = jobs
        .lease(LeaseJobs {
            owner: "crashed-worker".into(),
            now,
            lease_for: Duration::seconds(1),
            limit: 1,
            request_id: RequestId::new(),
        })
        .await?;
    let process = Arc::new(ProcessImportJob::new(
        Arc::new(PgImportRepository::new(pools.worker.clone())),
        Arc::new(EpubPublicationParser::new(
            blobs.clone(),
            ParserLimits::default(),
        )),
        Arc::new(PgCatalogRepository::new(pools.worker.clone())),
        RetrySchedule::default(),
    ));
    let handlers = Arc::new(WorkerHandlers::new(process));

    handlers
        .dispatch(leased[0].clone())
        .await
        .expect("catalog commits before injected crash");
    let before_restart: (String, String, i64, i64) = sqlx::query_as("SELECT upload.state,job.state,library.quota_used_bytes,(SELECT count(*) FROM folioharbor.items) FROM folioharbor.upload_sessions upload JOIN folioharbor.background_jobs job ON job.job_id=$2 JOIN folioharbor.libraries library ON library.library_id=upload.library_id WHERE upload.upload_id=$1")
        .bind(upload.as_uuid()).bind(job_id.as_uuid()).fetch_one(&pools.owner).await?;
    assert_eq!(
        before_restart,
        ("ready".into(), "leased".into(), payload_bytes, 1)
    );
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    let first_runner = WorkerRunner::new(
        jobs.clone(),
        handlers.clone(),
        "restarted-worker-a".into(),
        RunnerConfig::new(1).expect("one slot"),
    );
    let second_runner = WorkerRunner::new(
        jobs,
        handlers,
        "restarted-worker-b".into(),
        RunnerConfig::new(1).expect("one slot"),
    );
    let (first_count, second_count) =
        tokio::join!(first_runner.run_once(), second_runner.run_once());
    assert_eq!(first_count? + second_count?, 1);
    let after_restart: (String, i64, i64) = sqlx::query_as("SELECT job.state,library.quota_used_bytes,(SELECT count(*) FROM folioharbor.items) FROM folioharbor.background_jobs job JOIN folioharbor.libraries library USING(library_id) WHERE job.job_id=$1")
        .bind(job_id.as_uuid()).fetch_one(&pools.owner).await?;
    assert_eq!(after_restart, ("succeeded".into(), payload_bytes, 1));
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}
