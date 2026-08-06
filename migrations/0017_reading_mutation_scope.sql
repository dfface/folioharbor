ALTER TABLE folioharbor.reading_mutations
    ADD COLUMN request_fingerprint bytea CHECK (
      request_fingerprint IS NULL OR octet_length(request_fingerprint) = 32
    );

ALTER TABLE folioharbor.reading_mutations
    ALTER COLUMN global_locator DROP NOT NULL,
    ALTER COLUMN global_updated_at DROP NOT NULL;

ALTER TABLE folioharbor.reading_mutations
    DROP CONSTRAINT reading_mutations_global_version_check;
ALTER TABLE folioharbor.reading_mutations
    ADD CONSTRAINT reading_mutations_global_version_check CHECK (global_version >= 0);

UPDATE folioharbor.schema_metadata
SET schema_version = 17, applied_at = clock_timestamp()
WHERE singleton;
