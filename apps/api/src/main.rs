#![forbid(unsafe_code)]

use async_trait::async_trait;
use folioharbor_application::{
    config::{ConfigSources, Settings},
    identity::IdentityApi,
    ports::{Argon2PasswordHasher, Clock, MailError, Mailer, RandomSource},
    rate_limit::DurableRateLimiter,
};
use folioharbor_domain::identity::NormalizedEmail;
use folioharbor_http::AppState;
use folioharbor_postgres::{PgRateLimitRepository, connect_api, identity::PgIdentityRepository};
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
struct DeferredMailer;
#[async_trait]
impl Mailer for DeferredMailer {
    async fn send_verification(
        &self,
        _: &NormalizedEmail,
        _: SecretString,
    ) -> Result<(), MailError> {
        Err(MailError)
    }
    async fn send_password_reset(
        &self,
        _: &NormalizedEmail,
        _: SecretString,
    ) -> Result<(), MailError> {
        Err(MailError)
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
    let identity = Arc::new(IdentityApi::new(
        PgIdentityRepository::new(pool.clone()),
        Argon2PasswordHasher::new(SystemRandom),
        DeferredMailer,
        SystemClock,
        SystemRandom,
    ));
    let secret = SecretString::from(
        settings
            .auth
            .application_secrets
            .current_for_encryption()
            .secret()
            .expose_secret()
            .to_owned(),
    );
    let limiter = Arc::new(DurableRateLimiter::new(
        PgRateLimitRepository::new(pool),
        secret,
        SystemClock,
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
    );
    let listener = tokio::net::TcpListener::bind(&settings.server.bind_address).await?;
    axum::serve(
        listener,
        folioharbor_http::router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
