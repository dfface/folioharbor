#![forbid(unsafe_code)]

use std::env;

use secrecy::SecretString;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    match env::args().nth(1).as_deref() {
        Some("migrate") => migrate().await,
        _ => anyhow::bail!("usage: folioharbor migrate"),
    }
}

async fn migrate() -> anyhow::Result<()> {
    let url = SecretString::from(env::var("FOLIOHARBOR_DATABASE_URL")?.into_boxed_str());
    let pool = folioharbor_postgres::connect_owner(&url).await?;
    let report = folioharbor_postgres::run_migrations(&pool).await?;
    for version in report.versions {
        println!("schema version {version}");
    }
    pool.close().await;
    Ok(())
}
