CREATE TABLE folioharbor.user_accounts (
    user_id uuid PRIMARY KEY,
    normalized_email text NOT NULL UNIQUE,
    display_email text NOT NULL,
    status text NOT NULL CHECK (status IN ('pending_verification', 'verified', 'disabled')),
    created_at timestamptz NOT NULL,
    verified_at timestamptz,
    disabled_at timestamptz,
    version bigint NOT NULL DEFAULT 1 CHECK (version > 0)
);

CREATE TABLE folioharbor.password_credentials (
    user_id uuid PRIMARY KEY REFERENCES folioharbor.user_accounts(user_id) ON DELETE CASCADE,
    password_hash text NOT NULL,
    created_at timestamptz NOT NULL,
    changed_at timestamptz NOT NULL,
    version bigint NOT NULL DEFAULT 1 CHECK (version > 0)
);

CREATE TABLE folioharbor.email_verification_tokens (
    token_id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES folioharbor.user_accounts(user_id) ON DELETE CASCADE,
    token_hash bytea NOT NULL UNIQUE CHECK (octet_length(token_hash) = 32),
    created_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    consumed_at timestamptz,
    version bigint NOT NULL DEFAULT 1 CHECK (version > 0),
    CHECK (expires_at > created_at)
);

CREATE TABLE folioharbor.password_reset_tokens (
    token_id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES folioharbor.user_accounts(user_id) ON DELETE CASCADE,
    token_hash bytea NOT NULL UNIQUE CHECK (octet_length(token_hash) = 32),
    created_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    consumed_at timestamptz,
    version bigint NOT NULL DEFAULT 1 CHECK (version > 0),
    CHECK (expires_at > created_at)
);

CREATE TABLE folioharbor.user_sessions (
    session_id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES folioharbor.user_accounts(user_id) ON DELETE CASCADE,
    session_token_hash bytea NOT NULL UNIQUE CHECK (octet_length(session_token_hash) = 32),
    csrf_token_hash bytea NOT NULL UNIQUE CHECK (octet_length(csrf_token_hash) = 32),
    created_at timestamptz NOT NULL,
    last_seen_at timestamptz NOT NULL,
    idle_expires_at timestamptz NOT NULL,
    absolute_expires_at timestamptz NOT NULL,
    revoked_at timestamptz,
    revocation_reason text,
    version bigint NOT NULL DEFAULT 1 CHECK (version > 0),
    CHECK (idle_expires_at > created_at),
    CHECK (absolute_expires_at > idle_expires_at),
    CHECK ((revoked_at IS NULL) = (revocation_reason IS NULL))
);

CREATE INDEX user_sessions_user_id_idx ON folioharbor.user_sessions (user_id);

CREATE TABLE folioharbor.user_devices (
    device_id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES folioharbor.user_accounts(user_id) ON DELETE CASCADE,
    display_name text NOT NULL,
    created_at timestamptz NOT NULL,
    last_seen_at timestamptz NOT NULL,
    revoked_at timestamptz,
    version bigint NOT NULL DEFAULT 1 CHECK (version > 0)
);

ALTER TABLE folioharbor.user_sessions ENABLE ROW LEVEL SECURITY;
ALTER TABLE folioharbor.user_devices ENABLE ROW LEVEL SECURITY;

CREATE POLICY user_sessions_by_user ON folioharbor.user_sessions
    USING (user_id = folioharbor.current_user_id())
    WITH CHECK (user_id = folioharbor.current_user_id());
CREATE POLICY user_devices_by_user ON folioharbor.user_devices
    USING (user_id = folioharbor.current_user_id())
    WITH CHECK (user_id = folioharbor.current_user_id());

CREATE FUNCTION folioharbor.identity_register(
    p_user_id uuid, p_normalized_email text, p_display_email text, p_password_hash text,
    p_token_id uuid, p_token_hash bytea, p_created_at timestamptz, p_expires_at timestamptz
) RETURNS boolean LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
BEGIN
    INSERT INTO folioharbor.user_accounts (user_id, normalized_email, display_email, status, created_at)
    VALUES (p_user_id, p_normalized_email, p_display_email, 'pending_verification', p_created_at)
    ON CONFLICT (normalized_email) DO NOTHING;
    IF NOT FOUND THEN RETURN false; END IF;
    INSERT INTO folioharbor.password_credentials (user_id, password_hash, created_at, changed_at)
    VALUES (p_user_id, p_password_hash, p_created_at, p_created_at);
    INSERT INTO folioharbor.email_verification_tokens (token_id, user_id, token_hash, created_at, expires_at)
    VALUES (p_token_id, p_user_id, p_token_hash, p_created_at, p_expires_at);
    RETURN true;
