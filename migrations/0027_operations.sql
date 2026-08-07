CREATE TABLE folioharbor.system_administrators (
    user_id uuid PRIMARY KEY
        REFERENCES folioharbor.user_accounts(user_id) ON DELETE RESTRICT,
    created_at timestamptz NOT NULL
);

ALTER TABLE folioharbor.background_jobs
    ADD COLUMN origin_request_id text
        CHECK (
            origin_request_id IS NULL
            OR origin_request_id ~ '^[0-9A-HJKMNP-TV-Z]{26}$'
        ),
    ADD COLUMN origin_traceparent text
        CHECK (
            origin_traceparent IS NULL
            OR (
                origin_traceparent ~ '^00-[0-9a-f]{32}-[0-9a-f]{16}-[0-9a-f]{2}$'
                AND substring(origin_traceparent FROM 4 FOR 32) <> repeat('0', 32)
                AND substring(origin_traceparent FROM 37 FOR 16) <> repeat('0', 16)
            )
        );

COMMENT ON TABLE folioharbor.system_administrators IS
    'Instance administration only; this grants no library membership or content permission.';

CREATE FUNCTION folioharbor.operations_health()
RETURNS TABLE(schema_version bigint, system_administrator_exists boolean)
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path TO '' AS $$
SELECT metadata.schema_version,
    EXISTS (SELECT 1 FROM folioharbor.system_administrators)
FROM folioharbor.schema_metadata AS metadata
WHERE metadata.singleton
$$;

CREATE FUNCTION folioharbor.operations_bootstrap_admin(
    p_user_id uuid,
    p_normalized_email text,
    p_display_email text,
    p_password_hash text,
    p_created_at timestamptz
) RETURNS text
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO '' AS $$
DECLARE
    existing_user_id uuid;
BEGIN
    PERFORM pg_catalog.pg_advisory_xact_lock(
        pg_catalog.hashtextextended(p_normalized_email, 0)
    );
    SELECT account.user_id INTO existing_user_id
      FROM folioharbor.user_accounts AS account
     WHERE account.normalized_email = p_normalized_email
     FOR UPDATE;

    IF existing_user_id IS NOT NULL THEN
        IF EXISTS (
            SELECT 1 FROM folioharbor.system_administrators AS administrator
             WHERE administrator.user_id = existing_user_id
        ) THEN
            RETURN 'already_administrator';
        END IF;
        RETURN 'email_in_use';
    END IF;

    INSERT INTO folioharbor.user_accounts (
        user_id, normalized_email, display_email, status, created_at, verified_at
    ) VALUES (
        p_user_id, p_normalized_email, p_display_email, 'verified', p_created_at, p_created_at
    );
    INSERT INTO folioharbor.password_credentials (
        user_id, password_hash, created_at, changed_at
    ) VALUES (
        p_user_id, p_password_hash, p_created_at, p_created_at
    );
    INSERT INTO folioharbor.system_administrators (user_id, created_at)
    VALUES (p_user_id, p_created_at);
    RETURN 'created';
END $$;

CREATE FUNCTION folioharbor.job_attach_upload_origin_authorized(
    p_upload uuid,
    p_library uuid,
    p_actor uuid,
    p_request text,
    p_traceparent text
) RETURNS uuid
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path TO '' AS $$
DECLARE
    attached_job uuid;
BEGIN
    IF session_user <> 'folioharbor_api'
        OR p_actor IS DISTINCT FROM folioharbor.current_user_id()
        OR p_library IS DISTINCT FROM folioharbor.current_library_id()
        OR p_request IS DISTINCT FROM folioharbor.current_request_id()
        OR p_request !~ '^[0-9A-HJKMNP-TV-Z]{26}$'
        OR (p_traceparent IS NOT NULL AND (
            p_traceparent !~ '^00-[0-9a-f]{32}-[0-9a-f]{16}-[0-9a-f]{2}$'
            OR substring(p_traceparent FROM 4 FOR 32) = repeat('0', 32)
            OR substring(p_traceparent FROM 37 FOR 16) = repeat('0', 16)
        ))
    THEN
        RETURN NULL;
    END IF;

    UPDATE folioharbor.background_jobs
       SET origin_request_id = COALESCE(origin_request_id, p_request),
           origin_traceparent = CASE
               WHEN origin_request_id IS NULL THEN p_traceparent
               ELSE origin_traceparent
           END
     WHERE library_id = p_library
       AND kind = 'import_epub'
       AND idempotency_key = 'import:' || p_upload::text
       AND input->>'upload_id' = p_upload::text
    RETURNING job_id INTO attached_job;
    RETURN attached_job;
END $$;

ALTER FUNCTION folioharbor.operations_health() OWNER TO folioharbor_owner;
ALTER FUNCTION folioharbor.operations_bootstrap_admin(
    uuid, text, text, text, timestamptz
) OWNER TO folioharbor_owner;
ALTER FUNCTION folioharbor.job_attach_upload_origin_authorized(
    uuid, uuid, uuid, text, text
) OWNER TO folioharbor_owner;

REVOKE ALL ON TABLE folioharbor.system_administrators FROM PUBLIC;
REVOKE ALL ON FUNCTION folioharbor.operations_health() FROM PUBLIC;
REVOKE ALL ON FUNCTION folioharbor.operations_bootstrap_admin(
    uuid, text, text, text, timestamptz
) FROM PUBLIC;
REVOKE ALL ON FUNCTION folioharbor.job_attach_upload_origin_authorized(
    uuid, uuid, uuid, text, text
) FROM PUBLIC;

GRANT EXECUTE ON FUNCTION folioharbor.operations_health() TO folioharbor_api;
GRANT EXECUTE ON FUNCTION folioharbor.job_attach_upload_origin_authorized(
    uuid, uuid, uuid, text, text
) TO folioharbor_api;

UPDATE folioharbor.schema_metadata
   SET schema_version = 27, applied_at = clock_timestamp()
 WHERE singleton;
