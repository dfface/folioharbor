use std::{env, fs};

use secrecy::SecretString;

/// Applies all pending schema migrations with owner credentials.
///
/// # Errors
///
/// Returns an error for missing configuration, connection failures, or migration failures.
pub async fn run() -> anyhow::Result<()> {
    let url = owner_database_url()?;
    let pool = folioharbor_postgres::connect_owner(&url).await?;
    let report = folioharbor_postgres::run_migrations(&pool).await?;
    for version in report.versions {
        println!("schema version {version}");
    }
    pool.close().await;
    Ok(())
}

pub(crate) fn owner_database_url() -> anyhow::Result<SecretString> {
    let direct = env::var("FOLIOHARBOR_DATABASE_URL").ok();
    let file = env::var("FOLIOHARBOR_DATABASE_URL_FILE").ok();
    let value = match (direct, file) {
        (Some(_), Some(_)) => anyhow::bail!(
            "FOLIOHARBOR_DATABASE_URL and FOLIOHARBOR_DATABASE_URL_FILE are mutually exclusive"
        ),
        (Some(value), None) => value,
        (None, Some(path)) => fs::read_to_string(path)?
            .trim_end_matches(['\r', '\n'])
            .to_owned(),
        (None, None) => {
            anyhow::bail!("FOLIOHARBOR_DATABASE_URL or FOLIOHARBOR_DATABASE_URL_FILE is required")
        }
    };
    Ok(SecretString::from(value.into_boxed_str()))
}
