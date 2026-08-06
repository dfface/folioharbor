ALTER TABLE folioharbor.reading_mutations
    ADD COLUMN accounted_bytes bigint;

UPDATE folioharbor.reading_mutations SET accounted_bytes = 0;
UPDATE folioharbor.reading_mutations mutation
SET accounted_bytes = pg_column_size(mutation)
    + COALESCE(pg_column_size(global_locator), 0)
    + pg_column_size(device_locator);
ALTER TABLE folioharbor.reading_mutations
    ALTER COLUMN accounted_bytes SET DEFAULT 1,
    ALTER COLUMN accounted_bytes SET NOT NULL,
    ADD CONSTRAINT reading_mutations_accounted_bytes_positive CHECK (accounted_bytes > 0);

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

WITH ranked AS (
    SELECT user_id,
           client_mutation_id,
           sum(accounted_bytes) OVER (
               PARTITION BY user_id
               ORDER BY created_at DESC, client_mutation_id DESC
           ) AS retained_bytes
    FROM folioharbor.reading_mutations
)
DELETE FROM folioharbor.reading_mutations mutation
USING ranked
WHERE mutation.user_id = ranked.user_id
  AND mutation.client_mutation_id = ranked.client_mutation_id
  AND ranked.retained_bytes > 67108864;

UPDATE folioharbor.reading_mutation_usage
SET live_count = 0,
    live_bytes = 0,
    updated_at = clock_timestamp();

INSERT INTO folioharbor.reading_mutation_usage(user_id, live_count, live_bytes, updated_at)
SELECT user_id, live_count, live_bytes, clock_timestamp()
FROM (
    SELECT user_id,
           count(*)::bigint AS live_count,
           sum(accounted_bytes)::bigint AS live_bytes
    FROM folioharbor.reading_mutations
    GROUP BY user_id
) actual
ON CONFLICT(user_id) DO UPDATE
SET live_count = EXCLUDED.live_count,
    live_bytes = EXCLUDED.live_bytes,
    updated_at = EXCLUDED.updated_at;

CREATE FUNCTION folioharbor.reading_mutation_reject_update()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO ''
AS $$
BEGIN
    IF pg_trigger_depth() > 1
       AND (to_jsonb(NEW) - 'accounted_bytes') = (to_jsonb(OLD) - 'accounted_bytes') THEN
        RETURN NEW;
    END IF;
    RAISE EXCEPTION USING
      ERRCODE = 'P0001',
      MESSAGE = 'reading mutations are immutable',
      CONSTRAINT = 'reading_mutation_immutable';
END
$$;

CREATE FUNCTION folioharbor.reading_mutation_account_insert()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO ''
AS $$
DECLARE mutation_bytes bigint;
DECLARE reserved_count bigint;
BEGIN
    SELECT pg_column_size(mutation)
           + COALESCE(pg_column_size(global_locator), 0)
           + pg_column_size(device_locator)
    INTO STRICT mutation_bytes
    FROM folioharbor.reading_mutations mutation
    WHERE user_id = NEW.user_id AND client_mutation_id = NEW.client_mutation_id;

    UPDATE folioharbor.reading_mutations
    SET accounted_bytes = mutation_bytes
    WHERE user_id = NEW.user_id AND client_mutation_id = NEW.client_mutation_id;

    INSERT INTO folioharbor.reading_mutation_usage(user_id, live_count, live_bytes)
    VALUES(NEW.user_id, 0, 0)
    ON CONFLICT(user_id) DO NOTHING;

    UPDATE folioharbor.reading_mutation_usage
    SET live_count = live_count + 1,
        live_bytes = live_bytes + mutation_bytes,
        updated_at = clock_timestamp()
    WHERE user_id = NEW.user_id
      AND live_count < 10000
      AND live_bytes <= 67108864 - mutation_bytes
    RETURNING live_count INTO reserved_count;

    IF reserved_count IS NULL THEN
        RAISE EXCEPTION USING
          ERRCODE = 'P0001',
          MESSAGE = 'reading mutation capacity exhausted',
          CONSTRAINT = 'reading_mutation_quota';
    END IF;
    RETURN NULL;
END
$$;

CREATE FUNCTION folioharbor.reading_mutation_account_delete()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO ''
AS $$
BEGIN
    UPDATE folioharbor.reading_mutation_usage
    SET live_count = live_count - 1,
        live_bytes = live_bytes - OLD.accounted_bytes,
        updated_at = clock_timestamp()
    WHERE user_id = OLD.user_id;
    RETURN NULL;
END
$$;

CREATE TRIGGER reading_mutations_reject_update
BEFORE UPDATE ON folioharbor.reading_mutations
FOR EACH ROW EXECUTE FUNCTION folioharbor.reading_mutation_reject_update();
CREATE TRIGGER reading_mutations_account_insert
AFTER INSERT ON folioharbor.reading_mutations
FOR EACH ROW EXECUTE FUNCTION folioharbor.reading_mutation_account_insert();
CREATE TRIGGER reading_mutations_account_delete
AFTER DELETE ON folioharbor.reading_mutations
FOR EACH ROW EXECUTE FUNCTION folioharbor.reading_mutation_account_delete();

ALTER FUNCTION folioharbor.reading_mutation_reject_update() OWNER TO folioharbor_owner;
ALTER FUNCTION folioharbor.reading_mutation_account_insert() OWNER TO folioharbor_owner;
ALTER FUNCTION folioharbor.reading_mutation_account_delete() OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.reading_mutation_reject_update() FROM PUBLIC;
REVOKE ALL ON FUNCTION folioharbor.reading_mutation_account_insert() FROM PUBLIC;
REVOKE ALL ON FUNCTION folioharbor.reading_mutation_account_delete() FROM PUBLIC;

REVOKE UPDATE ON folioharbor.reading_mutations FROM folioharbor_api;
REVOKE INSERT, UPDATE, DELETE ON folioharbor.reading_mutation_usage FROM folioharbor_api;

UPDATE folioharbor.schema_metadata
SET schema_version = 19, applied_at = clock_timestamp()
WHERE singleton;
