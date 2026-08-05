ALTER TABLE folioharbor.libraries
    ADD COLUMN quota_limit_bytes bigint NOT NULL DEFAULT 5368709120 CHECK (quota_limit_bytes >= 0),
    ADD CONSTRAINT libraries_quota_within_limit CHECK (
        quota_used_bytes <= quota_limit_bytes - quota_reserved_bytes
    );

CREATE TABLE folioharbor.blobs (
    blob_id uuid PRIMARY KEY,
    storage_namespace text NOT NULL CHECK (
        storage_namespace ~ '^[a-z0-9][a-z0-9-]{0,127}$'
    ),
    sha256 bytea NOT NULL CHECK (octet_length(sha256) = 32),
    byte_size bigint NOT NULL CHECK (byte_size >= 0),
    created_at timestamptz NOT NULL,
    UNIQUE (storage_namespace, sha256, byte_size)
);

CREATE TABLE folioharbor.blob_locations (
    blob_id uuid NOT NULL REFERENCES folioharbor.blobs(blob_id) ON DELETE CASCADE,
    storage_key text NOT NULL UNIQUE CHECK (length(storage_key) BETWEEN 1 AND 512),
    state text NOT NULL CHECK (state IN ('staging', 'ready', 'quarantined', 'purged')),
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    PRIMARY KEY (blob_id, storage_key)
);

CREATE TABLE folioharbor.quota_reservations (
    upload_id uuid PRIMARY KEY,
    library_id uuid NOT NULL REFERENCES folioharbor.libraries(library_id) ON DELETE CASCADE,
    reserved_bytes bigint NOT NULL CHECK (reserved_bytes >= 0),
    expires_at timestamptz NOT NULL,
    state text NOT NULL CHECK (state IN ('active', 'consumed', 'released')),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp()
);
CREATE INDEX quota_reservations_library_active
    ON folioharbor.quota_reservations(library_id, expires_at)
    WHERE state = 'active';

ALTER TABLE folioharbor.quota_reservations ENABLE ROW LEVEL SECURITY;
ALTER TABLE folioharbor.quota_reservations FORCE ROW LEVEL SECURITY;
CREATE POLICY quota_reservations_owner_access ON folioharbor.quota_reservations
    USING (current_user = 'folioharbor_owner')
    WITH CHECK (current_user = 'folioharbor_owner');
CREATE POLICY quota_reservations_runtime_read ON folioharbor.quota_reservations FOR SELECT
    USING (
        library_id = folioharbor.current_library_id()
        AND (
            folioharbor.is_worker()
            OR EXISTS (
                SELECT 1 FROM folioharbor.library_memberships membership
                WHERE membership.library_id = quota_reservations.library_id
                  AND membership.user_id = folioharbor.current_user_id()
                  AND membership.status = 'active'
            )
        )
    );
GRANT SELECT ON folioharbor.quota_reservations TO folioharbor_api, folioharbor_worker;

CREATE FUNCTION folioharbor.quota_reserve(
    p_library uuid, p_upload uuid, p_bytes bigint, p_expires timestamptz
) RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE library_row folioharbor.libraries%ROWTYPE;
BEGIN
    IF NOT folioharbor.is_worker() OR p_library IS DISTINCT FROM folioharbor.current_library_id()
       OR p_bytes < 0 THEN
        RETURN 'not_active';
    END IF;
    SELECT * INTO library_row FROM folioharbor.libraries
      WHERE library_id = p_library FOR UPDATE;
    IF library_row.library_id IS NULL THEN RETURN 'not_active'; END IF;
    IF EXISTS (SELECT 1 FROM folioharbor.quota_reservations WHERE upload_id = p_upload) THEN
        RETURN 'not_active';
    END IF;
    IF p_bytes > library_row.quota_limit_bytes - library_row.quota_used_bytes - library_row.quota_reserved_bytes THEN
        RETURN 'exceeded';
    END IF;
    INSERT INTO folioharbor.quota_reservations(upload_id, library_id, reserved_bytes, expires_at, state)
      VALUES (p_upload, p_library, p_bytes, p_expires, 'active');
    UPDATE folioharbor.libraries SET quota_reserved_bytes = quota_reserved_bytes + p_bytes
      WHERE library_id = p_library;
    RETURN 'applied';
END $$;

