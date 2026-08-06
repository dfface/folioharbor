CREATE TABLE folioharbor.upload_sessions (
    upload_id uuid PRIMARY KEY,
    library_id uuid NOT NULL REFERENCES folioharbor.libraries(library_id) ON DELETE CASCADE,
    created_by uuid NOT NULL REFERENCES folioharbor.user_accounts(user_id),
    file_name text NOT NULL CHECK (length(file_name) BETWEEN 1 AND 512),
    media_type text NOT NULL CHECK (media_type IN ('application/epub+zip','application/octet-stream')),
    declared_bytes bigint NOT NULL CHECK (declared_bytes BETWEEN 1 AND 1073741824),
    dedup_scope text NOT NULL CHECK(dedup_scope IN('instance','library','disabled')),
    received_bytes bigint NOT NULL DEFAULT 0 CHECK (received_bytes >= 0 AND received_bytes <= declared_bytes),
    state text NOT NULL CHECK (state IN ('created','receiving','received','queued','validating','importing','ready','duplicate','failed','expired','retry_wait')),
    storage_key text CHECK (storage_key IS NULL OR length(storage_key) BETWEEN 1 AND 512),
    receipt_token uuid,
    receipt_lease_expires_at timestamptz,
    promotion_key text CHECK (promotion_key IS NULL OR length(promotion_key) BETWEEN 1 AND 512),
    promotion_owned boolean NOT NULL DEFAULT false,
    promotion_disposition text CHECK(promotion_disposition IS NULL OR promotion_disposition IN('installed','reused')),
    sha256 bytea CHECK (sha256 IS NULL OR octet_length(sha256)=32),
    error_code text CHECK (error_code IS NULL OR error_code ~ '^[a-z][a-z0-9_]{0,63}$'),
    error_summary text CHECK (error_summary IS NULL OR length(error_summary) <= 512),
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL
    ,CHECK ((state='receiving')=(receipt_token IS NOT NULL AND receipt_lease_expires_at IS NOT NULL))
);
CREATE INDEX upload_sessions_library_state_idx ON folioharbor.upload_sessions(library_id,state,updated_at);
CREATE INDEX upload_sessions_abandoned_idx ON folioharbor.upload_sessions(expires_at,upload_id)
 WHERE state IN('created','received');

CREATE TABLE folioharbor.failed_upload_purges (
 upload_id uuid PRIMARY KEY REFERENCES folioharbor.upload_sessions(upload_id) ON DELETE CASCADE,
 storage_key text NOT NULL CHECK(length(storage_key) BETWEEN 1 AND 512),
 delete_file boolean NOT NULL,
 eligible_at timestamptz NOT NULL,
 state text NOT NULL DEFAULT 'pending' CHECK(state IN('pending','leased','completed')),
 lease_owner text,lease_expires_at timestamptz,completed_at timestamptz,
 created_at timestamptz NOT NULL,updated_at timestamptz NOT NULL,
 CHECK((state='leased')=(lease_owner IS NOT NULL AND lease_expires_at IS NOT NULL))
);
CREATE INDEX failed_upload_purges_claim_idx ON folioharbor.failed_upload_purges(eligible_at,upload_id)
 WHERE state IN('pending','leased');
CREATE TABLE folioharbor.cleanup_boundaries (
 cleanup_kind text PRIMARY KEY CHECK(cleanup_kind IN('expire_uploads','purge_failed_uploads','blob_gc')),
 cursor_at timestamptz NOT NULL,updated_at timestamptz NOT NULL
);

CREATE TABLE folioharbor.upload_cleanups (
 upload_id uuid NOT NULL REFERENCES folioharbor.upload_sessions(upload_id) ON DELETE CASCADE,
 attempt_token uuid NOT NULL, staging_key text NOT NULL CHECK(staging_key ~ '^staging:[0-9a-f]{64}$'),
 final_key text CHECK(final_key IS NULL OR final_key ~ '^blob:[a-z0-9-]+:[0-9a-f]{64}:[0-9]+$'),
 final_owned boolean NOT NULL, state text NOT NULL DEFAULT 'pending' CHECK(state IN('pending','leased','completed')),
 lease_owner text, lease_expires_at timestamptz, created_at timestamptz NOT NULL, completed_at timestamptz,
 PRIMARY KEY(upload_id,attempt_token),
 CHECK((state='leased')=(lease_owner IS NOT NULL AND lease_expires_at IS NOT NULL))
);

CREATE TABLE folioharbor.blob_reachability_candidates (
 storage_key text NOT NULL CHECK(length(storage_key) BETWEEN 1 AND 512),
 source_upload_id uuid NOT NULL REFERENCES folioharbor.upload_sessions(upload_id) ON DELETE CASCADE,
 namespace text NOT NULL,sha256 bytea NOT NULL CHECK(octet_length(sha256)=32),byte_size bigint NOT NULL CHECK(byte_size>=0),
 state text NOT NULL CHECK(state IN('promotion_unknown','installed_shared')),
 created_at timestamptz NOT NULL,updated_at timestamptz NOT NULL,
 PRIMARY KEY(storage_key,source_upload_id)
);

ALTER TABLE folioharbor.upload_sessions ENABLE ROW LEVEL SECURITY;
ALTER TABLE folioharbor.upload_sessions FORCE ROW LEVEL SECURITY;
CREATE POLICY uploads_owner_access ON folioharbor.upload_sessions
 USING (current_user='folioharbor_owner') WITH CHECK (current_user='folioharbor_owner');
CREATE POLICY uploads_runtime_read ON folioharbor.upload_sessions FOR SELECT USING (
 library_id=folioharbor.current_library_id() AND (folioharbor.is_worker() OR EXISTS(
  SELECT 1 FROM folioharbor.library_memberships m JOIN folioharbor.role_permissions p USING(role_code)
  WHERE m.library_id=upload_sessions.library_id AND m.user_id=folioharbor.current_user_id()
    AND m.status='active' AND p.permission_code='holding.edit')));
