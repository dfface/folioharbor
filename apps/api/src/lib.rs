#![forbid(unsafe_code)]

use folioharbor_application::{
    catalog::{CatalogApi, CatalogService},
    config::Settings,
    imports::{UploadApi, UploadService},
    ports::Clock,
    reader::{ReaderApi, ReaderService},
};
use folioharbor_domain::imports::blob::DedupScope as DomainDedupScope;
use folioharbor_epub::{EpubResourceReader, ResourceCacheLimits};
use folioharbor_postgres::{
    PgAuthorizationRepository, PgCatalogRepository, PgReaderCatalogRepository, PgUploadRepository,
};
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
        match settings.storage.dedup_scope {
            folioharbor_application::config::DedupScope::Instance => DomainDedupScope::Instance,
            folioharbor_application::config::DedupScope::Library => DomainDedupScope::Library,
            folioharbor_application::config::DedupScope::Disabled => DomainDedupScope::Disabled,
        },
    ))
}

#[must_use]
pub fn build_catalog_api(settings: &Settings, pool: PgPool) -> Arc<dyn CatalogApi> {
    Arc::new(CatalogService::new(
        PgCatalogRepository::new(pool.clone()),
        PgAuthorizationRepository::new(pool),
        settings.auth.reader_download_enabled,
    ))
}

#[must_use]
pub fn build_reader_api(settings: &Settings, pool: PgPool) -> Arc<dyn ReaderApi> {
    let blobs = Arc::new(LocalBlobStore::new(settings.storage.root.clone()));
    Arc::new(ReaderService::new(
        PgReaderCatalogRepository::new(pool),
        EpubResourceReader::new(blobs, ResourceCacheLimits::default()),
    ))
}
