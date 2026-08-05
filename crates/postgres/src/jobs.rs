use async_trait::async_trait;
use folioharbor_application::ports::{JobRepository, JobRepositoryError, LeaseJobs};
use folioharbor_domain::{
    id::{JobId, LibraryId, RequestId},
    imports::job::{JobInput, JobKind, LeasedJob},
    time::OffsetDateTime,
};
use sqlx::PgPool;
use time::Duration;

use crate::{DatabaseContext, PgTransactionContext};

#[derive(Clone, Debug)]
pub struct PgJobRepository {
    pool: PgPool,
}
struct FinishJob<'a> {
    id: JobId,
    owner: &'a str,
    now: OffsetDateTime,
    state: &'a str,
    next: Option<OffsetDateTime>,
    code: Option<&'a str>,
    summary: Option<&'a str>,
    outcome: &'a str,
}
impl PgJobRepository {
    #[must_use]
    pub const fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}
fn persistence_error(_: sqlx::Error) -> JobRepositoryError {
    JobRepositoryError
}

impl PgJobRepository {
    async fn transaction(
        &self,
        request: RequestId,
        library: Option<LibraryId>,
    ) -> Result<sqlx::Transaction<'_, sqlx::Postgres>, JobRepositoryError> {
        let mut transaction = self.pool.begin().await.map_err(persistence_error)?;
        PgTransactionContext::apply(&mut transaction, &DatabaseContext::worker(request, library))
            .await
            .map_err(persistence_error)?;
        Ok(transaction)
    }

    async fn finish(&self, job: FinishJob<'_>) -> Result<bool, JobRepositoryError> {
        let mut transaction = self.transaction(RequestId::new(), None).await?;
        let changed = sqlx::query!("WITH changed AS (UPDATE folioharbor.background_jobs SET state=$4,next_run_at=COALESCE($5,next_run_at),lease_owner=NULL,lease_expires_at=NULL,error_code=$6,error_summary=$7,updated_at=$3 WHERE job_id=$1 AND state='leased' AND lease_owner=$2 RETURNING attempt_count) UPDATE folioharbor.job_attempts a SET finished_at=$3,outcome=$8,error_code=$6,error_summary=$7 FROM changed c WHERE a.job_id=$1 AND a.attempt=c.attempt_count",job.id.as_uuid(),job.owner,job.now,job.state,job.next,job.code,job.summary,job.outcome).execute(&mut *transaction).await.map_err(persistence_error)?.rows_affected() == 1;
        transaction.commit().await.map_err(persistence_error)?;
        Ok(changed)
    }
}