END $$;

CREATE FUNCTION folioharbor.identity_verify_email(p_token_hash bytea, p_now timestamptz)
RETURNS uuid LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE matched_user_id uuid;
BEGIN
    UPDATE folioharbor.email_verification_tokens SET consumed_at = p_now, version = version + 1
    WHERE token_hash = p_token_hash AND consumed_at IS NULL AND expires_at > p_now RETURNING user_id INTO matched_user_id;
    IF matched_user_id IS NULL THEN RETURN NULL; END IF;
    UPDATE folioharbor.user_accounts SET status = 'verified', verified_at = p_now, version = version + 1
    WHERE user_id = matched_user_id AND status = 'pending_verification';
    RETURN matched_user_id;
END $$;

CREATE FUNCTION folioharbor.identity_find_login(p_email text)
RETURNS TABLE(user_id uuid, status text, password_hash text) LANGUAGE sql SECURITY DEFINER SET search_path TO '' AS $$
    SELECT a.user_id, a.status, p.password_hash FROM folioharbor.user_accounts a
    JOIN folioharbor.password_credentials p USING (user_id) WHERE a.normalized_email = p_email
$$;

CREATE FUNCTION folioharbor.identity_create_session(
    p_session_id uuid, p_user_id uuid, p_session_hash bytea, p_csrf_hash bytea,
    p_created_at timestamptz, p_idle_expires_at timestamptz, p_absolute_expires_at timestamptz
) RETURNS void LANGUAGE sql SECURITY DEFINER SET search_path TO '' AS $$
    INSERT INTO folioharbor.user_sessions
      (session_id, user_id, session_token_hash, csrf_token_hash, created_at, last_seen_at, idle_expires_at, absolute_expires_at)
    VALUES (p_session_id, p_user_id, p_session_hash, p_csrf_hash, p_created_at, p_created_at, p_idle_expires_at, p_absolute_expires_at)
$$;

CREATE FUNCTION folioharbor.identity_authenticate_session(p_token_hash bytea, p_now timestamptz, p_new_idle timestamptz)
RETURNS TABLE(user_id uuid, session_id uuid) LANGUAGE sql SECURITY DEFINER SET search_path TO '' AS $$
    UPDATE folioharbor.user_sessions SET last_seen_at = p_now,
      idle_expires_at = LEAST(p_new_idle, absolute_expires_at), version = version + 1
    WHERE session_token_hash = p_token_hash AND revoked_at IS NULL
      AND idle_expires_at > p_now AND absolute_expires_at > p_now
    RETURNING user_id, session_id
$$;

CREATE FUNCTION folioharbor.identity_revoke_session(p_token_hash bytea, p_now timestamptz, p_reason text)
RETURNS void LANGUAGE sql SECURITY DEFINER SET search_path TO '' AS $$
    UPDATE folioharbor.user_sessions SET revoked_at = p_now, revocation_reason = p_reason, version = version + 1
    WHERE session_token_hash = p_token_hash AND revoked_at IS NULL
$$;

CREATE FUNCTION folioharbor.identity_issue_password_reset(
    p_token_id uuid, p_email text, p_token_hash bytea, p_created_at timestamptz, p_expires_at timestamptz
) RETURNS boolean LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
BEGIN
    INSERT INTO folioharbor.password_reset_tokens (token_id, user_id, token_hash, created_at, expires_at)
    SELECT p_token_id, user_id, p_token_hash, p_created_at, p_expires_at
    FROM folioharbor.user_accounts WHERE normalized_email = p_email;
    RETURN FOUND;
END $$;

