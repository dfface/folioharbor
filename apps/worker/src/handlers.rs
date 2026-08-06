use std::sync::Arc;

use async_trait::async_trait;
use folioharbor_application::imports::{CleanupImports, JobFailure, ProcessImportJob};
use folioharbor_domain::imports::job::{JobKind, LeasedJob};
use time::{Duration, OffsetDateTime};

use crate::runner::JobDispatcher;

pub struct WorkerHandlers {
    imports: Arc<ProcessImportJob>,
    cleanup: Option<Arc<CleanupImports>>,
}

impl WorkerHandlers {
    #[must_use]
    pub const fn new(imports: Arc<ProcessImportJob>) -> Self {
        Self {
            imports,
            cleanup: None,
        }
    }

    #[must_use]
    pub fn with_cleanup(imports: Arc<ProcessImportJob>, cleanup: Arc<CleanupImports>) -> Self {
        Self {
            imports,
            cleanup: Some(cleanup),
        }
    }
}

#[async_trait]
impl JobDispatcher for WorkerHandlers {
    async fn dispatch(&self, job: LeasedJob) -> Result<(), JobFailure> {
        match job.kind {
            JobKind::ImportEpub => self.imports.execute(job).await.map(|_| ()),
            kind @ (JobKind::ExpireUploadsAndReservations
            | JobKind::PurgeFailedUploads
            | JobKind::CollectBlobsLater) => {
                let cleanup =
                    self.cleanup
                        .as_ref()
                        .ok_or_else(|| JobFailure::OperatorRequired {
                            code: "cleanup_not_configured",
                            summary: "cleanup handler is not configured".to_owned(),
                        })?;
                cleanup
                    .run_kind(
                        &format!("cleanup-{}", job.job_id.as_uuid()),
                        kind,
                        OffsetDateTime::now_utc(),
                        100,
                    )
                    .await
                    .map(|_| ())
                    .map_err(|_| JobFailure::Transient {
                        code: "cleanup_unavailable",
                        retry_at: OffsetDateTime::now_utc() + Duration::minutes(1),
                    })
            }
        }
    }
}
