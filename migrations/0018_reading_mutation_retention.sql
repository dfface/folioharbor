ALTER TABLE folioharbor.reading_mutations
    ADD CONSTRAINT reading_mutations_global_snapshot_consistent CHECK (
      (
        global_version = 0
        AND outcome = 'conflict'
        AND global_package_id IS NULL
        AND global_content_unit_id IS NULL
        AND global_locator IS NULL
        AND global_updated_at IS NULL
      )
      OR
      (
        global_version > 0
        AND global_locator IS NOT NULL
        AND global_updated_at IS NOT NULL
      )
    );

CREATE INDEX reading_mutations_user_created_idx
    ON folioharbor.reading_mutations(user_id, created_at);

-- Upgrade safety: establish the same bounded retained set before counters are backfilled.
DELETE FROM folioharbor.reading_mutations
WHERE created_at < clock_timestamp() - interval '30 days';

WITH ranked AS (
    SELECT user_id,
           client_mutation_id,
           row_number() OVER (
             PARTITION BY user_id
             ORDER BY created_at DESC, client_mutation_id DESC
           ) AS retained_rank
    FROM folioharbor.reading_mutations
)
DELETE FROM folioharbor.reading_mutations mutation
USING ranked
WHERE mutation.user_id = ranked.user_id
  AND mutation.client_mutation_id = ranked.client_mutation_id
  AND ranked.retained_rank > 10000;

WITH sized AS (
    SELECT user_id,
           client_mutation_id,
           sum(
             pg_column_size(mutation)
             + COALESCE(pg_column_size(global_locator), 0)
             + pg_column_size(device_locator)
           ) OVER (
             PARTITION BY user_id
             ORDER BY created_at DESC, client_mutation_id DESC
           ) AS retained_bytes
    FROM folioharbor.reading_mutations mutation
)
DELETE FROM folioharbor.reading_mutations mutation
USING sized
WHERE mutation.user_id = sized.user_id
  AND mutation.client_mutation_id = sized.client_mutation_id
  AND sized.retained_bytes > 67108864;

CREATE TABLE folioharbor.reading_mutation_usage (
    user_id uuid PRIMARY KEY REFERENCES folioharbor.user_accounts(user_id) ON DELETE CASCADE,
    live_count bigint NOT NULL CHECK (live_count >= 0 AND live_count <= 10000),
    live_bytes bigint NOT NULL CHECK (live_bytes >= 0 AND live_bytes <= 67108864),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp()
);

INSERT INTO folioharbor.reading_mutation_usage(user_id, live_count, live_bytes)
SELECT user_id,
       count(*)::bigint,
       sum(
         pg_column_size(mutation)
         + COALESCE(pg_column_size(global_locator), 0)
         + pg_column_size(device_locator)
       )::bigint
FROM folioharbor.reading_mutations mutation
GROUP BY user_id;

ALTER TABLE folioharbor.reading_mutation_usage ENABLE ROW LEVEL SECURITY;
ALTER TABLE folioharbor.reading_mutation_usage FORCE ROW LEVEL SECURITY;
CREATE POLICY reading_mutation_usage_owner_access
    ON folioharbor.reading_mutation_usage
    USING (current_user = 'folioharbor_owner')
    WITH CHECK (current_user = 'folioharbor_owner');
CREATE POLICY reading_mutation_usage_user_access
    ON folioharbor.reading_mutation_usage
    USING (user_id = folioharbor.current_user_id())
    WITH CHECK (user_id = folioharbor.current_user_id());

GRANT SELECT, INSERT, UPDATE ON folioharbor.reading_mutation_usage TO folioharbor_api;
GRANT DELETE ON folioharbor.reading_mutations TO folioharbor_api;

UPDATE folioharbor.schema_metadata
SET schema_version = 18, applied_at = clock_timestamp()
WHERE singleton;
