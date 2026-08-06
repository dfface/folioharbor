use folioharbor_application::ports::{JobRepository, LeaseJobs};
use folioharbor_domain::{
    id::{JobId, LibraryId, RequestId, UploadId, UserId},
    imports::job::{JobInput, JobKind},
};
use folioharbor_postgres::{PgJobRepository, PgPools, run_migrations};
use folioharbor_test_support::postgres::TestPostgres;
use time::{Duration, OffsetDateTime};

#[tokio::test]
async fn concurrent_workers_never_lease_the_same_job_and_expired_leases_recover()
-> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_migrations(&pools.owner).await?;
    let repository = PgJobRepository::new(pools.worker.clone());
    let now = OffsetDateTime::now_utc();
    let library = LibraryId::new();
    let user = UserId::new();
    sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,$2,$2,'verified',$3,$3)")
        .bind(user.as_uuid()).bind("jobs@test.invalid").bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,created_at,updated_at) VALUES($1,'Jobs',$2,$2)")
        .bind(library.as_uuid()).bind(now).execute(&pools.owner).await?;
    let job_id = JobId::new();
    repository
        .enqueue(
            job_id,
            Some(library),
            JobKind::ImportEpub,
            JobInput::upload_v1("018f4cf8-1f46-7cc4-98c7-4aef77db10f3"),
            "upload:one",
            now,
        )
        .await?;

    let first = repository.clone();
    let second = repository.clone();
    let (a, b) = tokio::join!(
        first.lease(LeaseJobs {
            owner: "worker-a".into(),
            now,
            lease_for: Duration::minutes(5),
            limit: 1,
            request_id: RequestId::new()
        }),
        second.lease(LeaseJobs {
            owner: "worker-b".into(),
            now,
            lease_for: Duration::minutes(5),
            limit: 1,
            request_id: RequestId::new()
        })
    );
    assert_eq!(a?.len() + b?.len(), 1);

    let recovered = repository
        .lease(LeaseJobs {
            owner: "worker-c".into(),
            now: now + Duration::minutes(6),
            lease_for: Duration::minutes(5),
            limit: 1,
            request_id: RequestId::new(),
        })
        .await?;
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].job_id, job_id);
    assert_eq!(recovered[0].attempt, 2);
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn retry_schedule_and_heartbeat_survive_repository_restart() -> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_migrations(&pools.owner).await?;
    let now = OffsetDateTime::now_utc();
    let library = LibraryId::new();
    let user = UserId::new();
    sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,$2,$2,'verified',$3,$3)").bind(user.as_uuid()).bind("restart@test.invalid").bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,created_at,updated_at) VALUES($1,'Restart',$2,$2)").bind(library.as_uuid()).bind(now).execute(&pools.owner).await?;
    let repository = PgJobRepository::new(pools.worker.clone());
    let id = JobId::new();
    repository
        .enqueue(
            id,
            Some(library),
            JobKind::ImportEpub,
            JobInput::upload_v1(UploadId::new().as_uuid().to_string()),
            "restart:one",
            now,
        )
        .await?;
    let leased = repository
        .lease(LeaseJobs {
            owner: "worker-a".into(),
            now,
            lease_for: Duration::minutes(2),
            limit: 1,
            request_id: RequestId::new(),
        })
        .await?;
    assert_eq!(leased.len(), 1);
    assert!(
        repository
            .heartbeat(
                id,
                "worker-a",
                now + Duration::minutes(1),
                Duration::minutes(3)
            )
            .await?
    );
    let next = now + Duration::minutes(10);
    assert!(
        repository
            .retry(
                id,
                "worker-a",
                now + Duration::minutes(2),
                next,
                "temporary_io",
                "safe summary"
            )
            .await?
    );
    let restarted = PgJobRepository::new(pools.worker.clone());
    assert!(
        restarted
            .lease(LeaseJobs {
                owner: "worker-b".into(),
                now: next - Duration::seconds(1),
                lease_for: Duration::minutes(1),
                limit: 1,
                request_id: RequestId::new()
            })
            .await?
            .is_empty()
    );
    let retried = restarted
        .lease(LeaseJobs {
            owner: "worker-b".into(),
            now: next,
            lease_for: Duration::minutes(1),
            limit: 1,
            request_id: RequestId::new(),
        })
        .await?;
    assert_eq!(retried[0].attempt, 2);
    assert!(
        restarted
            .succeed(id, "worker-b", next + Duration::seconds(1))
            .await?
    );
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "the real PostgreSQL operator lifecycle is clearest as one linear scenario"
)]
async fn operator_required_is_durable_distinct_and_never_leased_after_restart() -> anyhow::Result<()>
{
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_migrations(&pools.owner).await?;
    let now = OffsetDateTime::now_utc();
    let library = LibraryId::new();
    let user = UserId::new();
    sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,'operator@test.invalid','operator@test.invalid','verified',$2,$2)")
        .bind(user.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,created_at,updated_at) VALUES($1,'Operator',$2,$2)")
        .bind(library.as_uuid()).bind(now).execute(&pools.owner).await?;
    let id = JobId::new();
    let upload = UploadId::new();
    sqlx::query("UPDATE folioharbor.libraries SET quota_reserved_bytes=10 WHERE library_id=$1")
        .bind(library.as_uuid())
        .execute(&pools.owner)
        .await?;
    sqlx::query("INSERT INTO folioharbor.upload_sessions(upload_id,library_id,created_by,file_name,media_type,declared_bytes,received_bytes,state,dedup_scope,storage_key,sha256,expires_at,created_at,updated_at) VALUES($1,$2,$3,'operator.epub','application/epub+zip',10,10,'operator_required','instance','blob:operator',decode(repeat('44',32),'hex'),$4,$5,$5)")
        .bind(upload.as_uuid()).bind(library.as_uuid()).bind(user.as_uuid())
        .bind(now+Duration::hours(1)).bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.quota_reservations(upload_id,library_id,reserved_bytes,expires_at,state) VALUES($1,$2,10,$3,'active')")
        .bind(upload.as_uuid()).bind(library.as_uuid()).bind(now+Duration::hours(1)).execute(&pools.owner).await?;
    let repository = PgJobRepository::new(pools.worker.clone());
    repository
        .enqueue(
            id,
            Some(library),
            JobKind::ImportEpub,
            JobInput::upload_v1(upload.as_uuid().to_string()),
            "operator:one",
            now,
        )
        .await?;
    assert_eq!(
        repository
            .lease(LeaseJobs {
                owner: "worker-a".into(),
                now,
                lease_for: Duration::minutes(1),
                limit: 1,
                request_id: RequestId::new()
            })
            .await?
            .len(),
        1
    );
    assert!(
        repository
            .operator_required(
                id,
                "worker-a",
                now + Duration::seconds(1),
                "schema_incompatible",
                "operator action required"
            )
            .await?
    );

    let persisted: (String, Option<String>) =
        sqlx::query_as("SELECT state,error_code FROM folioharbor.background_jobs WHERE job_id=$1")
            .bind(id.as_uuid())
            .fetch_one(&pools.owner)
            .await?;
    assert_eq!(
        persisted,
        (
            "operator_required".into(),
            Some("schema_incompatible".into())
        )
    );
    let restarted = PgJobRepository::new(pools.worker.clone());
    assert!(
        restarted
            .lease(LeaseJobs {
                owner: "worker-b".into(),
                now: now + Duration::days(1),
                lease_for: Duration::minutes(1),
                limit: 1,
                request_id: RequestId::new()
            })
            .await?
            .is_empty()
    );
    let retained: (String, String, i64, i64) = sqlx::query_as(
        "SELECT upload.state,upload.storage_key,library.quota_reserved_bytes,(SELECT count(*) FROM folioharbor.failed_upload_purges WHERE upload_id=$1) FROM folioharbor.upload_sessions upload JOIN folioharbor.libraries library USING(library_id) WHERE upload.upload_id=$1",
    )
    .bind(upload.as_uuid())
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(
        retained,
        ("operator_required".into(), "blob:operator".into(), 10, 0)
    );
    assert!(
        restarted
            .resume_operator_required(id, now + Duration::days(1))
            .await?
    );
    let resumed: (String, String, i64) = sqlx::query_as(
        "SELECT upload.state,job.state,library.quota_reserved_bytes FROM folioharbor.upload_sessions upload JOIN folioharbor.background_jobs job ON job.job_id=$2 JOIN folioharbor.libraries library ON library.library_id=upload.library_id WHERE upload.upload_id=$1",
    )
    .bind(upload.as_uuid()).bind(id.as_uuid()).fetch_one(&pools.owner).await?;
    assert_eq!(resumed, ("queued".into(), "pending".into(), 10));
    assert_eq!(
        restarted
            .lease(LeaseJobs {
                owner: "worker-c".into(),
                now: now + Duration::days(1),
                lease_for: Duration::minutes(1),
                limit: 1,
                request_id: RequestId::new()
            })
            .await?
            .len(),
        1
    );
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "the three cleanup schedules are verified together as one singleton lifecycle"
)]
async fn cleanup_job_kinds_are_global_idempotent_leasable_and_recurring() -> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_migrations(&pools.owner).await?;
    let now = OffsetDateTime::now_utc();
    let repository = PgJobRepository::new(pools.worker.clone());
    repository.ensure_cleanup_jobs(now).await?;
    repository.ensure_cleanup_jobs(now).await?;
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM folioharbor.background_jobs WHERE kind<>'import_epub'",
    )
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(count, 3);
    let leased = repository
        .lease(LeaseJobs {
            owner: "cleanup-worker".into(),
            now,
            lease_for: Duration::minutes(5),
            limit: 3,
            request_id: RequestId::new(),
        })
        .await?;
    assert_eq!(leased.len(), 3);
    assert!(
        leased
            .iter()
            .all(|job| job.library_id.is_none() && matches!(job.input, JobInput::CleanupV1))
    );
    assert!(
        leased
            .iter()
            .any(|job| job.kind == JobKind::ExpireUploadsAndReservations)
    );
    assert!(
        leased
            .iter()
            .any(|job| job.kind == JobKind::PurgeFailedUploads)
    );
    assert!(
        leased
            .iter()
            .any(|job| job.kind == JobKind::CollectBlobsLater)
    );
    for job in leased {
        assert!(
            repository
                .succeed(job.job_id, "cleanup-worker", now + Duration::seconds(1))
                .await?
        );
    }
    let scheduled_at = now + Duration::seconds(1);
    repository.ensure_cleanup_jobs(scheduled_at).await?;
    repository.ensure_cleanup_jobs(scheduled_at).await?;
    let recurring: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM folioharbor.background_jobs WHERE kind<>'import_epub' AND state='pending'",
    )
    .fetch_one(&pools.owner)
    .await?;
    assert_eq!(recurring, 3);
    let schedules: Vec<(String, OffsetDateTime)> = sqlx::query_as(
        "SELECT kind,next_run_at FROM folioharbor.background_jobs WHERE kind<>'import_epub' ORDER BY kind",
    )
    .fetch_all(&pools.owner)
    .await?;
    assert_eq!(
        schedules,
        vec![
            (
                "collect_blobs_later".into(),
                scheduled_at + Duration::hours(1),
            ),
            (
                "expire_uploads_and_reservations".into(),
                scheduled_at + Duration::minutes(1),
            ),
            (
                "purge_failed_uploads".into(),
                scheduled_at + Duration::minutes(5),
            ),
        ]
    );
    assert!(
        repository
            .lease(LeaseJobs {
                owner: "cleanup-worker".into(),
                now: scheduled_at + Duration::minutes(1) - Duration::milliseconds(1),
                lease_for: Duration::minutes(5),
                limit: 3,
                request_id: RequestId::new(),
            })
            .await?
            .is_empty()
    );
    let due = repository
        .lease(LeaseJobs {
            owner: "cleanup-worker".into(),
            now: scheduled_at + Duration::minutes(1),
            lease_for: Duration::minutes(5),
            limit: 3,
            request_id: RequestId::new(),
        })
        .await?;
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].kind, JobKind::ExpireUploadsAndReservations);
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn stale_leases_cannot_heartbeat_succeed_retry_or_fail() -> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_migrations(&pools.owner).await?;
    let now = OffsetDateTime::now_utc();
    let stale = now + Duration::minutes(2);
    let library = LibraryId::new();
    let user = UserId::new();
    sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,$2,$2,'verified',$3,$3)").bind(user.as_uuid()).bind("stale-jobs@test.invalid").bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,created_at,updated_at) VALUES($1,'Stale jobs',$2,$2)").bind(library.as_uuid()).bind(now).execute(&pools.owner).await?;
    let repository = PgJobRepository::new(pools.worker.clone());
    let ids = [JobId::new(), JobId::new(), JobId::new()];
    for (index, id) in ids.into_iter().enumerate() {
        repository
            .enqueue(
                id,
                Some(library),
                JobKind::ImportEpub,
                JobInput::upload_v1(UploadId::new().as_uuid().to_string()),
                &format!("stale:{index}"),
                now,
            )
            .await?;
    }
    assert_eq!(
        repository
            .lease(LeaseJobs {
                owner: "stale-worker".into(),
                now,
                lease_for: Duration::minutes(1),
                limit: 3,
                request_id: RequestId::new(),
            })
            .await?
            .len(),
        3
    );
    assert!(
        !repository
            .heartbeat(ids[0], "stale-worker", stale, Duration::minutes(1))
            .await?
    );
    assert!(!repository.succeed(ids[0], "stale-worker", stale).await?);
    assert!(
        !repository
            .retry(
                ids[1],
                "stale-worker",
                stale,
                stale + Duration::minutes(5),
                "temporary_io",
                "safe summary",
            )
            .await?
    );
    assert!(
        !repository
            .fail(
                ids[2],
                "stale-worker",
                stale,
                "invalid_epub",
                "safe summary",
            )
            .await?
    );
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn malformed_job_input_rolls_back_batch_and_is_quarantined_once() -> anyhow::Result<()> {
    let database = TestPostgres::provision().await?;
    let pools = PgPools::connect_for_tests(
        &database.owner_url()?,
        &database.api_url()?,
        &database.worker_url()?,
    )
    .await?;
    run_migrations(&pools.owner).await?;
    let now = OffsetDateTime::now_utc();
    let library = LibraryId::new();
    let user = UserId::new();
    sqlx::query("INSERT INTO folioharbor.user_accounts(user_id,normalized_email,display_email,status,created_at,verified_at) VALUES($1,$2,$2,'verified',$3,$3)").bind(user.as_uuid()).bind("poison@test.invalid").bind(now).execute(&pools.owner).await?;
    sqlx::query("INSERT INTO folioharbor.libraries(library_id,name,created_at,updated_at) VALUES($1,'Poison',$2,$2)").bind(library.as_uuid()).bind(now).execute(&pools.owner).await?;
    sqlx::query(
        "ALTER TABLE folioharbor.background_jobs DROP CONSTRAINT background_jobs_input_check",
    )
    .execute(&pools.owner)
    .await?;
    let poison = JobId::new();
    sqlx::query("INSERT INTO folioharbor.background_jobs(job_id,library_id,kind,state,input,idempotency_key,next_run_at,created_at,updated_at) VALUES($1,$2,'import_epub','pending',$3,'poison', $4,$4,$4)")
        .bind(poison.as_uuid())
        .bind(library.as_uuid())
        .bind(serde_json::json!({"version": 1}))
        .bind(now)
        .execute(&pools.owner)
        .await?;
    let valid = JobId::new();
    let repository = PgJobRepository::new(pools.worker.clone());
    repository
        .enqueue(
            valid,
            Some(library),
            JobKind::ImportEpub,
            JobInput::upload_v1(UploadId::new().as_uuid().to_string()),
            "valid-after-poison",
            now,
        )
        .await?;
    assert!(
        repository
            .lease(LeaseJobs {
                owner: "worker-poison".into(),
                now,
                lease_for: Duration::minutes(1),
                limit: 2,
                request_id: RequestId::new(),
            })
            .await
            .is_err()
    );
    let states: Vec<(uuid::Uuid, String)> = sqlx::query_as("SELECT job_id,state FROM folioharbor.background_jobs WHERE job_id IN($1,$2) ORDER BY job_id")
        .bind(poison.as_uuid())
        .bind(valid.as_uuid())
        .fetch_all(&pools.owner)
        .await?;
    assert!(states.contains(&(poison.as_uuid(), "failed".into())));
    assert!(states.contains(&(valid.as_uuid(), "pending".into())));
    let leased = repository
        .lease(LeaseJobs {
            owner: "worker-valid".into(),
            now,
            lease_for: Duration::minutes(1),
            limit: 2,
            request_id: RequestId::new(),
        })
        .await?;
    assert_eq!(leased.len(), 1);
    assert_eq!(leased[0].job_id, valid);
    pools.close().await;
    database.cleanup().await?;
    Ok(())
}
