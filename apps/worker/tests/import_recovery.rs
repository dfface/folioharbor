#![allow(clippy::expect_used, clippy::too_many_lines)]

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use folioharbor_application::{
    imports::{CleanupImports, JobFailure, ProcessImportJob, RetrySchedule},
    ports::{BlobStore, ImportRepository, JobRepository, LeaseJobs, PublicationParser},
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
    DatabaseContext, PgCatalogRepository, PgImportCleanupRepository, PgImportRepository,
    PgJobRepository, PgPools, PgTransactionContext, connect_worker, run_migrations,
};
use folioharbor_storage_local::LocalBlobStore;
use folioharbor_test_support::postgres::TestPostgres;
use folioharbor_worker::{JobDispatcher, RunnerConfig, WorkerRunner, handlers::WorkerHandlers};
use secrecy::SecretString;
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

#[test]
fn subprocess_worker_child() -> anyhow::Result<()> {
    let Ok(database_url) = std::env::var("FOLIOHARBOR_TEST_CHILD_DATABASE_URL") else {
        return Ok(());
    };
    let storage_root = std::env::var("FOLIOHARBOR_TEST_CHILD_STORAGE_ROOT")?;
    let owner = std::env::var("FOLIOHARBOR_TEST_CHILD_OWNER")?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async move {
            let pool = connect_worker(&SecretString::from(database_url.into_boxed_str())).await?;
            let blobs: Arc<dyn BlobStore> = Arc::new(LocalBlobStore::new(storage_root));
            let process = Arc::new(ProcessImportJob::new(
                Arc::new(PgImportRepository::new(pool.clone())),
                Arc::new(EpubPublicationParser::new(blobs, ParserLimits::default())),
                Arc::new(PgCatalogRepository::new(pool.clone())),
                RetrySchedule::default(),
            ));
            let runner = WorkerRunner::new(
                Arc::new(PgJobRepository::new(pool.clone())),
                Arc::new(WorkerHandlers::new(process)),
                owner,
                RunnerConfig::new(1).expect("one child slot"),
            );
            runner.run_once().await?;
            pool.close().await;
            Ok::<_, anyhow::Error>(())
        })
}

struct FailureDispatcher(Mutex<HashMap<String, Option<JobFailure>>>);

#[async_trait]
impl JobDispatcher for FailureDispatcher {
    async fn dispatch(
        &self,
        job: folioharbor_domain::imports::job::LeasedJob,
    ) -> Result<(), JobFailure> {
        let JobInput::ImportEpubV1 { upload_id } = &job.input else {
            return Ok(());
        };
        self.0
            .lock()
            .expect("failure fixture")
            .get_mut(upload_id)
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
            Some(library),
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
        "operator_required".into(),
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
async fn durable_cleanup_jobs_dispatch_all_closed_kinds_and_restart_with_new_cutoffs()
-> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_migrations(&pools.owner).await?;
    let directory = tempfile::tempdir()?;
    let blobs: Arc<dyn BlobStore> = Arc::new(LocalBlobStore::new(directory.path()));
    let process = Arc::new(ProcessImportJob::new(
        Arc::new(PgImportRepository::new(pools.worker.clone())),
        Arc::new(EpubPublicationParser::new(
            blobs.clone(),
            ParserLimits::default(),
        )),
        Arc::new(PgCatalogRepository::new(pools.worker.clone())),
        RetrySchedule::default(),
    ));
    let cleanup = Arc::new(CleanupImports::new(
        Arc::new(PgImportCleanupRepository::new(pools.worker.clone())),
        blobs,
    ));
    let first_jobs = Arc::new(PgJobRepository::new(pools.worker.clone()));
    let first_now = OffsetDateTime::now_utc();
    first_jobs.ensure_cleanup_jobs(first_now).await?;
    let first_runner = WorkerRunner::new(
        first_jobs,
        Arc::new(WorkerHandlers::with_cleanup(
            process.clone(),
            cleanup.clone(),
        )),
        "cleanup-process-a".into(),
        RunnerConfig::new(3).expect("three cleanup slots"),
    );
    assert_eq!(first_runner.run_once().await?, 3);
    let first: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM folioharbor.background_jobs WHERE kind<>'import_epub' AND state='succeeded'),(SELECT count(*) FROM folioharbor.cleanup_boundaries WHERE active_cutoff IS NULL)",
    )
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(first, (3, 3));
    let first_completed_at: OffsetDateTime =
        sqlx::query_scalar("SELECT max(updated_at) FROM folioharbor.cleanup_boundaries")
            .fetch_one(&pools.owner)
            .await?;

    let restarted_jobs = Arc::new(PgJobRepository::new(pools.worker.clone()));
    restarted_jobs
        .ensure_cleanup_jobs(OffsetDateTime::now_utc())
        .await?;
    let restarted = WorkerRunner::new(
        restarted_jobs,
        Arc::new(WorkerHandlers::with_cleanup(process, cleanup)),
        "cleanup-process-b".into(),
        RunnerConfig::new(1).expect("one cleanup slot"),
    );
    assert_eq!(restarted.run_once().await?, 1);
    let advanced: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM folioharbor.cleanup_boundaries WHERE updated_at>$1",
    )
    .bind(first_completed_at)
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(advanced, 1);
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum RecoveryCut {
    ReceivedRecord,
    BlobPromotion,
    ParserCompletion,
    CatalogTransaction,
    QuotaConsume,
    JobSuccess,
}

