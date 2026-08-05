CREATE TABLE folioharbor.upload_sessions (
    upload_id uuid PRIMARY KEY,
    library_id uuid NOT NULL REFERENCES folioharbor.libraries(library_id) ON DELETE CASCADE,
    created_by uuid NOT NULL REFERENCES folioharbor.user_accounts(user_id),
    file_name text NOT NULL CHECK (length(file_name) BETWEEN 1 AND 512),
    media_type text NOT NULL CHECK (media_type IN ('application/epub+zip','application/octet-stream')),
    declared_bytes bigint NOT NULL CHECK (declared_bytes BETWEEN 1 AND 1073741824),
    received_bytes bigint NOT NULL DEFAULT 0 CHECK (received_bytes >= 0 AND received_bytes <= declared_bytes),
    state text NOT NULL CHECK (state IN ('created','receiving','received','queued','validating','importing','ready','duplicate','failed','expired','retry_wait')),
    storage_key text CHECK (storage_key IS NULL OR length(storage_key) BETWEEN 1 AND 512),
    receipt_token uuid,
    receipt_lease_expires_at timestamptz,
    promotion_key text CHECK (promotion_key IS NULL OR length(promotion_key) BETWEEN 1 AND 512),
    promotion_owned boolean NOT NULL DEFAULT false,
    sha256 bytea CHECK (sha256 IS NULL OR octet_length(sha256)=32),
    error_code text CHECK (error_code IS NULL OR error_code ~ '^[a-z][a-z0-9_]{0,63}$'),
    error_summary text CHECK (error_summary IS NULL OR length(error_summary) <= 512),
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL
    ,CHECK ((state='receiving')=(receipt_token IS NOT NULL AND receipt_lease_expires_at IS NOT NULL))
);
CREATE INDEX upload_sessions_library_state_idx ON folioharbor.upload_sessions(library_id,state,updated_at);

