CREATE TABLE folioharbor.system_administrators (
    user_id uuid PRIMARY KEY
        REFERENCES folioharbor.user_accounts(user_id) ON DELETE RESTRICT,
    created_at timestamptz NOT NULL
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

ALTER FUNCTION folioharbor.operations_health() OWNER TO folioharbor_owner;
ALTER FUNCTION folioharbor.operations_bootstrap_admin(
    uuid, text, text, text, timestamptz
) OWNER TO folioharbor_owner;

REVOKE ALL ON TABLE folioharbor.system_administrators FROM PUBLIC;
REVOKE ALL ON FUNCTION folioharbor.operations_health() FROM PUBLIC;
REVOKE ALL ON FUNCTION folioharbor.operations_bootstrap_admin(
    uuid, text, text, text, timestamptz
) FROM PUBLIC;

GRANT EXECUTE ON FUNCTION folioharbor.operations_health() TO folioharbor_api;

UPDATE folioharbor.schema_metadata
   SET schema_version = 27, applied_at = clock_timestamp()
 WHERE singleton;