struct SeededImport {
    actor: UserId,
    library: LibraryId,
    upload: UploadId,
    job: JobId,
    bytes: i64,
    storage_key: StorageKey,
}

async fn seed_import(
    pools: &PgPools,
    blobs: &Arc<dyn BlobStore>,
    sequence: usize,
) -> anyhow::Result<SeededImport> {
    let now = OffsetDateTime::now_utc();
    let actor = UserId::new();
    let library = LibraryId::new();
    let upload = UploadId::new();
    let payload = fixtures::valid_epub()?;
    let bytes = i64::try_from(payload.len())?;
    let digest = Sha256Digest::from_bytes(Sha256::digest(&payload).into());
    let identity = BlobIdentity::new(
        StorageNamespace::for_scope(DedupScope::Instance, library, upload),
        digest,
        ByteCount::new(payload.len() as u64),
    );
    let staging = StorageKey::from_opaque(format!("staging:{:064x}", sequence + 1));
    blobs.create_staging_for(&staging).await?;
    blobs.append(&staging, &payload).await?;
    let promoted = blobs.promote(&staging, &identity).await?;
    sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,$2,$2,'verified',$3,$3)")
        .bind(actor.as_uuid()).bind(format!("matrix-{sequence}@test.invalid")).bind(now)
        .execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,quota_reserved_bytes,created_at,updated_at) VALUES($1,$2,$3,$4,$4)")
        .bind(library.as_uuid()).bind(format!("Matrix {sequence}")).bind(bytes).bind(now)
        .execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.library_memberships(library_id,user_id,role_code,status,joined_at) VALUES($1,$2,'editor','active',$3)")
        .bind(library.as_uuid()).bind(actor.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.upload_sessions(upload_id,library_id,created_by,file_name,media_type,declared_bytes,received_bytes,state,dedup_scope,storage_key,sha256,expires_at,created_at,updated_at) VALUES($1,$2,$3,'book.epub','application/epub+zip',$4,$4,'queued','instance',$5,$6,$7,$8,$8)")
        .bind(upload.as_uuid()).bind(library.as_uuid()).bind(actor.as_uuid()).bind(bytes)
        .bind(promoted.key.as_str()).bind(digest.as_bytes().to_vec()).bind(now+Duration::hours(1)).bind(now)
        .execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.quota_reservations(upload_id,library_id,reserved_bytes,expires_at,state) VALUES($1,$2,$3,$4,'active')")
        .bind(upload.as_uuid()).bind(library.as_uuid()).bind(bytes).bind(now+Duration::hours(1))
        .execute(&pools.owner).await?;
    let job = JobId::new();
    PgJobRepository::new(pools.worker.clone())
        .enqueue(
            job,
            Some(library),
            JobKind::ImportEpub,
            JobInput::upload_v1(upload.as_uuid().to_string()),
            &format!("matrix:{sequence}"),
            now,
        )
        .await?;
    Ok(SeededImport {
        actor,
        library,
        upload,
        job,
        bytes,
        storage_key: promoted.key,
    })
}