REVOKE ALL ON folioharbor.upload_sessions FROM PUBLIC;
GRANT SELECT ON folioharbor.upload_sessions TO folioharbor_api,folioharbor_worker;
ALTER TABLE folioharbor.upload_cleanups ENABLE ROW LEVEL SECURITY;
ALTER TABLE folioharbor.upload_cleanups FORCE ROW LEVEL SECURITY;
CREATE POLICY upload_cleanups_owner_access ON folioharbor.upload_cleanups
 USING (current_user='folioharbor_owner') WITH CHECK (current_user='folioharbor_owner');
CREATE POLICY upload_cleanups_worker_access ON folioharbor.upload_cleanups
 USING (folioharbor.is_worker()) WITH CHECK (folioharbor.is_worker());
REVOKE ALL ON folioharbor.upload_cleanups FROM PUBLIC;
GRANT SELECT,INSERT,UPDATE ON folioharbor.upload_cleanups TO folioharbor_worker;
ALTER TABLE folioharbor.failed_upload_purges ENABLE ROW LEVEL SECURITY;
ALTER TABLE folioharbor.failed_upload_purges FORCE ROW LEVEL SECURITY;
ALTER TABLE folioharbor.cleanup_boundaries ENABLE ROW LEVEL SECURITY;
ALTER TABLE folioharbor.cleanup_boundaries FORCE ROW LEVEL SECURITY;
CREATE POLICY failed_purges_owner_access ON folioharbor.failed_upload_purges
 USING(current_user='folioharbor_owner') WITH CHECK(current_user='folioharbor_owner');
CREATE POLICY cleanup_boundaries_owner_access ON folioharbor.cleanup_boundaries
 USING(current_user='folioharbor_owner') WITH CHECK(current_user='folioharbor_owner');
REVOKE ALL ON folioharbor.failed_upload_purges,folioharbor.cleanup_boundaries FROM PUBLIC;
ALTER TABLE folioharbor.blob_reachability_candidates ENABLE ROW LEVEL SECURITY;
ALTER TABLE folioharbor.blob_reachability_candidates FORCE ROW LEVEL SECURITY;
CREATE POLICY blob_candidates_owner_access ON folioharbor.blob_reachability_candidates
 USING (current_user='folioharbor_owner') WITH CHECK (current_user='folioharbor_owner');
CREATE POLICY blob_candidates_worker_access ON folioharbor.blob_reachability_candidates
 USING (folioharbor.is_worker()) WITH CHECK (folioharbor.is_worker());
REVOKE ALL ON folioharbor.blob_reachability_candidates FROM PUBLIC;
GRANT SELECT,UPDATE ON folioharbor.blob_reachability_candidates TO folioharbor_worker;

CREATE FUNCTION folioharbor.upload_create_authorized(
 p_upload uuid,p_library uuid,p_actor uuid,p_file text,p_media text,p_declared bigint,p_scope text,
 p_expires timestamptz,p_now timestamptz
) RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE library_row folioharbor.libraries%ROWTYPE;
BEGIN
 IF session_user <> 'folioharbor_api' OR p_actor IS DISTINCT FROM folioharbor.current_user_id()
    OR p_library IS DISTINCT FROM folioharbor.current_library_id() THEN RETURN 'not_found'; END IF;
 IF p_declared < 1 OR p_declared > 1073741824 OR p_file !~* '\.epub$'
    OR p_media NOT IN ('application/epub+zip','application/octet-stream')
    OR p_scope NOT IN('instance','library','disabled') THEN RETURN 'invalid'; END IF;
 SELECT * INTO library_row FROM folioharbor.libraries WHERE library_id=p_library FOR UPDATE;
 IF library_row.library_id IS NULL THEN RETURN 'not_found'; END IF;
 IF NOT EXISTS(SELECT 1 FROM folioharbor.library_memberships m JOIN folioharbor.role_permissions p USING(role_code)
  WHERE m.library_id=p_library AND m.user_id=p_actor AND m.status='active' AND p.permission_code='holding.edit')
 THEN RETURN 'forbidden'; END IF;
 IF EXISTS(SELECT 1 FROM folioharbor.upload_sessions WHERE upload_id=p_upload) THEN RETURN 'conflict'; END IF;
 IF p_declared > library_row.quota_limit_bytes-library_row.quota_used_bytes-library_row.quota_reserved_bytes
 THEN RETURN 'quota_exceeded'; END IF;
 INSERT INTO folioharbor.upload_sessions(upload_id,library_id,created_by,file_name,media_type,declared_bytes,dedup_scope,state,expires_at,created_at,updated_at)
 VALUES(p_upload,p_library,p_actor,p_file,p_media,p_declared,p_scope,'created',p_expires,p_now,p_now);
 INSERT INTO folioharbor.quota_reservations(upload_id,library_id,reserved_bytes,expires_at,state,created_at,updated_at)
 VALUES(p_upload,p_library,p_declared,p_expires,'active',p_now,p_now);
 UPDATE folioharbor.libraries SET quota_reserved_bytes=quota_reserved_bytes+p_declared WHERE library_id=p_library;
 RETURN 'created';
