use std::sync::Arc;

use folioharbor_domain::{
    id::{JobId, RequestId, UploadId},
    imports::{
        job::{JobInput, JobKind, LeasedJob},
        upload::UploadState,
    },
    time::OffsetDateTime,
};
use time::Duration;

use crate::{
    catalog::ImportPublicationCatalog,
    error::AppError,
    ports::{
        CatalogRepository, ImportReconciliation, ImportRepository, ImportRepositoryError,
        ImportWork, PublicationParser, PublicationParserError,
    },
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JobOutcome {
    Succeeded,
    AlreadyComplete,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JobFailure {
    Permanent {
        code: &'static str,
        summary: String,
    },
    Transient {
        code: &'static str,
        retry_at: OffsetDateTime,
    },
    OperatorRequired {
        code: &'static str,
        summary: String,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct RetrySchedule {
    maximum: Duration,
}

impl Default for RetrySchedule {
    fn default() -> Self {
        Self {
            maximum: Duration::minutes(15),
        }
    }
}

impl RetrySchedule {
    #[must_use]
    pub fn next(self, now: OffsetDateTime, saga: JobId, attempt: u32) -> OffsetDateTime {
        let exponent = attempt.min(8);
        let base_seconds = 2_i64.pow(exponent);
        let jitter_millis = i64::from(saga.as_uuid().as_bytes()[15]) * 1_000 / 256;
        let delay = Duration::seconds(base_seconds) + Duration::milliseconds(jitter_millis);
        now + delay.min(self.maximum)
    }
}

pub struct ProcessImportJob {
    imports: Arc<dyn ImportRepository>,
    parser: Arc<dyn PublicationParser>,
    catalog: Arc<dyn CatalogRepository>,
    retries: RetrySchedule,
}

impl ProcessImportJob {
    #[must_use]
    pub fn new(
        imports: Arc<dyn ImportRepository>,
        parser: Arc<dyn PublicationParser>,
        catalog: Arc<dyn CatalogRepository>,
        retries: RetrySchedule,
    ) -> Self {
        Self {
            imports,
            parser,
            catalog,
            retries,
        }
    }

    /// Reconciles and advances one leased EPUB import saga.
    ///
    /// # Errors
    /// Returns a closed failure classification suitable for durable queue state.
    pub async fn execute(&self, job: LeasedJob) -> Result<JobOutcome, JobFailure> {
        if job.kind != JobKind::ImportEpub {
            return Err(permanent(
                "invalid_job_input",
                "job payload failed validation",
            ));
        }
        let JobInput::ImportEpubV1 { upload_id } = &job.input else {
            return Err(permanent(
                "invalid_job_input",
                "job payload failed validation",
            ));
        };
        let library_id = job
            .library_id
            .ok_or_else(|| permanent("invalid_job_input", "job payload failed validation"))?;
        let upload_uuid = uuid::Uuid::parse_str(upload_id)
            .map_err(|_| permanent("invalid_job_input", "job payload failed validation"))?;
        let upload_id = UploadId::from_uuid(upload_uuid);
        let request_id = RequestId::new();
        let now = OffsetDateTime::now_utc();
        let work = match self
            .imports
            .reconcile(upload_id, library_id, request_id, now)
            .await
            .map_err(|error| self.repository_failure(error, &job, now))?
        {
            ImportReconciliation::Complete => return Ok(JobOutcome::AlreadyComplete),
            ImportReconciliation::TerminalFailure { code }
            | ImportReconciliation::OperatorRequired { code } => {
                return Err(reconciled_failure(&code));
            }
            ImportReconciliation::Work(work) => work,
        };
        let publication = match self.parser.parse(&work.storage_key).await {
            Ok(publication) => publication,
            Err(error) => {
                return Err(self
                    .parser_failure(&work, &job, request_id, now, error)
                    .await);
            }
        };
        self.imports
            .begin_catalog(&work, request_id, now)
            .await
            .map_err(|error| self.repository_failure(error, &job, now))?;
        let result = ImportPublicationCatalog::new(self.catalog.as_ref())
            .execute(crate::catalog::ImportCatalogCommand {
                library_id: work.library_id,
                upload_id: work.upload_id,
                actor_id: work.actor_id,
                original_blob_id: work.blob_id,
                logical_bytes: work.logical_bytes,
                parser_profile_version: self.parser.profile_version().to_owned(),
                publication,
                request_id,
                now,
            })
            .await;
        match result {
            Ok(_) => Ok(JobOutcome::Succeeded),
            Err(error) => {
                let (state, code, failure) = match error {
                    AppError::Conflict { .. } | AppError::Invalid { .. } => (
                        UploadState::Failed,
                        "catalog_import_invalid",
                        permanent("catalog_import_invalid", "catalog import is not valid"),
                    ),
                    _ => (
                        UploadState::RetryWait,
                        "catalog_unavailable",
                        transient(self.retries, &job, now, "catalog_unavailable"),
                    ),
                };
                if let Err(repository_error) = self
                    .imports
                    .record_failure(&work, state, code, request_id, now)
                    .await
                {
                    return Err(self.repository_failure(repository_error, &job, now));
                }
                Err(failure)
            }
        }
    }

    async fn parser_failure(
        &self,
        work: &ImportWork,
        job: &LeasedJob,
        request_id: RequestId,
        now: OffsetDateTime,
        error: PublicationParserError,
    ) -> JobFailure {
        let (state, code, failure) = match error {
            PublicationParserError::EncryptedContent => (
                UploadState::Failed,
                "encrypted_epub_unsupported",
                permanent(
                    "encrypted_epub_unsupported",
                    "encrypted EPUB files are not supported",
                ),
            ),
            PublicationParserError::InvalidNavigation => (
                UploadState::Failed,
                "invalid_epub_navigation",
                permanent(
                    "invalid_epub_navigation",
                    "publication navigation is invalid",
                ),
            ),
            PublicationParserError::Malformed => (
                UploadState::Failed,
                "invalid_epub",
                permanent("invalid_epub", "publication is malformed"),
            ),
            PublicationParserError::Unavailable => (
                UploadState::RetryWait,
                "blob_io_unavailable",
                transient(self.retries, job, now, "blob_io_unavailable"),
            ),
            PublicationParserError::Configuration => {
                return operator(
                    "parser_configuration_invalid",
                    "parser configuration requires operator action",
                );
            }
            PublicationParserError::Capacity => {
                return operator(
                    "storage_capacity_exhausted",
                    "storage requires operator action",
                );
            }
        };
        if let Err(repository_error) = self
            .imports
            .record_failure(work, state, code, request_id, now)
            .await
        {
            return self.repository_failure(repository_error, job, now);
        }
        failure
    }

    fn repository_failure(
        &self,
        error: ImportRepositoryError,
        job: &LeasedJob,
        now: OffsetDateTime,
    ) -> JobFailure {
        match error {
            ImportRepositoryError::InvalidState => {
                permanent("import_state_invalid", "upload is not importable")
            }
            ImportRepositoryError::Unavailable => {
                transient(self.retries, job, now, "database_unavailable")
            }
            ImportRepositoryError::Schema => operator(
                "schema_incompatible",
                "database schema requires operator action",
            ),
        }
    }
}

fn reconciled_failure(code: &str) -> JobFailure {
    match code {
        "parser_configuration_invalid" | "storage_capacity_exhausted" | "schema_incompatible" => {
            JobFailure::OperatorRequired {
                code: match code {
                    "parser_configuration_invalid" => "parser_configuration_invalid",
                    "storage_capacity_exhausted" => "storage_capacity_exhausted",
                    _ => "schema_incompatible",
                },
                summary: "persisted import requires operator action".to_owned(),
            }
        }
        _ => JobFailure::Permanent {
            code: match code {
                "invalid_epub" => "invalid_epub",
                "catalog_import_invalid" => "catalog_import_invalid",
                _ => "import_failed",
            },
            summary: format!("persisted import failure: {code}"),
        },
    }
}

fn permanent(code: &'static str, summary: &str) -> JobFailure {
    JobFailure::Permanent {
        code,
        summary: summary.to_owned(),
    }
}

fn operator(code: &'static str, summary: &str) -> JobFailure {
    JobFailure::OperatorRequired {
        code,
        summary: summary.to_owned(),
    }
}

fn transient(
    retries: RetrySchedule,
    job: &LeasedJob,
    now: OffsetDateTime,
    code: &'static str,
) -> JobFailure {
    JobFailure::Transient {
        code,
        retry_at: retries.next(now, job.job_id, job.attempt),
    }
}