CREATE FUNCTION folioharbor.identity_reset_password(p_token_hash bytea, p_password_hash text, p_now timestamptz)
RETURNS uuid LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE matched_user_id uuid;
BEGIN
    UPDATE folioharbor.password_reset_tokens SET consumed_at = p_now, version = version + 1
    WHERE token_hash = p_token_hash AND consumed_at IS NULL AND expires_at > p_now RETURNING user_id INTO matched_user_id;
    IF matched_user_id IS NULL THEN RETURN NULL; END IF;
    UPDATE folioharbor.password_credentials SET password_hash = p_password_hash, changed_at = p_now, version = version + 1
    WHERE user_id = matched_user_id;
    UPDATE folioharbor.user_sessions SET revoked_at = p_now, revocation_reason = 'password_reset', version = version + 1
    WHERE user_id = matched_user_id AND revoked_at IS NULL;
    RETURN matched_user_id;
END $$;

ALTER FUNCTION folioharbor.identity_register(uuid, text, text, text, uuid, bytea, timestamptz, timestamptz) OWNER TO folioharbor_owner;
ALTER FUNCTION folioharbor.identity_verify_email(bytea, timestamptz) OWNER TO folioharbor_owner;
ALTER FUNCTION folioharbor.identity_find_login(text) OWNER TO folioharbor_owner;
ALTER FUNCTION folioharbor.identity_create_session(uuid, uuid, bytea, bytea, timestamptz, timestamptz, timestamptz) OWNER TO folioharbor_owner;
ALTER FUNCTION folioharbor.identity_authenticate_session(bytea, timestamptz, timestamptz) OWNER TO folioharbor_owner;
ALTER FUNCTION folioharbor.identity_revoke_session(bytea, timestamptz, text) OWNER TO folioharbor_owner;
ALTER FUNCTION folioharbor.identity_issue_password_reset(uuid, text, bytea, timestamptz, timestamptz) OWNER TO folioharbor_owner;
ALTER FUNCTION folioharbor.identity_reset_password(bytea, text, timestamptz) OWNER TO folioharbor_owner;

REVOKE ALL ON FUNCTION folioharbor.identity_register(uuid, text, text, text, uuid, bytea, timestamptz, timestamptz) FROM PUBLIC;
REVOKE ALL ON FUNCTION folioharbor.identity_verify_email(bytea, timestamptz) FROM PUBLIC;
REVOKE ALL ON FUNCTION folioharbor.identity_find_login(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION folioharbor.identity_create_session(uuid, uuid, bytea, bytea, timestamptz, timestamptz, timestamptz) FROM PUBLIC;
REVOKE ALL ON FUNCTION folioharbor.identity_authenticate_session(bytea, timestamptz, timestamptz) FROM PUBLIC;
REVOKE ALL ON FUNCTION folioharbor.identity_revoke_session(bytea, timestamptz, text) FROM PUBLIC;
REVOKE ALL ON FUNCTION folioharbor.identity_issue_password_reset(uuid, text, bytea, timestamptz, timestamptz) FROM PUBLIC;
REVOKE ALL ON FUNCTION folioharbor.identity_reset_password(bytea, text, timestamptz) FROM PUBLIC;

GRANT EXECUTE ON FUNCTION folioharbor.identity_register(uuid, text, text, text, uuid, bytea, timestamptz, timestamptz) TO folioharbor_api;
GRANT EXECUTE ON FUNCTION folioharbor.identity_verify_email(bytea, timestamptz) TO folioharbor_api;
GRANT EXECUTE ON FUNCTION folioharbor.identity_find_login(text) TO folioharbor_api;
GRANT EXECUTE ON FUNCTION folioharbor.identity_create_session(uuid, uuid, bytea, bytea, timestamptz, timestamptz, timestamptz) TO folioharbor_api;
GRANT EXECUTE ON FUNCTION folioharbor.identity_authenticate_session(bytea, timestamptz, timestamptz) TO folioharbor_api;
GRANT EXECUTE ON FUNCTION folioharbor.identity_revoke_session(bytea, timestamptz, text) TO folioharbor_api;
GRANT EXECUTE ON FUNCTION folioharbor.identity_issue_password_reset(uuid, text, bytea, timestamptz, timestamptz) TO folioharbor_api;
GRANT EXECUTE ON FUNCTION folioharbor.identity_reset_password(bytea, text, timestamptz) TO folioharbor_api;

GRANT SELECT, INSERT, UPDATE ON folioharbor.user_sessions, folioharbor.user_devices TO folioharbor_api;

UPDATE folioharbor.schema_metadata SET schema_version = 2, applied_at = clock_timestamp();