fn import_service(pools: &PgPools, blobs: Arc<dyn BlobStore>, owner: &str) -> WorkerRunner {
    let process = Arc::new(ProcessImportJob::new(
        Arc::new(PgImportRepository::new(pools.worker.clone())),
        Arc::new(EpubPublicationParser::new(blobs, ParserLimits::default())),
        Arc::new(PgCatalogRepository::new(pools.worker.clone())),
        RetrySchedule::default(),
    ));
    WorkerRunner::new(
        Arc::new(PgJobRepository::new(pools.worker.clone())),
        Arc::new(WorkerHandlers::new(process)),
        owner.to_owned(),
        RunnerConfig::new(1).expect("one matrix slot"),
    )
}

#[tokio::test]
async fn persisted_service_restart_fault_matrix_covers_all_six_boundaries() -> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_migrations(&pools.owner).await?;
    let directory = tempfile::tempdir()?;
    let blobs: Arc<dyn BlobStore> = Arc::new(LocalBlobStore::new(directory.path()));
    let cuts = [
        RecoveryCut::ReceivedRecord,
        RecoveryCut::BlobPromotion,
        RecoveryCut::ParserCompletion,
        RecoveryCut::CatalogTransaction,
        RecoveryCut::QuotaConsume,
        RecoveryCut::JobSuccess,
    ];

    for (sequence, cut) in cuts.into_iter().enumerate() {
        let seeded = seed_import(&pools, &blobs, sequence).await?;
        match cut {
            RecoveryCut::ReceivedRecord => {
                sqlx::query("DELETE FROM folioharbor.background_jobs WHERE job_id=$1")
                    .bind(seeded.job.as_uuid())
                    .execute(&pools.owner)
                    .await?;
                sqlx::query(
                    "UPDATE folioharbor.upload_sessions SET state='received' WHERE upload_id=$1",
                )
                .bind(seeded.upload.as_uuid())
                .execute(&pools.owner)
                .await?;
                let durable: (String, i64) = sqlx::query_as(
                    "SELECT state,received_bytes FROM folioharbor.upload_sessions WHERE upload_id=$1")
                    .bind(seeded.upload.as_uuid()).fetch_one(&pools.owner).await?;
                assert_eq!(durable, ("received".into(), seeded.bytes));
                let request = RequestId::new();
                let now = OffsetDateTime::now_utc();
                let mut transaction = pools.api.begin().await?;
                PgTransactionContext::apply(
                    &mut transaction,
                    &DatabaseContext::api(seeded.actor, seeded.library, request),
                )
                .await?;
                let finalized: bool = sqlx::query_scalar(
                    "SELECT folioharbor.upload_finalize_authorized($1,$2,$3,$4,$5,$6,$7,$8)",
                )
                .bind(seeded.upload.as_uuid())
                .bind(seeded.library.as_uuid())
                .bind(seeded.actor.as_uuid())
                .bind(seeded.bytes)
                .bind(seeded.storage_key.as_str())
                .bind("staging:unused")
                .bind(seeded.job.as_uuid())
                .bind(now)
                .fetch_one(&mut *transaction)
                .await?;
                assert!(finalized);
                transaction.commit().await?;
            }
            RecoveryCut::BlobPromotion => {
                let mut source = blobs.open_publication(&seeded.storage_key).await?;
                assert!(std::io::Read::read(&mut source, &mut [0_u8; 1])? > 0);
            }
            RecoveryCut::ParserCompletion => {
                let jobs = PgJobRepository::new(pools.worker.clone());
                let now = OffsetDateTime::now_utc();
                let leased = jobs
                    .lease(LeaseJobs {
                        owner: "parser-crash".into(),
                        now,
                        lease_for: Duration::minutes(1),
                        limit: 1,
                        request_id: RequestId::new(),
                    })
                    .await?;
                let imports = PgImportRepository::new(pools.worker.clone());
                imports
                    .reconcile(seeded.upload, seeded.library, RequestId::new(), now)
                    .await?;
                EpubPublicationParser::new(blobs.clone(), ParserLimits::default())
                    .parse(&seeded.storage_key)
                    .await
                    .expect("parser completes before crash");
                sqlx::query(
                    "UPDATE folioharbor.background_jobs SET lease_expires_at=$2 WHERE job_id=$1",
                )
                .bind(leased[0].job_id.as_uuid())
                .bind(now - Duration::seconds(1))
                .execute(&pools.owner)
                .await?;
            }
            RecoveryCut::CatalogTransaction => {
                sqlx::query(
                    "UPDATE folioharbor.upload_sessions SET state='importing' WHERE upload_id=$1",
                )
                .bind(seeded.upload.as_uuid())
                .execute(&pools.owner)
                .await?;
            }
            RecoveryCut::QuotaConsume => {
                let jobs = PgJobRepository::new(pools.worker.clone());
                let now = OffsetDateTime::now_utc();
                let leased = jobs
                    .lease(LeaseJobs {
                        owner: "quota-crash".into(),
                        now,
                        lease_for: Duration::minutes(1),
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
                WorkerHandlers::new(process)
                    .dispatch(leased[0].clone())
                    .await
                    .expect("catalog and quota commit before crash");
                sqlx::query(
                    "UPDATE folioharbor.background_jobs SET lease_expires_at=$2 WHERE job_id=$1",
                )
                .bind(seeded.job.as_uuid())
                .bind(now - Duration::seconds(1))
                .execute(&pools.owner)
                .await?;
            }
            RecoveryCut::JobSuccess => {
                import_service(&pools, blobs.clone(), "job-success-first")
                    .run_once()
                    .await?;
            }
        }
        let restarted =
            import_service(&pools, blobs.clone(), &format!("matrix-restart-{sequence}"));
        let processed = restarted.run_once().await?;
        if matches!(cut, RecoveryCut::JobSuccess) {
            assert_eq!(processed, 0);
        } else {
            assert_eq!(processed, 1);
        }
        let invariant: (String, i64, i64, i64) = sqlx::query_as(
            "SELECT job.state,library.quota_used_bytes,(SELECT count(*) FROM folioharbor.items item JOIN folioharbor.holdings holding USING(holding_id) WHERE holding.library_id=$2),(SELECT count(*) FROM folioharbor.blob_locations location WHERE location.storage_key=$3 AND location.state='ready') FROM folioharbor.background_jobs job JOIN folioharbor.libraries library ON library.library_id=$2 WHERE job.job_id=$1")
            .bind(seeded.job.as_uuid()).bind(seeded.library.as_uuid()).bind(seeded.storage_key.as_str())
            .fetch_one(&pools.owner).await?;
        assert_eq!(
            invariant,
            ("succeeded".into(), seeded.bytes, 1, 1),
            "{cut:?}"
        );
    }
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
        Some(library),
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
    drop(handlers);
    drop(jobs);
    let executable = std::env::current_exe()?;
    let worker_url = database.worker_url()?;
    let mut first = std::process::Command::new(&executable)
        .args(["--exact", "subprocess_worker_child", "--nocapture"])
        .env("FOLIOHARBOR_TEST_CHILD_DATABASE_URL", &worker_url)
        .env("FOLIOHARBOR_TEST_CHILD_STORAGE_ROOT", directory.path())
        .env("FOLIOHARBOR_TEST_CHILD_OWNER", "process-worker-a")
        .spawn()?;
    let mut second = std::process::Command::new(&executable)
        .args(["--exact", "subprocess_worker_child", "--nocapture"])
        .env("FOLIOHARBOR_TEST_CHILD_DATABASE_URL", &worker_url)
        .env("FOLIOHARBOR_TEST_CHILD_STORAGE_ROOT", directory.path())
        .env("FOLIOHARBOR_TEST_CHILD_OWNER", "process-worker-b")
        .spawn()?;
    assert!(first.wait()?.success());
    assert!(second.wait()?.success());
    let after_restart: (String, i32, i64, i64) = sqlx::query_as("SELECT job.state,job.attempt_count,library.quota_used_bytes,(SELECT count(*) FROM folioharbor.items) FROM folioharbor.background_jobs job JOIN folioharbor.libraries library USING(library_id) WHERE job.job_id=$1")
        .bind(job_id.as_uuid()).fetch_one(&pools.owner).await?;
    assert_eq!(after_restart, ("succeeded".into(), 2, payload_bytes, 1));
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}
