use std::sync::Arc;

use async_trait::async_trait;
use folioharbor_application::imports::{JobFailure, ProcessImportJob};
use folioharbor_domain::imports::job::{JobKind, LeasedJob};

use crate::runner::JobDispatcher;

pub struct WorkerHandlers {
    imports: Arc<ProcessImportJob>,
}

impl WorkerHandlers {
    #[must_use]
    pub const fn new(imports: Arc<ProcessImportJob>) -> Self {
        Self { imports }
    }
}

#[async_trait]
impl JobDispatcher for WorkerHandlers {
    async fn dispatch(&self, job: LeasedJob) -> Result<(), JobFailure> {
        match job.kind {
            JobKind::ImportEpub => self.imports.execute(job).await.map(|_| ()),
        }
    }
}
