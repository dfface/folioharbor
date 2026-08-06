#![forbid(unsafe_code)]

use folioharbor_api::{
    build_catalog_api, build_download, build_progress_api, build_reader_api, build_upload_api,
};
use folioharbor_application::{
    config::{ConfigSources, Settings},
    identity::IdentityApi,
    libraries::LibraryService,
    mail::MailOutbox,
    ports::{Argon2PasswordHasher, Clock, RandomSource},
    rate_limit::DurableRateLimiter,
};
use folioharbor_http::AppState;
use folioharbor_postgres::{
    PgAuditRepository, PgAuthorizationRepository, PgRateLimitRepository, connect_api,
    identity::PgIdentityRepository, libraries::PgLibraryRepository,
};
use secrecy::{ExposeSecret as _, SecretString};
use std::{collections::BTreeMap, net::SocketAddr, sync::Arc};

#[derive(Clone, Copy)]
struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> folioharbor_domain::time::OffsetDateTime {
        folioharbor_domain::time::OffsetDateTime::now_utc()
    }
}
#[derive(Clone, Copy)]
struct SystemRandom;
impl RandomSource for SystemRandom {
    fn fill(&self, destination: &mut [u8]) {
        if getrandom::fill(destination).is_err() {
            std::process::abort();
        }
    }
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let settings = Settings::load(ConfigSources {
        environment: std::env::vars().collect::<BTreeMap<_, _>>(),
        ..ConfigSources::default()
    })?;
    let database_url = settings
        .database
        .url
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("FOLIOHARBOR_DATABASE_URL is required"))?;
    let pool = connect_api(database_url).await?;
    let upload_api = build_upload_api(&settings, pool.clone());
    let catalog_api = build_catalog_api(&settings, pool.clone());
    let reader_api = build_reader_api(&settings, pool.clone());
    let progress_api = build_progress_api(pool.clone());
    let (download_api, download_blobs) = build_download(&settings, pool.clone());
    let library_repository = PgLibraryRepository::new(pool.clone());
    let personal_library_enabled = settings.auth.personal_library_enabled;
    let secret = SecretString::from(
        settings
            .auth
            .application_secrets
            .current_for_encryption()
            .secret()
            .expose_secret()
            .to_owned(),
    );
    let mail_outbox = MailOutbox::new(Arc::new(settings.auth.application_secrets));
    let identity = Arc::new(IdentityApi::new_configured(
        PgIdentityRepository::new(pool.clone()),
        Argon2PasswordHasher::new(SystemRandom),
        mail_outbox.clone(),
        SystemClock,
        SystemRandom,
        personal_library_enabled,
        library_repository.clone(),
    ));
    let limiter = Arc::new(DurableRateLimiter::new(
        PgRateLimitRepository::new(pool.clone()),
        secret,
        SystemClock,
    ));
    let library_api = Arc::new(LibraryService::new(
        library_repository,
        PgAuthorizationRepository::new(pool.clone()),
        PgAuditRepository::new(pool),
        mail_outbox,
        SystemClock,
        SystemRandom,
    ));
    let state = AppState::new(
        settings.server.public_base_url.as_url().clone(),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity.clone(),
        identity,
        limiter,
    )
    .with_library_api(library_api)
    .with_upload_api(upload_api)
    .with_catalog_api(catalog_api);
    let state = state
        .with_reader_api(reader_api)
        .with_progress_api(progress_api)
        .with_download(download_api, download_blobs);
    let listener = tokio::net::TcpListener::bind(&settings.server.bind_address).await?;
    axum::serve(
        listener,
        folioharbor_http::router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
