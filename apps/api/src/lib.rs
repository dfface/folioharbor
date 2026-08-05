#![forbid(unsafe_code)]

use folioharbor_application::{
    config::Settings,
    imports::{UploadApi, UploadService},
    ports::Clock,
};
use folioharbor_postgres::{PgAuthorizationRepository, PgUploadRepository};
use folioharbor_storage_local::LocalBlobStore;
use sqlx::PgPool;
use std::sync::Arc;

#[derive(Clone, Copy)]
struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> folioharbor_domain::time::OffsetDateTime {
        folioharbor_domain::time::OffsetDateTime::now_utc()
    }
}

#[must_use]
pub fn build_upload_api(settings: &Settings, pool: PgPool) -> Arc<dyn UploadApi> {
    Arc::new(UploadService::new(
        Arc::new(PgUploadRepository::new(pool.clone())),
        Arc::new(PgAuthorizationRepository::new(pool)),
        Arc::new(LocalBlobStore::new(settings.storage.root.clone())),
        Arc::new(SystemClock),
    ))
}