CREATE FUNCTION folioharbor.quota_resize(
    p_library uuid, p_upload uuid, p_bytes bigint
) RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE library_row folioharbor.libraries%ROWTYPE;
DECLARE reservation folioharbor.quota_reservations%ROWTYPE;
BEGIN
    IF NOT folioharbor.is_worker() OR p_library IS DISTINCT FROM folioharbor.current_library_id()
       OR p_bytes < 0 THEN RETURN 'not_active'; END IF;
    SELECT * INTO library_row FROM folioharbor.libraries WHERE library_id = p_library FOR UPDATE;
    SELECT * INTO reservation FROM folioharbor.quota_reservations
      WHERE upload_id = p_upload AND library_id = p_library AND state = 'active' FOR UPDATE;
    IF reservation.upload_id IS NULL THEN RETURN 'not_active'; END IF;
    IF p_bytes > library_row.quota_limit_bytes - library_row.quota_used_bytes
       - library_row.quota_reserved_bytes + reservation.reserved_bytes THEN RETURN 'exceeded'; END IF;
    UPDATE folioharbor.libraries
      SET quota_reserved_bytes = quota_reserved_bytes - reservation.reserved_bytes + p_bytes
      WHERE library_id = p_library;
    UPDATE folioharbor.quota_reservations
      SET reserved_bytes = p_bytes, updated_at = clock_timestamp() WHERE upload_id = p_upload;
    RETURN 'applied';
END $$;

CREATE FUNCTION folioharbor.quota_consume(p_library uuid, p_upload uuid)
RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE reservation folioharbor.quota_reservations%ROWTYPE;
BEGIN
    IF NOT folioharbor.is_worker() OR p_library IS DISTINCT FROM folioharbor.current_library_id() THEN
      RETURN 'not_active'; END IF;
    PERFORM 1 FROM folioharbor.libraries WHERE library_id = p_library FOR UPDATE;
    SELECT * INTO reservation FROM folioharbor.quota_reservations
      WHERE upload_id = p_upload AND library_id = p_library AND state = 'active' FOR UPDATE;
    IF reservation.upload_id IS NULL THEN RETURN 'not_active'; END IF;
    UPDATE folioharbor.libraries SET
      quota_reserved_bytes = quota_reserved_bytes - reservation.reserved_bytes,
      quota_used_bytes = quota_used_bytes + reservation.reserved_bytes
      WHERE library_id = p_library;
    UPDATE folioharbor.quota_reservations SET state = 'consumed', updated_at = clock_timestamp()
      WHERE upload_id = p_upload;
    RETURN 'applied';
END $$;

CREATE FUNCTION folioharbor.quota_release(p_library uuid, p_upload uuid)
RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE reservation folioharbor.quota_reservations%ROWTYPE;
BEGIN
    IF NOT folioharbor.is_worker() OR p_library IS DISTINCT FROM folioharbor.current_library_id() THEN
      RETURN 'not_active'; END IF;
    PERFORM 1 FROM folioharbor.libraries WHERE library_id = p_library FOR UPDATE;
    SELECT * INTO reservation FROM folioharbor.quota_reservations
      WHERE upload_id = p_upload AND library_id = p_library AND state = 'active' FOR UPDATE;
    IF reservation.upload_id IS NULL THEN RETURN 'not_active'; END IF;
    UPDATE folioharbor.libraries SET quota_reserved_bytes = quota_reserved_bytes - reservation.reserved_bytes
      WHERE library_id = p_library;
    UPDATE folioharbor.quota_reservations SET state = 'released', updated_at = clock_timestamp()
      WHERE upload_id = p_upload;
    RETURN 'applied';
END $$;

ALTER FUNCTION folioharbor.quota_reserve(uuid,uuid,bigint,timestamptz) OWNER TO folioharbor_owner;
ALTER FUNCTION folioharbor.quota_resize(uuid,uuid,bigint) OWNER TO folioharbor_owner;
ALTER FUNCTION folioharbor.quota_consume(uuid,uuid) OWNER TO folioharbor_owner;
ALTER FUNCTION folioharbor.quota_release(uuid,uuid) OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.quota_reserve(uuid,uuid,bigint,timestamptz) FROM PUBLIC;
REVOKE ALL ON FUNCTION folioharbor.quota_resize(uuid,uuid,bigint) FROM PUBLIC;
REVOKE ALL ON FUNCTION folioharbor.quota_consume(uuid,uuid) FROM PUBLIC;
REVOKE ALL ON FUNCTION folioharbor.quota_release(uuid,uuid) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.quota_reserve(uuid,uuid,bigint,timestamptz),
 folioharbor.quota_resize(uuid,uuid,bigint),folioharbor.quota_consume(uuid,uuid),
 folioharbor.quota_release(uuid,uuid) TO folioharbor_worker;

UPDATE folioharbor.schema_metadata SET schema_version = 8, applied_at = clock_timestamp()
WHERE singleton;
