use std::collections::BTreeMap;

use folioharbor_application::config::{ConfigSources, StorageSettings};
use folioharbor_application::operations::ConsistencyCheck;
use folioharbor_storage_local::LocalBlobStore;

/// Compares database Blob lifecycle ownership with bytes on disk and verifies
/// integrity for ready locations.
///
/// # Errors
///
/// Returns an error for invalid configuration, unavailable dependencies, or any discrepancy.
pub async fn run() -> anyhow::Result<()> {
    let storage_root = storage_root_from_environment(std::env::vars().collect())?;
    let url = crate::migrate::owner_database_url()?;
    let pool = folioharbor_postgres::connect_owner(&url).await?;
    let repository = folioharbor_postgres::PgOperationsRepository::new(pool.clone());
    let blobs = LocalBlobStore::new(storage_root);
    let report = ConsistencyCheck::new(&repository, &blobs).execute().await?;
    println!(
        "checked={} missing_blobs={} orphan_locations={} hash_mismatches={}",
        report.checked, report.missing_blobs, report.orphan_locations, report.hash_mismatches
    );
    pool.close().await;
    if !report.is_clean() {
        anyhow::bail!("storage consistency check found discrepancies");
    }
    Ok(())
}

/// Resolves the storage-check root through the same defaults, source mapping,
/// and validation used by the long-running services.
///
/// # Errors
///
/// Returns an error when storage configuration is invalid or uses a removed key.
pub fn storage_root_from_environment(
    environment: BTreeMap<String, String>,
) -> anyhow::Result<std::path::PathBuf> {
    Ok(StorageSettings::load(ConfigSources {
        environment,
        ..ConfigSources::default()
    })?
    .root)
}