CREATE TABLE folioharbor.upload_cleanups (
 upload_id uuid NOT NULL REFERENCES folioharbor.upload_sessions(upload_id) ON DELETE CASCADE,
 attempt_token uuid NOT NULL, staging_key text NOT NULL CHECK(length(staging_key) BETWEEN 1 AND 512),
 final_key text CHECK(final_key IS NULL OR length(final_key) BETWEEN 1 AND 512),
 final_owned boolean NOT NULL, state text NOT NULL DEFAULT 'pending' CHECK(state IN('pending','leased','completed')),
 lease_owner text, lease_expires_at timestamptz, created_at timestamptz NOT NULL, completed_at timestamptz,
 PRIMARY KEY(upload_id,attempt_token),
 CHECK((state='leased')=(lease_owner IS NOT NULL AND lease_expires_at IS NOT NULL))
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

CREATE FUNCTION folioharbor.upload_create_authorized(
 p_upload uuid,p_library uuid,p_actor uuid,p_file text,p_media text,p_declared bigint,
 p_expires timestamptz,p_now timestamptz
) RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE library_row folioharbor.libraries%ROWTYPE;
BEGIN
 IF session_user <> 'folioharbor_api' OR p_actor IS DISTINCT FROM folioharbor.current_user_id()
    OR p_library IS DISTINCT FROM folioharbor.current_library_id() THEN RETURN 'not_found'; END IF;
 IF p_declared < 1 OR p_declared > 1073741824 OR p_file !~* '\.epub$'
    OR p_media NOT IN ('application/epub+zip','application/octet-stream') THEN RETURN 'invalid'; END IF;
 SELECT * INTO library_row FROM folioharbor.libraries WHERE library_id=p_library FOR UPDATE;
 IF library_row.library_id IS NULL THEN RETURN 'not_found'; END IF;
 IF NOT EXISTS(SELECT 1 FROM folioharbor.library_memberships m JOIN folioharbor.role_permissions p USING(role_code)
  WHERE m.library_id=p_library AND m.user_id=p_actor AND m.status='active' AND p.permission_code='holding.edit')
 THEN RETURN 'forbidden'; END IF;
 IF EXISTS(SELECT 1 FROM folioharbor.upload_sessions WHERE upload_id=p_upload) THEN RETURN 'conflict'; END IF;
 IF p_declared > library_row.quota_limit_bytes-library_row.quota_used_bytes-library_row.quota_reserved_bytes
 THEN RETURN 'quota_exceeded'; END IF;
 INSERT INTO folioharbor.upload_sessions(upload_id,library_id,created_by,file_name,media_type,declared_bytes,state,expires_at,created_at,updated_at)
 VALUES(p_upload,p_library,p_actor,p_file,p_media,p_declared,'created',p_expires,p_now,p_now);
 INSERT INTO folioharbor.quota_reservations(upload_id,library_id,reserved_bytes,expires_at,state,created_at,updated_at)
 VALUES(p_upload,p_library,p_declared,p_expires,'active',p_now,p_now);
 UPDATE folioharbor.libraries SET quota_reserved_bytes=quota_reserved_bytes+p_declared WHERE library_id=p_library;
 RETURN 'created';
END $$;
REVOKE ALL ON FUNCTION folioharbor.upload_create_authorized(uuid,uuid,uuid,text,text,bigint,timestamptz,timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.upload_create_authorized(uuid,uuid,uuid,text,text,bigint,timestamptz,timestamptz) TO folioharbor_api;

CREATE FUNCTION folioharbor.upload_transition_authorized(
 p_upload uuid,p_library uuid,p_actor uuid,p_from text,p_to text,p_received bigint,
 p_storage text,p_error text,p_now timestamptz
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
 IF NOT ((p_from='created' AND p_to='receiving') OR
   (p_from='receiving' AND p_to='failed') OR
   (p_from='failed' AND p_to='receiving')) THEN RETURN false; END IF;
 IF p_from='receiving' AND p_to='failed' AND p_storage IS DISTINCT FROM upload.storage_key THEN RETURN false; END IF;
 SELECT * INTO reservation FROM folioharbor.quota_reservations WHERE upload_id=p_upload FOR UPDATE;
 IF p_from='failed' AND p_to='receiving' THEN
   IF reservation.state<>'released'
    OR EXISTS(SELECT 1 FROM folioharbor.upload_cleanups WHERE upload_id=p_upload AND state<>'completed')
    OR upload.declared_bytes > (SELECT quota_limit_bytes-quota_used_bytes-quota_reserved_bytes FROM folioharbor.libraries WHERE library_id=p_library) THEN RETURN false; END IF;
   UPDATE folioharbor.quota_reservations SET state='active',reserved_bytes=upload.declared_bytes,updated_at=p_now WHERE upload_id=p_upload;
   UPDATE folioharbor.libraries SET quota_reserved_bytes=quota_reserved_bytes+upload.declared_bytes WHERE library_id=p_library;
 ELSIF p_to='failed' AND reservation.state='active' THEN
   UPDATE folioharbor.libraries SET quota_reserved_bytes=quota_reserved_bytes-reservation.reserved_bytes WHERE library_id=p_library;
   UPDATE folioharbor.quota_reservations SET state='released',updated_at=p_now WHERE upload_id=p_upload;
 END IF;
 IF p_to='receiving' THEN
  IF p_storage IS NULL THEN RETURN false; END IF;
  UPDATE folioharbor.upload_sessions SET state=p_to,received_bytes=p_received,storage_key=p_storage,
   receipt_token=gen_random_uuid(),receipt_lease_expires_at=p_now+interval '5 minutes',
   promotion_key=NULL,promotion_owned=false,error_code=NULL,updated_at=p_now WHERE upload_id=p_upload;
 ELSE
  UPDATE folioharbor.upload_sessions SET state=p_to,received_bytes=p_received,storage_key=NULL,
   receipt_token=NULL,receipt_lease_expires_at=NULL,promotion_key=NULL,promotion_owned=false,
   error_code=p_error,updated_at=p_now WHERE upload_id=p_upload;
 END IF;
 RETURN true;
END $$;
REVOKE ALL ON FUNCTION folioharbor.upload_transition_authorized(uuid,uuid,uuid,text,text,bigint,text,text,timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.upload_transition_authorized(uuid,uuid,uuid,text,text,bigint,text,text,timestamptz) TO folioharbor_api;

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
 p_upload uuid,p_library uuid,p_actor uuid,p_staging text,p_now timestamptz
) RETURNS boolean LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
BEGIN
 IF session_user <> 'folioharbor_api' OR p_actor IS DISTINCT FROM folioharbor.current_user_id()
  OR p_library IS DISTINCT FROM folioharbor.current_library_id() THEN RETURN false; END IF;
 UPDATE folioharbor.upload_sessions SET receipt_lease_expires_at=p_now+interval '5 minutes',updated_at=p_now
  WHERE upload_id=p_upload AND library_id=p_library AND state='receiving'
   AND storage_key=p_staging AND receipt_lease_expires_at>p_now;
 RETURN FOUND;
END $$;
REVOKE ALL ON FUNCTION folioharbor.upload_heartbeat_authorized(uuid,uuid,uuid,text,timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.upload_heartbeat_authorized(uuid,uuid,uuid,text,timestamptz) TO folioharbor_api;

CREATE FUNCTION folioharbor.upload_prepare_promotion_authorized(
 p_upload uuid,p_library uuid,p_actor uuid,p_staging text,p_final text,p_owned boolean,p_now timestamptz
) RETURNS boolean LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
BEGIN
 IF session_user <> 'folioharbor_api' OR p_actor IS DISTINCT FROM folioharbor.current_user_id()
  OR p_library IS DISTINCT FROM folioharbor.current_library_id() OR length(p_final)=0 THEN RETURN false; END IF;
 UPDATE folioharbor.upload_sessions SET promotion_key=p_final,promotion_owned=p_owned,updated_at=p_now
  WHERE upload_id=p_upload AND library_id=p_library AND state='receiving'
   AND storage_key=p_staging AND receipt_lease_expires_at>p_now;
 RETURN FOUND;
END $$;
REVOKE ALL ON FUNCTION folioharbor.upload_prepare_promotion_authorized(uuid,uuid,uuid,text,text,boolean,timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.upload_prepare_promotion_authorized(uuid,uuid,uuid,text,text,boolean,timestamptz) TO folioharbor_api;

CREATE FUNCTION folioharbor.upload_mark_received_authorized(
 p_upload uuid,p_library uuid,p_actor uuid,p_staging text,p_final text,p_received bigint,p_now timestamptz
) RETURNS boolean LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE upload folioharbor.upload_sessions%ROWTYPE;
DECLARE reservation folioharbor.quota_reservations%ROWTYPE;
BEGIN
 IF session_user <> 'folioharbor_api' OR p_actor IS DISTINCT FROM folioharbor.current_user_id()
  OR p_library IS DISTINCT FROM folioharbor.current_library_id() THEN RETURN false; END IF;
 PERFORM 1 FROM folioharbor.libraries WHERE library_id=p_library FOR UPDATE;
 SELECT * INTO upload FROM folioharbor.upload_sessions WHERE upload_id=p_upload AND library_id=p_library FOR UPDATE;
 IF upload.state<>'receiving' OR upload.storage_key IS DISTINCT FROM p_staging
  OR upload.promotion_key IS DISTINCT FROM p_final OR upload.receipt_lease_expires_at<=p_now
  OR p_received<0 OR p_received>upload.declared_bytes THEN RETURN false; END IF;
 SELECT * INTO reservation FROM folioharbor.quota_reservations WHERE upload_id=p_upload FOR UPDATE;
 IF reservation.state<>'active' THEN RETURN false; END IF;
 UPDATE folioharbor.libraries SET quota_reserved_bytes=quota_reserved_bytes-reservation.reserved_bytes+p_received
  WHERE library_id=p_library;
 UPDATE folioharbor.quota_reservations SET reserved_bytes=p_received,updated_at=p_now WHERE upload_id=p_upload;
 UPDATE folioharbor.upload_sessions SET state='received',received_bytes=p_received,storage_key=p_final,
  receipt_token=NULL,receipt_lease_expires_at=NULL,promotion_key=NULL,promotion_owned=false,error_code=NULL,updated_at=p_now
  WHERE upload_id=p_upload;
 RETURN true;
END $$;
REVOKE ALL ON FUNCTION folioharbor.upload_mark_received_authorized(uuid,uuid,uuid,text,text,bigint,timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.upload_mark_received_authorized(uuid,uuid,uuid,text,text,bigint,timestamptz) TO folioharbor_api;

CREATE FUNCTION folioharbor.upload_record_orphan_cleanup_authorized(
 p_upload uuid,p_library uuid,p_actor uuid,p_staging text,p_now timestamptz
) RETURNS void LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
BEGIN
 IF session_user <> 'folioharbor_api' OR p_actor IS DISTINCT FROM folioharbor.current_user_id()
  OR p_library IS DISTINCT FROM folioharbor.current_library_id() OR length(p_staging)=0
  OR NOT EXISTS(SELECT 1 FROM folioharbor.upload_sessions WHERE upload_id=p_upload AND library_id=p_library)
  OR NOT EXISTS(SELECT 1 FROM folioharbor.library_memberships m JOIN folioharbor.role_permissions p USING(role_code)
   WHERE m.library_id=p_library AND m.user_id=p_actor AND m.status='active' AND p.permission_code='holding.edit')
 THEN RETURN; END IF;
 INSERT INTO folioharbor.upload_cleanups(upload_id,attempt_token,staging_key,final_owned,created_at)
  VALUES(p_upload,gen_random_uuid(),p_staging,false,p_now);
END $$;
REVOKE ALL ON FUNCTION folioharbor.upload_record_orphan_cleanup_authorized(uuid,uuid,uuid,text,timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.upload_record_orphan_cleanup_authorized(uuid,uuid,uuid,text,timestamptz) TO folioharbor_api;

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
   OR upload.promotion_key IS DISTINCT FROM p_storage OR upload.receipt_lease_expires_at<=p_now) THEN RETURN false; END IF;
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
  receipt_token=NULL,receipt_lease_expires_at=NULL,promotion_key=NULL,promotion_owned=false,error_code=NULL,updated_at=p_now
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
  ORDER BY COALESCE(receipt_lease_expires_at,expires_at) LIMIT p_limit LOOP
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
     receipt_lease_expires_at=NULL,promotion_key=NULL,promotion_owned=false,error_code='receipt_expired',updated_at=p_now
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
ALTER TABLE folioharbor.background_jobs ENABLE ROW LEVEL SECURITY; ALTER TABLE folioharbor.background_jobs FORCE ROW LEVEL SECURITY;
ALTER TABLE folioharbor.job_attempts ENABLE ROW LEVEL SECURITY; ALTER TABLE folioharbor.job_attempts FORCE ROW LEVEL SECURITY;
CREATE POLICY jobs_owner_access ON folioharbor.background_jobs USING(current_user='folioharbor_owner') WITH CHECK(current_user='folioharbor_owner');
CREATE POLICY attempts_owner_access ON folioharbor.job_attempts USING(current_user='folioharbor_owner') WITH CHECK(current_user='folioharbor_owner');
CREATE POLICY jobs_worker_access ON folioharbor.background_jobs USING(folioharbor.is_worker()) WITH CHECK(folioharbor.is_worker());
CREATE POLICY attempts_worker_access ON folioharbor.job_attempts USING(folioharbor.is_worker()) WITH CHECK(folioharbor.is_worker());
REVOKE ALL ON folioharbor.background_jobs,folioharbor.job_attempts FROM PUBLIC;
GRANT SELECT,INSERT,UPDATE ON folioharbor.background_jobs,folioharbor.job_attempts TO folioharbor_worker;

UPDATE folioharbor.schema_metadata SET schema_version=9,applied_at=clock_timestamp() WHERE singleton;
