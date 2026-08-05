DROP FUNCTION folioharbor.identity_authenticate_session(bytea, timestamptz, timestamptz);
CREATE FUNCTION folioharbor.identity_authenticate_session(p_token_hash bytea, p_now timestamptz, p_new_idle timestamptz)
RETURNS TABLE(user_id uuid, session_id uuid, csrf_token_hash bytea) LANGUAGE sql SECURITY DEFINER SET search_path TO '' AS $$
    UPDATE folioharbor.user_sessions SET last_seen_at = p_now,
      idle_expires_at = LEAST(p_new_idle, absolute_expires_at), version = version + 1
    WHERE session_token_hash = p_token_hash AND revoked_at IS NULL
      AND idle_expires_at > p_now AND absolute_expires_at > p_now
    RETURNING user_id, session_id, csrf_token_hash
$$;
ALTER FUNCTION folioharbor.identity_authenticate_session(bytea, timestamptz, timestamptz) OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.identity_authenticate_session(bytea, timestamptz, timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.identity_authenticate_session(bytea, timestamptz, timestamptz) TO folioharbor_api;

CREATE TABLE folioharbor.auth_rate_limit_buckets (
    bucket_key bytea PRIMARY KEY CHECK (octet_length(bucket_key) = 32),
    purpose text NOT NULL CHECK (purpose IN ('registration', 'login', 'verification', 'invitation', 'password_reset')),
    tokens double precision NOT NULL CHECK (tokens >= 0),
    updated_at timestamptz NOT NULL,
    version bigint NOT NULL DEFAULT 1 CHECK (version > 0)
);
ALTER TABLE folioharbor.auth_rate_limit_buckets OWNER TO folioharbor_owner;
GRANT SELECT, INSERT, UPDATE ON folioharbor.auth_rate_limit_buckets TO folioharbor_api;

UPDATE folioharbor.schema_metadata SET schema_version = 3, applied_at = clock_timestamp();
