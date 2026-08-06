CREATE TABLE folioharbor.mail_outbox (
    mail_id uuid PRIMARY KEY,
    recipient_account_id uuid REFERENCES folioharbor.user_accounts(user_id) ON DELETE SET NULL,
    delivery_address text NOT NULL CHECK(length(delivery_address) BETWEEN 3 AND 320),
    template_code text NOT NULL CHECK(template_code IN ('verification','invitation','password_reset')),
    template_version integer NOT NULL CHECK(template_version > 0),
    locale text NOT NULL CHECK(locale IN ('en','zh-CN')),
    token_ciphertext bytea NOT NULL,
    encryption_key_id text NOT NULL CHECK(length(encryption_key_id) BETWEEN 1 AND 128),
    token_nonce bytea NOT NULL CHECK(octet_length(token_nonce) = 12),
    idempotency_key text NOT NULL UNIQUE CHECK(length(idempotency_key) BETWEEN 1 AND 200),
    invitation_library_id uuid REFERENCES folioharbor.libraries(library_id) ON DELETE SET NULL,
    invitation_role text CHECK(invitation_role IS NULL OR invitation_role IN ('editor','reader')),
    state text NOT NULL CHECK(state IN ('pending','leased','retry_wait','sent','failed','expired')),
    attempt_count integer NOT NULL DEFAULT 0 CHECK(attempt_count >= 0),
    lease_owner text,
    lease_expires_at timestamptz,
    next_run_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    last_error_code text CHECK(last_error_code IS NULL OR last_error_code ~ '^[a-z][a-z0-9_]{0,63}$'),
    delivered_at timestamptz,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    CHECK(expires_at > created_at),
    CHECK((lease_owner IS NULL) = (lease_expires_at IS NULL)),
    CHECK((template_code = 'invitation') = (invitation_library_id IS NOT NULL AND invitation_role IS NOT NULL)),
    CHECK((state IN ('sent','failed','expired')) = (token_ciphertext = ''::bytea))
);
CREATE INDEX mail_outbox_ready_idx ON folioharbor.mail_outbox(next_run_at,created_at)
    WHERE state IN ('pending','retry_wait');
ALTER TABLE folioharbor.mail_outbox ENABLE ROW LEVEL SECURITY;
ALTER TABLE folioharbor.mail_outbox FORCE ROW LEVEL SECURITY;
CREATE POLICY mail_outbox_owner_access ON folioharbor.mail_outbox
    USING(current_user='folioharbor_owner') WITH CHECK(current_user='folioharbor_owner');
CREATE POLICY mail_outbox_api_insert ON folioharbor.mail_outbox FOR INSERT
    WITH CHECK(current_user='folioharbor_api');
CREATE POLICY mail_outbox_api_idempotency ON folioharbor.mail_outbox FOR SELECT
    USING(current_user='folioharbor_api');
CREATE POLICY mail_outbox_worker_access ON folioharbor.mail_outbox
    USING(folioharbor.is_worker()) WITH CHECK(folioharbor.is_worker());
REVOKE ALL ON folioharbor.mail_outbox FROM PUBLIC;
GRANT INSERT,SELECT(idempotency_key) ON folioharbor.mail_outbox TO folioharbor_api;
GRANT SELECT,UPDATE ON folioharbor.mail_outbox TO folioharbor_worker;

CREATE FUNCTION folioharbor.mail_recipient_account_id(p_email text)
RETURNS uuid LANGUAGE sql STABLE SECURITY DEFINER SET search_path TO '' AS $$
    SELECT user_id FROM folioharbor.user_accounts WHERE normalized_email=p_email
$$;
CREATE FUNCTION folioharbor.identity_issue_password_reset_recipient(
    p_token_id uuid, p_email text, p_token_hash bytea,
    p_created_at timestamptz, p_expires_at timestamptz
) RETURNS uuid LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE matched_user_id uuid;
BEGIN
    SELECT user_id INTO matched_user_id
    FROM folioharbor.user_accounts WHERE normalized_email=p_email;
    IF matched_user_id IS NULL THEN RETURN NULL; END IF;
    INSERT INTO folioharbor.password_reset_tokens
      (token_id,user_id,token_hash,created_at,expires_at)
    VALUES (p_token_id,matched_user_id,p_token_hash,p_created_at,p_expires_at);
    RETURN matched_user_id;
END $$;
ALTER FUNCTION folioharbor.mail_recipient_account_id(text) OWNER TO folioharbor_owner;
ALTER FUNCTION folioharbor.identity_issue_password_reset_recipient(uuid,text,bytea,timestamptz,timestamptz) OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.mail_recipient_account_id(text) FROM PUBLIC;
REVOKE ALL ON FUNCTION folioharbor.identity_issue_password_reset_recipient(uuid,text,bytea,timestamptz,timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.mail_recipient_account_id(text) TO folioharbor_api;
GRANT EXECUTE ON FUNCTION folioharbor.identity_issue_password_reset_recipient(uuid,text,bytea,timestamptz,timestamptz) TO folioharbor_api;
UPDATE folioharbor.schema_metadata SET schema_version=24,applied_at=clock_timestamp() WHERE singleton;