#[async_trait]
impl JobRepository for PgJobRepository {
    async fn enqueue(
        &self,
        id: JobId,
        library: LibraryId,
        kind: JobKind,
        input: JobInput,
        idempotency_key: &str,
        run_at: OffsetDateTime,
    ) -> Result<JobId, JobRepositoryError> {
        let mut transaction = self.transaction(RequestId::new(), Some(library)).await?;
        let value = serde_json::json!({"version": input.version, "upload_id": input.upload_id});
        let stored = sqlx::query_scalar!(r#"INSERT INTO folioharbor.background_jobs(job_id,library_id,kind,state,input,idempotency_key,next_run_at,created_at,updated_at) VALUES($1,$2,$3,'pending',$4,$5,$6,$6,$6) ON CONFLICT(idempotency_key) DO UPDATE SET idempotency_key=EXCLUDED.idempotency_key RETURNING job_id AS "job_id!""#,id.as_uuid(),library.as_uuid(),kind.as_str(),value,idempotency_key,run_at).fetch_one(&mut *transaction).await.map_err(persistence_error)?;
        transaction.commit().await.map_err(persistence_error)?;
        Ok(JobId::from_uuid(stored))
    }

    async fn lease(&self, request: LeaseJobs) -> Result<Vec<LeasedJob>, JobRepositoryError> {
        let mut transaction = self.transaction(request.request_id, None).await?;
        let expires = request.now + request.lease_for;
        let rows = sqlx::query!(r#"WITH candidates AS (SELECT job_id,state,attempt_count FROM folioharbor.background_jobs WHERE next_run_at <= $1 AND (state IN ('pending','retry_wait') OR (state='leased' AND lease_expires_at <= $1)) ORDER BY next_run_at,created_at FOR UPDATE SKIP LOCKED LIMIT $2), expired AS (UPDATE folioharbor.job_attempts a SET finished_at=$1,outcome='lease_expired' FROM candidates c WHERE c.state='leased' AND a.job_id=c.job_id AND a.attempt=c.attempt_count), leased AS (UPDATE folioharbor.background_jobs j SET state='leased',lease_owner=$3,lease_expires_at=$4,attempt_count=j.attempt_count+1,error_code=NULL,error_summary=NULL,updated_at=$1 FROM candidates c WHERE j.job_id=c.job_id RETURNING j.job_id,j.library_id,j.kind,j.input,j.attempt_count,j.lease_expires_at), attempts AS (INSERT INTO folioharbor.job_attempts(job_id,attempt,lease_owner,started_at) SELECT job_id,attempt_count,$3,$1 FROM leased) SELECT job_id AS "job_id!",library_id AS "library_id!",kind AS "kind!",input AS "input!",attempt_count AS "attempt_count!",lease_expires_at AS "lease_expires_at!" FROM leased"#,request.now,i64::from(request.limit),&request.owner,expires).fetch_all(&mut *transaction).await.map_err(persistence_error)?;
        transaction.commit().await.map_err(persistence_error)?;
        rows.into_iter()
            .map(|row| {
                let kind = row.kind;
                let input = row.input;
                let version = input
                    .get("version")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|value| u16::try_from(value).ok())
                    .ok_or(JobRepositoryError)?;
                let upload_id = input
                    .get("upload_id")
                    .and_then(serde_json::Value::as_str)
                    .ok_or(JobRepositoryError)?
                    .to_owned();
                Ok(LeasedJob {
                    job_id: JobId::from_uuid(row.job_id),
                    library_id: LibraryId::from_uuid(row.library_id),
                    kind: JobKind::parse(&kind).ok_or(JobRepositoryError)?,
                    input: JobInput { version, upload_id },
                    attempt: u32::try_from(row.attempt_count).map_err(|_| JobRepositoryError)?,
                    lease_expires_at: row.lease_expires_at,
                })
            })
            .collect()
    }

    async fn heartbeat(
        &self,
        id: JobId,
        owner: &str,
        now: OffsetDateTime,
        lease_for: Duration,
    ) -> Result<bool, JobRepositoryError> {
        let mut transaction = self.transaction(RequestId::new(), None).await?;
        let changed = sqlx::query!("UPDATE folioharbor.background_jobs SET lease_expires_at=$3,updated_at=$2 WHERE job_id=$1 AND state='leased' AND lease_owner=$4 AND lease_expires_at>$2",id.as_uuid(),now,now+lease_for,owner).execute(&mut *transaction).await.map_err(persistence_error)?.rows_affected()==1;
        transaction.commit().await.map_err(persistence_error)?;
        Ok(changed)
    }
    async fn succeed(
        &self,
        id: JobId,
        owner: &str,
        now: OffsetDateTime,
    ) -> Result<bool, JobRepositoryError> {
        self.finish(FinishJob {
            id,
            owner,
            now,
            state: "succeeded",
            next: None,
            code: None,
            summary: None,
            outcome: "succeeded",
        })
        .await
    }
    async fn retry(
        &self,
        id: JobId,
        owner: &str,
        now: OffsetDateTime,
        next_run: OffsetDateTime,
        code: &str,
        summary: &str,
    ) -> Result<bool, JobRepositoryError> {
        self.finish(FinishJob {
            id,
            owner,
            now,
            state: "retry_wait",
            next: Some(next_run),
            code: Some(code),
            summary: Some(summary),
            outcome: "retry",
        })
        .await
    }
    async fn fail(
        &self,
        id: JobId,
        owner: &str,
        now: OffsetDateTime,
        code: &str,
        summary: &str,
    ) -> Result<bool, JobRepositoryError> {
        self.finish(FinishJob {
            id,
            owner,
            now,
            state: "failed",
            next: None,
            code: Some(code),
            summary: Some(summary),
            outcome: "failed",
        })
        .await
    }
}
