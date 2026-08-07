# Backup and restore

PostgreSQL and the complete Blob/staging volume are one business backup set. A database dump without its matching volume (or a volume archive without its matching database) is not a FolioHarbor backup.

## Capture

1. Record the release/image identifier and `SELECT schema_version, applied_at FROM folioharbor.schema_metadata WHERE singleton`.
2. Record a Blob watermark from `SELECT count(*), max(updated_at) FROM folioharbor.blob_locations` and identify the volume snapshot/archive in the same manifest.
3. Quiesce API and Worker writes, wait for in-flight work to stop, then capture PostgreSQL and the Blob/staging volume as one coordinated set.
4. Encrypt, checksum, copy off-host, and periodically restore-test the set.

FolioHarbor does not claim crash-consistent cross-volume snapshots. If the storage platform cannot provide an atomic snapshot spanning PostgreSQL and Blob storage, the operator must quiesce writes and coordinate the two captures. Do not infer consistency from close timestamps alone.

## Restore

1. Keep API and Worker stopped and provision empty replacement PostgreSQL and Blob storage with the documented ownership/modes.
2. Restore PostgreSQL first, including the recorded schema metadata, then restore the matching Blob/staging archive before allowing any runtime process to start.
3. Confirm the restored schema version matches the application binary. Do not silently migrate a restore until its original matching set has been validated.
4. Run `folioharbor storage check` with owner credentials and the restored storage configuration. It reports safe counts for missing Blobs, orphan/invalid locations, and hash/size mismatches without printing storage keys or hashes.
5. Compare the recorded Blob watermark and checksums, resolve every discrepancy, then start API and Worker and verify readiness plus a representative authorized read.

Keep the original backup immutable until application-level verification succeeds. A non-clean storage check is an incident, not permission to delete either side of the mismatch.