END $$;
REVOKE ALL ON FUNCTION folioharbor.upload_create_authorized(uuid,uuid,uuid,text,text,bigint,text,timestamptz,timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.upload_create_authorized(uuid,uuid,uuid,text,text,bigint,text,timestamptz,timestamptz) TO folioharbor_api;

CREATE FUNCTION folioharbor.upload_begin_receipt_authorized(
 p_upload uuid,p_library uuid,p_actor uuid,p_from text,p_now timestamptz
) RETURNS TABLE(attempt_token uuid,staging_key text) LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE reservation folioharbor.quota_reservations%ROWTYPE;
DECLARE upload folioharbor.upload_sessions%ROWTYPE;
DECLARE generated_token uuid;
DECLARE generated_staging text;
BEGIN
 IF session_user <> 'folioharbor_api' OR p_actor IS DISTINCT FROM folioharbor.current_user_id()
  OR p_library IS DISTINCT FROM folioharbor.current_library_id() THEN RETURN; END IF;
 IF NOT EXISTS(SELECT 1 FROM folioharbor.library_memberships m JOIN folioharbor.role_permissions p USING(role_code)
  WHERE m.library_id=p_library AND m.user_id=p_actor AND m.status='active' AND p.permission_code='holding.edit') THEN RETURN; END IF;
 PERFORM 1 FROM folioharbor.libraries WHERE library_id=p_library FOR UPDATE;
 SELECT * INTO upload FROM folioharbor.upload_sessions WHERE upload_id=p_upload AND library_id=p_library FOR UPDATE;
 IF upload.upload_id IS NULL OR upload.state<>p_from OR p_from NOT IN('created','failed') THEN RETURN; END IF;
 SELECT * INTO reservation FROM folioharbor.quota_reservations WHERE upload_id=p_upload FOR UPDATE;
 IF p_from='failed' THEN
  IF reservation.state<>'released'
   OR EXISTS(SELECT 1 FROM folioharbor.upload_cleanups WHERE upload_id=p_upload AND state<>'completed')
   OR upload.declared_bytes > (SELECT quota_limit_bytes-quota_used_bytes-quota_reserved_bytes FROM folioharbor.libraries WHERE library_id=p_library) THEN RETURN; END IF;
  UPDATE folioharbor.quota_reservations SET state='active',reserved_bytes=upload.declared_bytes,updated_at=p_now WHERE upload_id=p_upload;
  UPDATE folioharbor.libraries SET quota_reserved_bytes=quota_reserved_bytes+upload.declared_bytes WHERE library_id=p_library;
 ELSIF reservation.state<>'active' THEN
  RETURN;
 END IF;
 generated_token := gen_random_uuid();
 generated_staging := 'staging:'||replace(gen_random_uuid()::text,'-','')||replace(gen_random_uuid()::text,'-','');
 UPDATE folioharbor.upload_sessions SET state='receiving',received_bytes=0,storage_key=generated_staging,
  receipt_token=generated_token,receipt_lease_expires_at=p_now+interval '5 minutes',
  promotion_key=NULL,promotion_owned=false,promotion_disposition=NULL,sha256=NULL,error_code=NULL,updated_at=p_now
  WHERE upload_id=p_upload;
 RETURN QUERY SELECT generated_token,generated_staging;
END $$;
REVOKE ALL ON FUNCTION folioharbor.upload_begin_receipt_authorized(uuid,uuid,uuid,text,timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.upload_begin_receipt_authorized(uuid,uuid,uuid,text,timestamptz) TO folioharbor_api;

CREATE FUNCTION folioharbor.upload_transition_authorized(
 p_upload uuid,p_library uuid,p_actor uuid,p_from text,p_to text,p_received bigint,
 p_attempt uuid,p_storage text,p_error text,p_now timestamptz
) RETURNS boolean LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE reservation folioharbor.quota_reservations%ROWTYPE;
DECLARE upload folioharbor.upload_sessions%ROWTYPE;
BEGIN
 IF session_user <> 'folioharbor_api' OR p_actor IS DISTINCT FROM folioharbor.current_user_id()
  OR p_library IS DISTINCT FROM folioharbor.current_library_id() THEN RETURN false; END IF;
 IF NOT EXISTS(SELECT 1 FROM folioharbor.library_memberships m JOIN folioharbor.role_permissions p USING(role_code)
  WHERE m.library_id=p_library AND m.user_id=p_actor AND m.status='active' AND p.permission_code='holding.edit') THEN RETURN false; END IF;
 PERFORM 1 FROM folioharbor.libraries WHERE library_id=p_library FOR UPDATE;
 SELECT * INTO upload FROM folioharbor.upload_sessions WHERE upload_id=p_upload AND library_id=p_library FOR UPDATE;
 IF upload.upload_id IS NULL OR upload.state<>p_from OR p_received<0 OR p_received>upload.declared_bytes THEN RETURN false; END IF;
 IF NOT (p_from='receiving' AND p_to='failed') THEN RETURN false; END IF;
 IF p_attempt IS DISTINCT FROM upload.receipt_token OR p_storage IS DISTINCT FROM upload.storage_key THEN RETURN false; END IF;
 SELECT * INTO reservation FROM folioharbor.quota_reservations WHERE upload_id=p_upload FOR UPDATE;
 IF reservation.state='active' THEN
   UPDATE folioharbor.libraries SET quota_reserved_bytes=quota_reserved_bytes-reservation.reserved_bytes WHERE library_id=p_library;
   UPDATE folioharbor.quota_reservations SET state='released',updated_at=p_now WHERE upload_id=p_upload;
 END IF;
 UPDATE folioharbor.upload_sessions SET state=p_to,received_bytes=p_received,storage_key=NULL,
  receipt_token=NULL,receipt_lease_expires_at=NULL,promotion_key=NULL,promotion_owned=false,promotion_disposition=NULL,
  error_code=p_error,updated_at=p_now WHERE upload_id=p_upload;
 RETURN true;
END $$;
REVOKE ALL ON FUNCTION folioharbor.upload_transition_authorized(uuid,uuid,uuid,text,text,bigint,uuid,text,text,timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.upload_transition_authorized(uuid,uuid,uuid,text,text,bigint,uuid,text,text,timestamptz) TO folioharbor_api;

CREATE FUNCTION folioharbor.upload_transition_worker(
 p_upload uuid,p_library uuid,p_from text,p_to text,p_error text,p_now timestamptz
) RETURNS boolean LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE upload folioharbor.upload_sessions%ROWTYPE;
BEGIN
 IF NOT folioharbor.is_worker() OR p_library IS DISTINCT FROM folioharbor.current_library_id() THEN RETURN false; END IF;
 SELECT * INTO upload FROM folioharbor.upload_sessions WHERE upload_id=p_upload AND library_id=p_library FOR UPDATE;
 IF upload.upload_id IS NULL OR upload.state<>p_from THEN RETURN false; END IF;
 IF NOT ((p_from='queued' AND p_to='validating') OR
   (p_from='validating' AND p_to IN('importing','failed','retry_wait')) OR
   (p_from='importing' AND p_to IN('ready','duplicate','failed','retry_wait')) OR
   (p_from='retry_wait' AND p_to='queued')) THEN RETURN false; END IF;
 UPDATE folioharbor.upload_sessions SET state=p_to,error_code=p_error,updated_at=p_now WHERE upload_id=p_upload;
 RETURN true;
END $$;
REVOKE ALL ON FUNCTION folioharbor.upload_transition_worker(uuid,uuid,text,text,text,timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.upload_transition_worker(uuid,uuid,text,text,text,timestamptz) TO folioharbor_worker;

CREATE TABLE folioharbor.background_jobs (
 job_id uuid PRIMARY KEY,library_id uuid NOT NULL REFERENCES folioharbor.libraries(library_id) ON DELETE CASCADE,
 kind text NOT NULL CHECK(kind IN('import_epub')),state text NOT NULL CHECK(state IN('pending','leased','retry_wait','succeeded','failed')),
 input jsonb NOT NULL CHECK(
  jsonb_typeof(input)='object'
  AND input->'version'='1'::jsonb
  AND jsonb_typeof(input->'upload_id')='string'
  AND input->>'upload_id' ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$'
 ),
 idempotency_key text NOT NULL UNIQUE CHECK(length(idempotency_key) BETWEEN 1 AND 200),
 attempt_count integer NOT NULL DEFAULT 0 CHECK(attempt_count>=0),next_run_at timestamptz NOT NULL,
 lease_owner text CHECK(lease_owner IS NULL OR length(lease_owner) BETWEEN 1 AND 128),lease_expires_at timestamptz,
 error_code text CHECK(error_code IS NULL OR error_code ~ '^[a-z][a-z0-9_]{0,63}$'),
 error_summary text CHECK(error_summary IS NULL OR length(error_summary)<=512),created_at timestamptz NOT NULL,updated_at timestamptz NOT NULL,
 CHECK((state='leased')=(lease_owner IS NOT NULL AND lease_expires_at IS NOT NULL))
);
CREATE INDEX background_jobs_lease_idx ON folioharbor.background_jobs(next_run_at,created_at) WHERE state IN('pending','retry_wait','leased');
CREATE TABLE folioharbor.job_attempts (
 job_id uuid NOT NULL REFERENCES folioharbor.background_jobs(job_id) ON DELETE CASCADE,attempt integer NOT NULL CHECK(attempt>0),
 lease_owner text NOT NULL,started_at timestamptz NOT NULL,finished_at timestamptz,outcome text CHECK(outcome IS NULL OR outcome IN('succeeded','retry','failed','lease_expired')),
 error_code text,error_summary text,PRIMARY KEY(job_id,attempt)
);

CREATE FUNCTION folioharbor.upload_heartbeat_authorized(
 p_upload uuid,p_library uuid,p_actor uuid,p_attempt uuid,p_staging text,p_now timestamptz
) RETURNS boolean LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
BEGIN
 IF session_user <> 'folioharbor_api' OR p_actor IS DISTINCT FROM folioharbor.current_user_id()
  OR p_library IS DISTINCT FROM folioharbor.current_library_id() THEN RETURN false; END IF;
 UPDATE folioharbor.upload_sessions SET receipt_lease_expires_at=p_now+interval '5 minutes',updated_at=p_now
  WHERE upload_id=p_upload AND library_id=p_library AND state='receiving'
   AND receipt_token=p_attempt AND storage_key=p_staging AND receipt_lease_expires_at>p_now;
 RETURN FOUND;
END $$;
REVOKE ALL ON FUNCTION folioharbor.upload_heartbeat_authorized(uuid,uuid,uuid,uuid,text,timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.upload_heartbeat_authorized(uuid,uuid,uuid,uuid,text,timestamptz) TO folioharbor_api;

CREATE FUNCTION folioharbor.upload_prepare_promotion_authorized(
 p_upload uuid,p_library uuid,p_actor uuid,p_attempt uuid,p_staging text,p_final text,p_digest bytea,p_received bigint,p_now timestamptz
) RETURNS boolean LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE upload folioharbor.upload_sessions%ROWTYPE;
DECLARE namespace text;
DECLARE expected_key text;
BEGIN
 IF session_user <> 'folioharbor_api' OR p_actor IS DISTINCT FROM folioharbor.current_user_id()
  OR p_library IS DISTINCT FROM folioharbor.current_library_id()
  OR p_staging !~ '^staging:[0-9a-f]{64}$' OR octet_length(p_digest)<>32 THEN RETURN false; END IF;
 SELECT * INTO upload FROM folioharbor.upload_sessions WHERE upload_id=p_upload AND library_id=p_library FOR UPDATE;
 IF upload.state<>'receiving' OR upload.receipt_token IS DISTINCT FROM p_attempt
  OR upload.storage_key IS DISTINCT FROM p_staging
  OR upload.receipt_lease_expires_at<=p_now OR p_received<0 OR p_received>upload.declared_bytes THEN RETURN false; END IF;
 namespace := CASE upload.dedup_scope
  WHEN 'instance' THEN 'instance-v1'
  WHEN 'library' THEN 'library-'||replace(p_library::text,'-','')
  WHEN 'disabled' THEN 'upload-'||replace(p_upload::text,'-','') END;
 expected_key := 'blob:'||namespace||':'||encode(p_digest,'hex')||':'||p_received::text;
 IF p_final IS DISTINCT FROM expected_key THEN RETURN false; END IF;
 UPDATE folioharbor.upload_sessions SET promotion_key=p_final,promotion_owned=false,
  promotion_disposition=NULL,sha256=p_digest,received_bytes=p_received,updated_at=p_now WHERE upload_id=p_upload;
 INSERT INTO folioharbor.blob_reachability_candidates(storage_key,source_upload_id,namespace,sha256,byte_size,state,created_at,updated_at)
  VALUES(p_final,p_upload,namespace,p_digest,p_received,'promotion_unknown',p_now,p_now)
  ON CONFLICT(storage_key,source_upload_id) DO UPDATE SET state='promotion_unknown',updated_at=p_now;
 RETURN true;
END $$;
REVOKE ALL ON FUNCTION folioharbor.upload_prepare_promotion_authorized(uuid,uuid,uuid,uuid,text,text,bytea,bigint,timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.upload_prepare_promotion_authorized(uuid,uuid,uuid,uuid,text,text,bytea,bigint,timestamptz) TO folioharbor_api;

CREATE FUNCTION folioharbor.upload_record_promotion_disposition_authorized(
 p_upload uuid,p_library uuid,p_actor uuid,p_attempt uuid,p_staging text,p_final text,p_disposition text,p_now timestamptz
) RETURNS boolean LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE upload folioharbor.upload_sessions%ROWTYPE;
BEGIN
 IF session_user <> 'folioharbor_api' OR p_actor IS DISTINCT FROM folioharbor.current_user_id()
  OR p_library IS DISTINCT FROM folioharbor.current_library_id()
  OR p_disposition NOT IN('installed','reused') THEN RETURN false; END IF;
 SELECT * INTO upload FROM folioharbor.upload_sessions WHERE upload_id=p_upload AND library_id=p_library FOR UPDATE;
 IF upload.state<>'receiving' OR upload.receipt_token IS DISTINCT FROM p_attempt
  OR upload.storage_key IS DISTINCT FROM p_staging
  OR upload.promotion_key IS DISTINCT FROM p_final OR upload.receipt_lease_expires_at<=p_now THEN RETURN false; END IF;
 UPDATE folioharbor.upload_sessions SET promotion_disposition=p_disposition,
  promotion_owned=(upload.dedup_scope='disabled' AND p_disposition='installed'),updated_at=p_now
  WHERE upload_id=p_upload;
 IF p_disposition='installed' AND upload.dedup_scope IN('instance','library') THEN
  UPDATE folioharbor.blob_reachability_candidates SET state='installed_shared',updated_at=p_now
   WHERE storage_key=p_final AND source_upload_id=p_upload;
 ELSE
  DELETE FROM folioharbor.blob_reachability_candidates WHERE storage_key=p_final AND source_upload_id=p_upload;
 END IF;
 RETURN true;
END $$;
REVOKE ALL ON FUNCTION folioharbor.upload_record_promotion_disposition_authorized(uuid,uuid,uuid,uuid,text,text,text,timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.upload_record_promotion_disposition_authorized(uuid,uuid,uuid,uuid,text,text,text,timestamptz) TO folioharbor_api;

CREATE FUNCTION folioharbor.upload_mark_received_authorized(
 p_upload uuid,p_library uuid,p_actor uuid,p_attempt uuid,p_staging text,p_final text,p_received bigint,p_now timestamptz
) RETURNS boolean LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE upload folioharbor.upload_sessions%ROWTYPE;
DECLARE reservation folioharbor.quota_reservations%ROWTYPE;
BEGIN
 IF session_user <> 'folioharbor_api' OR p_actor IS DISTINCT FROM folioharbor.current_user_id()
  OR p_library IS DISTINCT FROM folioharbor.current_library_id() THEN RETURN false; END IF;
 PERFORM 1 FROM folioharbor.libraries WHERE library_id=p_library FOR UPDATE;
 SELECT * INTO upload FROM folioharbor.upload_sessions WHERE upload_id=p_upload AND library_id=p_library FOR UPDATE;
 IF upload.state<>'receiving' OR upload.receipt_token IS DISTINCT FROM p_attempt
  OR upload.storage_key IS DISTINCT FROM p_staging
  OR upload.promotion_key IS DISTINCT FROM p_final OR upload.receipt_lease_expires_at<=p_now
  OR upload.promotion_disposition IS NULL OR upload.received_bytes<>p_received
  OR p_received<0 OR p_received>upload.declared_bytes THEN RETURN false; END IF;
 SELECT * INTO reservation FROM folioharbor.quota_reservations WHERE upload_id=p_upload FOR UPDATE;
 IF reservation.state<>'active' THEN RETURN false; END IF;
 UPDATE folioharbor.libraries SET quota_reserved_bytes=quota_reserved_bytes-reservation.reserved_bytes+p_received
  WHERE library_id=p_library;
 UPDATE folioharbor.quota_reservations SET reserved_bytes=p_received,updated_at=p_now WHERE upload_id=p_upload;
 UPDATE folioharbor.upload_sessions SET state='received',received_bytes=p_received,storage_key=p_final,
  receipt_token=NULL,receipt_lease_expires_at=NULL,promotion_key=NULL,promotion_owned=false,promotion_disposition=NULL,error_code=NULL,updated_at=p_now
  WHERE upload_id=p_upload;
 RETURN true;
END $$;
REVOKE ALL ON FUNCTION folioharbor.upload_mark_received_authorized(uuid,uuid,uuid,uuid,text,text,bigint,timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.upload_mark_received_authorized(uuid,uuid,uuid,uuid,text,text,bigint,timestamptz) TO folioharbor_api;

CREATE FUNCTION folioharbor.upload_record_orphan_cleanup_authorized(
 p_upload uuid,p_library uuid,p_actor uuid,p_attempt uuid,p_staging text,p_now timestamptz
) RETURNS void LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE upload folioharbor.upload_sessions%ROWTYPE;
BEGIN
 IF session_user <> 'folioharbor_api' OR p_actor IS DISTINCT FROM folioharbor.current_user_id()
  OR p_library IS DISTINCT FROM folioharbor.current_library_id()
  OR NOT EXISTS(SELECT 1 FROM folioharbor.library_memberships m JOIN folioharbor.role_permissions p USING(role_code)
   WHERE m.library_id=p_library AND m.user_id=p_actor AND m.status='active' AND p.permission_code='holding.edit')
 THEN RETURN; END IF;
 SELECT * INTO upload FROM folioharbor.upload_sessions WHERE upload_id=p_upload AND library_id=p_library FOR UPDATE;
 IF upload.state<>'receiving' OR upload.receipt_token IS DISTINCT FROM p_attempt
  OR upload.storage_key IS DISTINCT FROM p_staging THEN RETURN; END IF;
 INSERT INTO folioharbor.upload_cleanups(upload_id,attempt_token,staging_key,final_owned,created_at)
  VALUES(p_upload,upload.receipt_token,upload.storage_key,false,p_now)
  ON CONFLICT(upload_id,attempt_token) DO NOTHING;
END $$;
REVOKE ALL ON FUNCTION folioharbor.upload_record_orphan_cleanup_authorized(uuid,uuid,uuid,uuid,text,timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.upload_record_orphan_cleanup_authorized(uuid,uuid,uuid,uuid,text,timestamptz) TO folioharbor_api;

CREATE FUNCTION folioharbor.upload_finalize_authorized(
 p_upload uuid,p_library uuid,p_actor uuid,p_received bigint,p_storage text,p_staging text,p_job uuid,p_now timestamptz
) RETURNS boolean LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE reservation folioharbor.quota_reservations%ROWTYPE;
DECLARE upload folioharbor.upload_sessions%ROWTYPE;
BEGIN
 IF session_user <> 'folioharbor_api' OR p_actor IS DISTINCT FROM folioharbor.current_user_id()
  OR p_library IS DISTINCT FROM folioharbor.current_library_id() THEN RETURN false; END IF;
 IF NOT EXISTS(SELECT 1 FROM folioharbor.library_memberships m JOIN folioharbor.role_permissions p USING(role_code)
  WHERE m.library_id=p_library AND m.user_id=p_actor AND m.status='active' AND p.permission_code='holding.edit') THEN RETURN false; END IF;
 PERFORM 1 FROM folioharbor.libraries WHERE library_id=p_library FOR UPDATE;
 SELECT * INTO upload FROM folioharbor.upload_sessions WHERE upload_id=p_upload AND library_id=p_library FOR UPDATE;
 IF upload.upload_id IS NULL OR upload.state NOT IN('receiving','received','queued')
  OR p_received<0 OR p_received>upload.declared_bytes OR length(p_storage)=0 THEN RETURN false; END IF;
 IF upload.state='receiving' AND (upload.storage_key IS DISTINCT FROM p_staging
   OR upload.promotion_key IS DISTINCT FROM p_storage OR upload.promotion_disposition IS NULL
   OR upload.receipt_lease_expires_at<=p_now) THEN RETURN false; END IF;
 IF upload.state<>'receiving' AND
  (upload.received_bytes<>p_received OR upload.storage_key IS DISTINCT FROM p_storage) THEN RETURN false; END IF;
 SELECT * INTO reservation FROM folioharbor.quota_reservations WHERE upload_id=p_upload FOR UPDATE;
 IF reservation.state<>'active' THEN RETURN false; END IF;
 IF EXISTS(SELECT 1 FROM folioharbor.background_jobs WHERE idempotency_key='import:'||p_upload::text) THEN
   IF NOT EXISTS(SELECT 1 FROM folioharbor.background_jobs WHERE idempotency_key='import:'||p_upload::text
    AND library_id=p_library AND kind='import_epub' AND input->>'upload_id'=p_upload::text) THEN RETURN false; END IF;
 ELSE
   INSERT INTO folioharbor.background_jobs(job_id,library_id,kind,state,input,idempotency_key,next_run_at,created_at,updated_at)
   VALUES(p_job,p_library,'import_epub','pending',jsonb_build_object('version',1,'upload_id',p_upload::text),'import:'||p_upload::text,p_now,p_now,p_now);
 END IF;
 IF upload.state='receiving' THEN
   UPDATE folioharbor.libraries SET quota_reserved_bytes=quota_reserved_bytes-reservation.reserved_bytes+p_received WHERE library_id=p_library;
   UPDATE folioharbor.quota_reservations SET reserved_bytes=p_received,updated_at=p_now WHERE upload_id=p_upload;
 END IF;
 UPDATE folioharbor.upload_sessions SET state='queued',received_bytes=p_received,storage_key=p_storage,
  receipt_token=NULL,receipt_lease_expires_at=NULL,promotion_key=NULL,promotion_owned=false,
  promotion_disposition=NULL,error_code=NULL,updated_at=p_now
  WHERE upload_id=p_upload;
 RETURN true;
END $$;
REVOKE ALL ON FUNCTION folioharbor.upload_finalize_authorized(uuid,uuid,uuid,bigint,text,text,uuid,timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.upload_finalize_authorized(uuid,uuid,uuid,bigint,text,text,uuid,timestamptz) TO folioharbor_api;

CREATE FUNCTION folioharbor.upload_expire_worker(p_now timestamptz,p_limit bigint)
RETURNS bigint LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE candidate record;
DECLARE upload folioharbor.upload_sessions%ROWTYPE;
DECLARE reservation folioharbor.quota_reservations%ROWTYPE;
DECLARE expired bigint := 0;
BEGIN
 IF session_user <> 'folioharbor_worker' OR NOT folioharbor.is_worker() OR p_limit<1 THEN RETURN 0; END IF;
 FOR candidate IN SELECT upload_id,library_id FROM folioharbor.upload_sessions
  WHERE (state='created' AND expires_at<=p_now)
     OR (state='receiving' AND receipt_lease_expires_at<=p_now)
  ORDER BY COALESCE(receipt_lease_expires_at,expires_at) LIMIT p_limit FOR UPDATE SKIP LOCKED LOOP
   PERFORM 1 FROM folioharbor.libraries WHERE library_id=candidate.library_id FOR UPDATE;
   SELECT * INTO upload FROM folioharbor.upload_sessions WHERE upload_id=candidate.upload_id FOR UPDATE;
   IF NOT ((upload.state='created' AND upload.expires_at<=p_now)
        OR (upload.state='receiving' AND upload.receipt_lease_expires_at<=p_now)) THEN CONTINUE; END IF;
   SELECT * INTO reservation FROM folioharbor.quota_reservations WHERE upload_id=candidate.upload_id FOR UPDATE;
   IF reservation.state<>'active' THEN CONTINUE; END IF;
   UPDATE folioharbor.libraries SET quota_reserved_bytes=quota_reserved_bytes-reservation.reserved_bytes WHERE library_id=candidate.library_id;
   UPDATE folioharbor.quota_reservations SET state='released',updated_at=p_now WHERE upload_id=candidate.upload_id;
   IF upload.state='receiving' THEN
    INSERT INTO folioharbor.upload_cleanups(upload_id,attempt_token,staging_key,final_key,final_owned,created_at)
     VALUES(upload.upload_id,upload.receipt_token,upload.storage_key,upload.promotion_key,upload.promotion_owned,p_now)
     ON CONFLICT(upload_id,attempt_token) DO NOTHING;
    UPDATE folioharbor.upload_sessions SET state='failed',storage_key=NULL,receipt_token=NULL,
     receipt_lease_expires_at=NULL,promotion_key=NULL,promotion_owned=false,promotion_disposition=NULL,error_code='receipt_expired',updated_at=p_now
     WHERE upload_id=candidate.upload_id;
   ELSE
    UPDATE folioharbor.upload_sessions SET state='expired',error_code='upload_expired',updated_at=p_now WHERE upload_id=candidate.upload_id;
   END IF;
   expired := expired+1;
 END LOOP;
 RETURN expired;
END $$;
REVOKE ALL ON FUNCTION folioharbor.upload_expire_worker(timestamptz,bigint) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.upload_expire_worker(timestamptz,bigint) TO folioharbor_worker;

CREATE FUNCTION folioharbor.import_expire_abandoned_worker(p_boundary timestamptz,p_limit bigint)
RETURNS bigint LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE candidate record;
DECLARE reservation folioharbor.quota_reservations%ROWTYPE;
DECLARE expired bigint := 0;
BEGIN
 IF session_user<>'folioharbor_worker' OR NOT folioharbor.is_worker()
    OR p_limit<1 OR p_limit>1000 THEN RETURN 0; END IF;
 FOR candidate IN SELECT upload_id,library_id,state,storage_key,dedup_scope,sha256,received_bytes FROM folioharbor.upload_sessions
   WHERE state IN('created','received') AND expires_at<=p_boundary
   ORDER BY expires_at,upload_id LIMIT p_limit FOR UPDATE SKIP LOCKED LOOP
  PERFORM 1 FROM folioharbor.libraries WHERE library_id=candidate.library_id FOR UPDATE;
  SELECT * INTO reservation FROM folioharbor.quota_reservations
    WHERE upload_id=candidate.upload_id FOR UPDATE;
  IF reservation.state='active' THEN
   UPDATE folioharbor.libraries SET quota_reserved_bytes=quota_reserved_bytes-reservation.reserved_bytes
     WHERE library_id=candidate.library_id;
   UPDATE folioharbor.quota_reservations SET state='released',updated_at=p_boundary
     WHERE upload_id=candidate.upload_id;
  END IF;
  IF candidate.state='received' THEN
   IF candidate.dedup_scope='disabled' AND candidate.sha256 IS NOT NULL THEN
    INSERT INTO folioharbor.blobs(blob_id,storage_namespace,sha256,byte_size,created_at)
     VALUES(candidate.upload_id,'upload-'||replace(candidate.upload_id::text,'-',''),candidate.sha256,candidate.received_bytes,p_boundary)
     ON CONFLICT(storage_namespace,sha256,byte_size) DO NOTHING;
    INSERT INTO folioharbor.blob_locations(blob_id,storage_key,state,created_at,updated_at)
     VALUES(candidate.upload_id,candidate.storage_key,'quarantined',p_boundary,p_boundary)
     ON CONFLICT ON CONSTRAINT blob_locations_storage_key_key DO UPDATE SET state='quarantined',updated_at=p_boundary
     WHERE folioharbor.blob_locations.blob_id=EXCLUDED.blob_id;
   END IF;
   INSERT INTO folioharbor.failed_upload_purges(upload_id,storage_key,delete_file,eligible_at,created_at,updated_at)
    VALUES(candidate.upload_id,candidate.storage_key,candidate.dedup_scope='disabled',p_boundary+interval '24 hours',p_boundary,p_boundary)
    ON CONFLICT(upload_id) DO NOTHING;
   UPDATE folioharbor.upload_sessions SET state='failed',error_code='received_expired',updated_at=p_boundary
    WHERE upload_id=candidate.upload_id;
  ELSE
   UPDATE folioharbor.upload_sessions SET state='expired',error_code='upload_expired',updated_at=p_boundary
    WHERE upload_id=candidate.upload_id;
  END IF;
  expired := expired+1;
 END LOOP;
 INSERT INTO folioharbor.cleanup_boundaries(cleanup_kind,cursor_at,updated_at)
  VALUES('expire_uploads',p_boundary,p_boundary)
  ON CONFLICT(cleanup_kind) DO UPDATE SET cursor_at=GREATEST(folioharbor.cleanup_boundaries.cursor_at,EXCLUDED.cursor_at),updated_at=p_boundary;
 RETURN expired;
END $$;
ALTER FUNCTION folioharbor.import_expire_abandoned_worker(timestamptz,bigint) OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.import_expire_abandoned_worker(timestamptz,bigint) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.import_expire_abandoned_worker(timestamptz,bigint) TO folioharbor_worker;

CREATE FUNCTION folioharbor.import_schedule_failed_purge_worker(p_upload uuid,p_library uuid,p_now timestamptz)
RETURNS boolean LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE upload folioharbor.upload_sessions%ROWTYPE;
BEGIN
 IF session_user<>'folioharbor_worker' OR NOT folioharbor.is_worker()
    OR p_library IS DISTINCT FROM folioharbor.current_library_id() THEN RETURN false; END IF;
 SELECT * INTO upload FROM folioharbor.upload_sessions WHERE upload_id=p_upload AND library_id=p_library FOR UPDATE;
 IF upload.upload_id IS NULL OR upload.state<>'failed' OR upload.storage_key IS NULL THEN RETURN false; END IF;
 INSERT INTO folioharbor.failed_upload_purges(upload_id,storage_key,delete_file,eligible_at,created_at,updated_at)
  VALUES(p_upload,upload.storage_key,upload.dedup_scope='disabled',p_now+interval '24 hours',p_now,p_now)
  ON CONFLICT(upload_id) DO NOTHING;
 IF upload.dedup_scope='disabled' THEN
  UPDATE folioharbor.blob_locations SET state='quarantined',updated_at=p_now
   WHERE storage_key=upload.storage_key;
 END IF;
 RETURN true;
END $$;
ALTER FUNCTION folioharbor.import_schedule_failed_purge_worker(uuid,uuid,timestamptz) OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.import_schedule_failed_purge_worker(uuid,uuid,timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.import_schedule_failed_purge_worker(uuid,uuid,timestamptz) TO folioharbor_worker;

CREATE FUNCTION folioharbor.import_claim_failed_purges_worker(p_owner text,p_boundary timestamptz,p_limit bigint)
RETURNS TABLE(upload_id uuid,storage_key text,delete_file boolean) LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
BEGIN
 IF session_user<>'folioharbor_worker' OR NOT folioharbor.is_worker()
    OR length(p_owner) NOT BETWEEN 1 AND 128 OR p_limit<1 OR p_limit>1000 THEN RETURN; END IF;
 RETURN QUERY WITH candidates AS (
  SELECT purge.upload_id FROM folioharbor.failed_upload_purges purge
   WHERE purge.eligible_at<=p_boundary AND (purge.state='pending' OR (purge.state='leased' AND purge.lease_expires_at<=p_boundary))
   ORDER BY purge.eligible_at,purge.upload_id LIMIT p_limit FOR UPDATE SKIP LOCKED
 ), leased AS (
  UPDATE folioharbor.failed_upload_purges purge SET state='leased',lease_owner=p_owner,
   lease_expires_at=p_boundary+interval '5 minutes',updated_at=p_boundary
   FROM candidates WHERE purge.upload_id=candidates.upload_id
   RETURNING purge.upload_id,purge.storage_key,purge.delete_file
 ) SELECT leased.upload_id,leased.storage_key,leased.delete_file FROM leased;
 INSERT INTO folioharbor.cleanup_boundaries(cleanup_kind,cursor_at,updated_at)
  VALUES('purge_failed_uploads',p_boundary,p_boundary)
  ON CONFLICT(cleanup_kind) DO UPDATE SET cursor_at=GREATEST(folioharbor.cleanup_boundaries.cursor_at,EXCLUDED.cursor_at),updated_at=p_boundary;
END $$;
ALTER FUNCTION folioharbor.import_claim_failed_purges_worker(text,timestamptz,bigint) OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.import_claim_failed_purges_worker(text,timestamptz,bigint) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.import_claim_failed_purges_worker(text,timestamptz,bigint) TO folioharbor_worker;

CREATE FUNCTION folioharbor.import_complete_failed_purge_worker(p_upload uuid,p_owner text,p_now timestamptz)
RETURNS boolean LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
BEGIN
 IF session_user<>'folioharbor_worker' OR NOT folioharbor.is_worker() THEN RETURN false; END IF;
 UPDATE folioharbor.blob_locations location SET state='purged',updated_at=p_now
  FROM folioharbor.failed_upload_purges purge
  WHERE purge.upload_id=p_upload AND purge.state='leased' AND purge.lease_owner=p_owner
    AND purge.delete_file AND location.storage_key=purge.storage_key;
 UPDATE folioharbor.failed_upload_purges SET state='completed',lease_owner=NULL,lease_expires_at=NULL,
  completed_at=p_now,updated_at=p_now WHERE upload_id=p_upload AND state='leased' AND lease_owner=p_owner;
 RETURN FOUND;
END $$;
ALTER FUNCTION folioharbor.import_complete_failed_purge_worker(uuid,text,timestamptz) OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.import_complete_failed_purge_worker(uuid,text,timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.import_complete_failed_purge_worker(uuid,text,timestamptz) TO folioharbor_worker;
ALTER TABLE folioharbor.background_jobs ENABLE ROW LEVEL SECURITY; ALTER TABLE folioharbor.background_jobs FORCE ROW LEVEL SECURITY;
ALTER TABLE folioharbor.job_attempts ENABLE ROW LEVEL SECURITY; ALTER TABLE folioharbor.job_attempts FORCE ROW LEVEL SECURITY;
CREATE POLICY jobs_owner_access ON folioharbor.background_jobs USING(current_user='folioharbor_owner') WITH CHECK(current_user='folioharbor_owner');
CREATE POLICY attempts_owner_access ON folioharbor.job_attempts USING(current_user='folioharbor_owner') WITH CHECK(current_user='folioharbor_owner');
CREATE POLICY jobs_worker_access ON folioharbor.background_jobs USING(folioharbor.is_worker()) WITH CHECK(folioharbor.is_worker());
CREATE POLICY attempts_worker_access ON folioharbor.job_attempts USING(folioharbor.is_worker()) WITH CHECK(folioharbor.is_worker());
REVOKE ALL ON folioharbor.background_jobs,folioharbor.job_attempts FROM PUBLIC;
GRANT SELECT,INSERT,UPDATE ON folioharbor.background_jobs,folioharbor.job_attempts TO folioharbor_worker;

UPDATE folioharbor.schema_metadata SET schema_version=9,applied_at=clock_timestamp() WHERE singleton;
