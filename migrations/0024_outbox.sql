CREATE TABLE folioharbor.mail_outbox (
    mail_id uuid PRIMARY KEY,
    recipient_account_id uuid REFERENCES folioharbor.user_accounts(user_id) ON DELETE SET NULL,
    delivery_address text NOT NULL CHECK(length(delivery_address) BETWEEN 3 AND 320),
    template_code text NOT NULL CHECK(template_code IN ('verification','invitation','password_reset')),
    template_version integer NOT NULL CHECK(template_version > 0),
    locale text NOT NULL CHECK(locale IN ('en','zh-CN')),
    token_ciphertext bytea NOT NULL CHECK(octet_length(token_ciphertext) > 0),
    encryption_key_id text NOT NULL CHECK(length(encryption_key_id) BETWEEN 1 AND 128),
    token_nonce bytea NOT NULL CHECK(octet_length(token_nonce) = 12),
    idempotency_key text NOT NULL UNIQUE CHECK(length(idempotency_key) BETWEEN 1 AND 200),
    state text NOT NULL CHECK(state IN ('pending','leased','retry_wait','sent','failed','expired')),
    attempt_count integer NOT NULL DEFAULT 0 CHECK(attempt_count >= 0),
    next_run_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    last_error_code text CHECK(last_error_code IS NULL OR last_error_code ~ '^[a-z][a-z0-9_]{0,63}$'),
    delivered_at timestamptz,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL,
    CHECK(expires_at > created_at),
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
CREATE POLICY mail_outbox_worker_access ON folioharbor.mail_outbox
    USING(folioharbor.is_worker()) WITH CHECK(folioharbor.is_worker());
REVOKE ALL ON folioharbor.mail_outbox FROM PUBLIC;
GRANT INSERT ON folioharbor.mail_outbox TO folioharbor_api;
GRANT SELECT,UPDATE ON folioharbor.mail_outbox TO folioharbor_worker;
UPDATE folioharbor.schema_metadata SET schema_version=24,applied_at=clock_timestamp() WHERE singleton;
