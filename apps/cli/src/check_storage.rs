use folioharbor_application::operations::ConsistencyCheck;
use folioharbor_storage_local::LocalBlobStore;

/// Compares ready database Blob locations with their bytes on disk.
///
/// # Errors
///
/// Returns an error for invalid configuration, unavailable dependencies, or any discrepancy.
pub async fn run() -> anyhow::Result<()> {
    let storage_root = std::env::var_os("FOLIOHARBOR_STORAGE_ROOT").map_or_else(
        || std::path::PathBuf::from("/var/lib/folioharbor/blobs"),
        std::path::PathBuf::from,
    );
    if !storage_root.is_absolute() || storage_root.parent().is_none() {
        anyhow::bail!("FOLIOHARBOR_STORAGE_ROOT must be an absolute non-root path");
    }
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
