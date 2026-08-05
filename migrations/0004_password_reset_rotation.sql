DROP FUNCTION folioharbor.identity_reset_password(bytea, text, timestamptz, text);

CREATE FUNCTION folioharbor.identity_reset_password(
    p_token_hash bytea,
    p_password_hash text,
    p_now timestamptz,
    p_reason text,
    p_session_id uuid,
    p_session_token_hash bytea,
    p_csrf_token_hash bytea,
    p_created_at timestamptz,
    p_idle_expires_at timestamptz,
    p_absolute_expires_at timestamptz
) RETURNS uuid LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE matched_user_id uuid;
BEGIN
    UPDATE folioharbor.password_reset_tokens SET consumed_at = p_now, version = version + 1
    WHERE token_hash = p_token_hash AND consumed_at IS NULL AND expires_at > p_now
    RETURNING user_id INTO matched_user_id;
    IF matched_user_id IS NULL THEN RETURN NULL; END IF;

    UPDATE folioharbor.password_credentials SET password_hash = p_password_hash,
      changed_at = p_now, version = version + 1 WHERE user_id = matched_user_id;
    UPDATE folioharbor.user_sessions SET revoked_at = p_now, revocation_reason = p_reason,
      version = version + 1 WHERE user_id = matched_user_id AND revoked_at IS NULL;
    INSERT INTO folioharbor.user_sessions (
      session_id, user_id, session_token_hash, csrf_token_hash, created_at, last_seen_at,
      idle_expires_at, absolute_expires_at
    ) VALUES (
      p_session_id, matched_user_id, p_session_token_hash, p_csrf_token_hash, p_created_at,
      p_created_at, p_idle_expires_at, p_absolute_expires_at
    );
    RETURN matched_user_id;
END $$;

ALTER FUNCTION folioharbor.identity_reset_password(bytea, text, timestamptz, text, uuid, bytea, bytea, timestamptz, timestamptz, timestamptz) OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.identity_reset_password(bytea, text, timestamptz, text, uuid, bytea, bytea, timestamptz, timestamptz, timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.identity_reset_password(bytea, text, timestamptz, text, uuid, bytea, bytea, timestamptz, timestamptz, timestamptz) TO folioharbor_api;

UPDATE folioharbor.schema_metadata SET schema_version = 4, applied_at = clock_timestamp();
