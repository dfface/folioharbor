-- Runtime storage policies are deployment inputs. Persist decisions made from those
-- inputs and remove superseded actor-parameterized API entry points.

ALTER TABLE folioharbor.upload_sessions
    DROP CONSTRAINT upload_sessions_declared_bytes_check,
    ADD CONSTRAINT upload_sessions_declared_bytes_check CHECK (declared_bytes >= 1);

CREATE OR REPLACE FUNCTION folioharbor.upload_create_authorized(
 p_upload uuid,p_library uuid,p_actor uuid,p_file text,p_media text,p_declared bigint,p_scope text,
 p_expires timestamptz,p_now timestamptz
) RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE library_row folioharbor.libraries%ROWTYPE;
BEGIN
 IF session_user <> 'folioharbor_api' OR p_actor IS DISTINCT FROM folioharbor.current_user_id()
    OR p_library IS DISTINCT FROM folioharbor.current_library_id() THEN RETURN 'not_found'; END IF;
 IF p_declared < 1 OR p_file !~* '\.epub$'
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

CREATE FUNCTION folioharbor.library_provision_personal_configured(
 p_library_id uuid,p_user_id uuid,p_now timestamptz,p_quota_limit bigint
) RETURNS TABLE(library_id uuid,name text) LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
#variable_conflict use_column
DECLARE found_id uuid; found_name text;
BEGIN
 IF session_user<>'folioharbor_api'
    OR p_user_id IS DISTINCT FROM folioharbor.current_user_id()
    OR folioharbor.current_library_id() IS NOT NULL
    OR p_quota_limit<1 THEN
   RAISE EXCEPTION 'personal library request context is invalid' USING ERRCODE='22023';
 END IF;
 INSERT INTO folioharbor.libraries(
   library_id,personal_owner_id,name,quota_limit_bytes,created_at,updated_at
 ) VALUES(p_library_id,p_user_id,'Personal Library',p_quota_limit,p_now,p_now)
 ON CONFLICT(personal_owner_id) WHERE personal_owner_id IS NOT NULL DO NOTHING;
 SELECT l.library_id,l.name INTO found_id,found_name
 FROM folioharbor.libraries l WHERE l.personal_owner_id=p_user_id;
 INSERT INTO folioharbor.library_memberships(library_id,user_id,role_code,status,joined_at)
 VALUES(found_id,p_user_id,'owner','active',p_now)
 ON CONFLICT(library_id,user_id) WHERE status='active' DO NOTHING;
 RETURN QUERY SELECT found_id,found_name;
END $$;
ALTER FUNCTION folioharbor.library_provision_personal_configured(uuid,uuid,timestamptz,bigint)
    OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.library_provision_personal_configured(uuid,uuid,timestamptz,bigint)
    FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.library_provision_personal_configured(uuid,uuid,timestamptz,bigint)
    TO folioharbor_api;

CREATE FUNCTION folioharbor.library_members_web_visible(
 p_actor uuid,p_library uuid,p_version bigint
) RETURNS TABLE(user_id uuid,role_code text) LANGUAGE sql SECURITY DEFINER SET search_path TO '' AS $$
 SELECT members.user_id,members.role_code
 FROM folioharbor.library_memberships members
 WHERE session_user='folioharbor_api'
   AND p_actor IS NOT DISTINCT FROM folioharbor.current_user_id()
   AND p_library IS NOT DISTINCT FROM folioharbor.current_library_id()
   AND members.library_id=p_library AND members.status='active'
   AND folioharbor.library_revalidate_grant(p_actor,p_library,'holding.view',p_version)
 ORDER BY members.user_id
$$;
ALTER FUNCTION folioharbor.library_members_web_visible(uuid,uuid,bigint)
    OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.library_members_web_visible(uuid,uuid,bigint) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.library_members_web_visible(uuid,uuid,bigint)
    TO folioharbor_api;

REVOKE EXECUTE ON FUNCTION
 folioharbor.library_provision_personal(uuid,uuid,timestamptz),
 folioharbor.library_accept_invitation(uuid,bytea,timestamptz),
 folioharbor.library_list_visible(uuid),
 folioharbor.library_get_visible(uuid,uuid,bigint),
 folioharbor.library_members_visible(uuid,uuid,bigint)
FROM folioharbor_api;
DROP FUNCTION folioharbor.library_provision_personal(uuid,uuid,timestamptz);
DROP FUNCTION folioharbor.library_accept_invitation(uuid,bytea,timestamptz);
DROP FUNCTION folioharbor.library_list_visible(uuid);
DROP FUNCTION folioharbor.library_get_visible(uuid,uuid,bigint);
DROP FUNCTION folioharbor.library_members_visible(uuid,uuid,bigint);

ALTER TABLE folioharbor.items DROP CONSTRAINT items_lifecycle_timestamps;
ALTER TABLE folioharbor.items ADD CONSTRAINT items_lifecycle_timestamps CHECK (
 (state='active' AND deleted_at IS NULL AND purge_eligible_at IS NULL AND purged_at IS NULL)
 OR (state IN('deleted','purge_eligible') AND deleted_at IS NOT NULL
     AND purge_eligible_at>=deleted_at AND purged_at IS NULL)
 OR (state='purged' AND deleted_at IS NOT NULL
     AND purge_eligible_at>=deleted_at AND purged_at>=purge_eligible_at)
);

ALTER TABLE folioharbor.blob_locations DROP CONSTRAINT blob_locations_lifecycle_timestamps;
ALTER TABLE folioharbor.blob_locations ADD CONSTRAINT blob_locations_lifecycle_timestamps CHECK (
 (state IN('staging','ready','quarantined') AND purge_pending_at IS NULL
   AND purge_after IS NULL AND purged_at IS NULL AND purge_lease_owner IS NULL
   AND purge_lease_token IS NULL AND purge_lease_expires_at IS NULL)
 OR (state='purge_pending' AND purge_pending_at IS NOT NULL
   AND purge_after>=purge_pending_at AND purged_at IS NULL AND purge_lease_owner IS NULL
   AND purge_lease_token IS NULL AND purge_lease_expires_at IS NULL)
 OR (state='deleting' AND purge_pending_at IS NOT NULL
   AND purge_after>=purge_pending_at AND purged_at IS NULL AND purge_lease_owner IS NOT NULL
   AND purge_lease_token IS NOT NULL AND purge_lease_expires_at IS NOT NULL)
 OR (state='purged' AND purge_pending_at IS NOT NULL
   AND purge_after>=purge_pending_at AND purged_at>=purge_after AND purge_lease_owner IS NULL
   AND purge_lease_token IS NULL AND purge_lease_expires_at IS NULL)
);

CREATE OR REPLACE FUNCTION folioharbor.blob_location_fill_legacy_purge_timestamps()
RETURNS trigger LANGUAGE plpgsql SET search_path TO '' AS $$
BEGIN
 IF NEW.state IN('ready','quarantined') THEN
  NEW.purge_pending_at:=NULL; NEW.purge_after:=NULL; NEW.purged_at:=NULL;
  NEW.purge_lease_owner:=NULL; NEW.purge_lease_token:=NULL; NEW.purge_lease_expires_at:=NULL;
 ELSIF NEW.state='purged' AND NEW.purged_at IS NULL THEN
  NEW.purge_after:=COALESCE(NEW.purge_after,NEW.updated_at);
  NEW.purge_pending_at:=COALESCE(NEW.purge_pending_at,NEW.purge_after);
  NEW.purged_at:=NEW.updated_at;
  NEW.purge_lease_owner:=NULL; NEW.purge_lease_token:=NULL; NEW.purge_lease_expires_at:=NULL;
 END IF;
 RETURN NEW;
END $$;

CREATE FUNCTION folioharbor.item_lifecycle_mutate_authorized(
 p_actor uuid,p_library uuid,p_item uuid,p_operation text,p_now timestamptz,
 p_membership_version bigint,p_request text,p_recovery_seconds bigint
) RETURNS TABLE(outcome text,item_state text,item_deleted_at timestamptz,
 item_purge_eligible_at timestamptz,item_purged_at timestamptz)
LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE current_item folioharbor.items%ROWTYPE; expected_action text;
BEGIN
 IF session_user<>'folioharbor_api' OR p_actor IS DISTINCT FROM folioharbor.current_user_id()
    OR p_library IS DISTINCT FROM folioharbor.current_library_id()
    OR p_request IS DISTINCT FROM folioharbor.current_request_id()
    OR p_operation NOT IN('delete','restore') OR p_recovery_seconds<1 THEN
  RETURN QUERY SELECT 'not_found'::text,NULL::text,NULL::timestamptz,NULL::timestamptz,NULL::timestamptz; RETURN;
 END IF;
 SELECT item.* INTO current_item FROM folioharbor.items item
 JOIN folioharbor.holdings holding USING(holding_id)
 WHERE item.item_id=p_item AND holding.library_id=p_library FOR UPDATE OF item;
 IF current_item.item_id IS NULL THEN
  RETURN QUERY SELECT 'not_found'::text,NULL::text,NULL::timestamptz,NULL::timestamptz,NULL::timestamptz; RETURN;
 END IF;
 IF NOT folioharbor.library_revalidate_grant(p_actor,p_library,'holding.edit',p_membership_version) THEN
  RETURN QUERY SELECT 'forbidden'::text,NULL::text,NULL::timestamptz,NULL::timestamptz,NULL::timestamptz; RETURN;
 END IF;
 IF p_operation='delete' THEN
  IF current_item.state='active' THEN
   UPDATE folioharbor.items SET state='deleted',deleted_at=p_now,
    purge_eligible_at=p_now+make_interval(secs=>p_recovery_seconds::double precision),purged_at=NULL
   WHERE item_id=p_item RETURNING * INTO current_item;
  END IF;
  expected_action:='item.delete';
 ELSE
  IF current_item.state='deleted' AND p_now<current_item.purge_eligible_at THEN
   UPDATE folioharbor.items SET state='active',deleted_at=NULL,purge_eligible_at=NULL,purged_at=NULL
   WHERE item_id=p_item RETURNING * INTO current_item;
  ELSIF current_item.state<>'active' THEN
   RETURN QUERY SELECT 'window_elapsed'::text,current_item.state,current_item.deleted_at,
    current_item.purge_eligible_at,current_item.purged_at; RETURN;
  END IF;
  expected_action:='item.restore';
 END IF;
 PERFORM folioharbor.audit_record_allowed(gen_random_uuid(),p_actor,p_actor,p_library,
  expected_action,'item',p_item,'allowed',NULL,p_request,'api',p_now,NULL,
  expected_action,'item',p_item);
 RETURN QUERY SELECT 'applied'::text,current_item.state,current_item.deleted_at,
  current_item.purge_eligible_at,current_item.purged_at;
END $$;
ALTER FUNCTION folioharbor.item_lifecycle_mutate_authorized(uuid,uuid,uuid,text,timestamptz,bigint,text,bigint)
 OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.item_lifecycle_mutate_authorized(uuid,uuid,uuid,text,timestamptz,bigint,text,bigint)
 FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.item_lifecycle_mutate_authorized(uuid,uuid,uuid,text,timestamptz,bigint,text,bigint)
 TO folioharbor_api;
REVOKE EXECUTE ON FUNCTION folioharbor.item_lifecycle_mutate_authorized(uuid,uuid,uuid,text,timestamptz,bigint,text)
 FROM folioharbor_api;
DROP FUNCTION folioharbor.item_lifecycle_mutate_authorized(uuid,uuid,uuid,text,timestamptz,bigint,text);

CREATE FUNCTION folioharbor.gc_prepare_items_worker(p_now timestamptz,p_limit bigint,p_gc_delay_seconds bigint)
RETURNS bigint LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE candidate record; candidate_blob uuid; candidate_blobs uuid[]; logical_bytes bigint; processed bigint:=0;
BEGIN
 IF session_user<>'folioharbor_worker' OR NOT folioharbor.is_worker()
    OR p_limit<1 OR p_limit>1000 OR p_gc_delay_seconds<1 THEN RETURN 0; END IF;
 FOR candidate IN SELECT item.item_id,item.package_id,item.manifestation_id,holding.library_id
  FROM folioharbor.items item JOIN folioharbor.holdings holding USING(holding_id)
  WHERE item.state='deleted' AND item.purge_eligible_at<=p_now
  ORDER BY item.purge_eligible_at,item.item_id LIMIT p_limit FOR UPDATE OF item SKIP LOCKED
 LOOP
  SELECT COALESCE(array_agg(DISTINCT reference.blob_id),ARRAY[]::uuid[]) INTO candidate_blobs FROM (
   SELECT asset.blob_id FROM folioharbor.item_assets asset WHERE asset.item_id=candidate.item_id
   UNION SELECT package.blob_id FROM folioharbor.publication_packages package WHERE package.package_id=candidate.package_id
   UNION SELECT resource.source_blob_id FROM folioharbor.publication_resources resource WHERE resource.package_id=candidate.package_id
   UNION SELECT asset.blob_id FROM folioharbor.manifestation_assets asset WHERE asset.manifestation_id=candidate.manifestation_id
  ) reference;
  PERFORM 1 FROM folioharbor.libraries WHERE library_id=candidate.library_id FOR UPDATE;
  PERFORM blob.blob_id FROM folioharbor.blobs blob WHERE blob.blob_id=ANY(candidate_blobs)
   ORDER BY blob.blob_id FOR UPDATE;
  SELECT COALESCE(sum(blob.byte_size),0) INTO logical_bytes FROM folioharbor.item_assets asset
   JOIN folioharbor.blobs blob USING(blob_id) WHERE asset.item_id=candidate.item_id;
  IF (SELECT quota_used_bytes FROM folioharbor.libraries WHERE library_id=candidate.library_id)<logical_bytes
   THEN RAISE EXCEPTION 'logical quota underflow' USING ERRCODE='23514'; END IF;
  UPDATE folioharbor.items SET state='purge_eligible' WHERE item_id=candidate.item_id;
  UPDATE folioharbor.libraries SET quota_used_bytes=quota_used_bytes-logical_bytes,updated_at=p_now
   WHERE library_id=candidate.library_id;
  DELETE FROM folioharbor.item_assets WHERE item_id=candidate.item_id;
  UPDATE folioharbor.items SET package_id=NULL WHERE item_id=candidate.item_id;
  IF candidate.package_id IS NOT NULL AND NOT EXISTS(
   SELECT 1 FROM folioharbor.items WHERE package_id=candidate.package_id) THEN
   DELETE FROM folioharbor.manifestation_assets WHERE manifestation_id=candidate.manifestation_id;
   DELETE FROM folioharbor.publication_packages WHERE package_id=candidate.package_id;
  END IF;
  UPDATE folioharbor.items SET state='purged',purged_at=p_now WHERE item_id=candidate.item_id;
  FOREACH candidate_blob IN ARRAY candidate_blobs LOOP
   IF NOT folioharbor.blob_has_authoritative_reference(candidate_blob) THEN
    UPDATE folioharbor.blob_locations SET state='purge_pending',purge_pending_at=p_now,
     purge_after=p_now+make_interval(secs=>p_gc_delay_seconds::double precision),purged_at=NULL,
     purge_lease_owner=NULL,purge_lease_token=NULL,purge_lease_expires_at=NULL,updated_at=p_now
    WHERE blob_id=candidate_blob AND state='ready';
   END IF;
  END LOOP;
  processed:=processed+1;
 END LOOP;
 RETURN processed;
END $$;
ALTER FUNCTION folioharbor.gc_prepare_items_worker(timestamptz,bigint,bigint) OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.gc_prepare_items_worker(timestamptz,bigint,bigint) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.gc_prepare_items_worker(timestamptz,bigint,bigint) TO folioharbor_worker;
REVOKE EXECUTE ON FUNCTION folioharbor.gc_prepare_items_worker(timestamptz,bigint) FROM folioharbor_worker;
DROP FUNCTION folioharbor.gc_prepare_items_worker(timestamptz,bigint);

CREATE FUNCTION folioharbor.import_expire_abandoned_worker(
 p_boundary timestamptz,p_limit bigint,p_retention_seconds bigint
) RETURNS bigint LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE candidate record; reservation folioharbor.quota_reservations%ROWTYPE; expired bigint:=0;
BEGIN
 IF session_user<>'folioharbor_worker' OR NOT folioharbor.is_worker()
    OR p_limit<1 OR p_limit>1000 OR p_retention_seconds<1 THEN RETURN 0; END IF;
 FOR candidate IN SELECT upload_id,library_id,state,storage_key,dedup_scope,sha256,received_bytes
  FROM folioharbor.upload_sessions WHERE state IN('created','received') AND expires_at<=p_boundary
  ORDER BY expires_at,upload_id LIMIT p_limit FOR UPDATE SKIP LOCKED LOOP
  PERFORM 1 FROM folioharbor.libraries WHERE library_id=candidate.library_id FOR UPDATE;
  SELECT * INTO reservation FROM folioharbor.quota_reservations WHERE upload_id=candidate.upload_id FOR UPDATE;
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
     ON CONFLICT ON CONSTRAINT blob_locations_storage_key_key DO UPDATE
      SET state='quarantined',updated_at=p_boundary
     WHERE folioharbor.blob_locations.blob_id=EXCLUDED.blob_id;
   END IF;
   INSERT INTO folioharbor.failed_upload_purges(upload_id,storage_key,delete_file,eligible_at,created_at,updated_at)
    VALUES(candidate.upload_id,candidate.storage_key,candidate.dedup_scope='disabled',
     p_boundary+make_interval(secs=>p_retention_seconds::double precision),p_boundary,p_boundary)
    ON CONFLICT(upload_id) DO NOTHING;
   UPDATE folioharbor.upload_sessions SET state='failed',error_code='received_expired',updated_at=p_boundary
    WHERE upload_id=candidate.upload_id;
  ELSE
   UPDATE folioharbor.upload_sessions SET state='expired',error_code='upload_expired',updated_at=p_boundary
    WHERE upload_id=candidate.upload_id;
  END IF;
  expired:=expired+1;
 END LOOP;
 RETURN expired;
END $$;
ALTER FUNCTION folioharbor.import_expire_abandoned_worker(timestamptz,bigint,bigint) OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.import_expire_abandoned_worker(timestamptz,bigint,bigint) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.import_expire_abandoned_worker(timestamptz,bigint,bigint) TO folioharbor_worker;
REVOKE EXECUTE ON FUNCTION folioharbor.import_expire_abandoned_worker(timestamptz,bigint) FROM folioharbor_worker;
DROP FUNCTION folioharbor.import_expire_abandoned_worker(timestamptz,bigint);

CREATE FUNCTION folioharbor.import_record_failure_worker(
 p_upload uuid,p_library uuid,p_to text,p_error text,p_now timestamptz,p_retention_seconds bigint
) RETURNS boolean LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE upload folioharbor.upload_sessions%ROWTYPE; reservation folioharbor.quota_reservations%ROWTYPE;
BEGIN
 IF session_user<>'folioharbor_worker' OR NOT folioharbor.is_worker()
    OR p_library IS DISTINCT FROM folioharbor.current_library_id()
    OR p_to NOT IN('failed','retry_wait') OR p_retention_seconds<1 THEN RETURN false; END IF;
 PERFORM 1 FROM folioharbor.libraries WHERE library_id=p_library FOR UPDATE;
 SELECT * INTO upload FROM folioharbor.upload_sessions
  WHERE upload_id=p_upload AND library_id=p_library FOR UPDATE;
 IF upload.upload_id IS NULL OR upload.state NOT IN('validating','importing') THEN RETURN false; END IF;
 IF p_to='failed' THEN
  SELECT * INTO reservation FROM folioharbor.quota_reservations WHERE upload_id=p_upload FOR UPDATE;
  IF reservation.state='active' THEN
   UPDATE folioharbor.libraries SET quota_reserved_bytes=quota_reserved_bytes-reservation.reserved_bytes
    WHERE library_id=p_library;
   UPDATE folioharbor.quota_reservations SET state='released',updated_at=p_now WHERE upload_id=p_upload;
  END IF;
  INSERT INTO folioharbor.failed_upload_purges(upload_id,storage_key,delete_file,eligible_at,created_at,updated_at)
   VALUES(p_upload,upload.storage_key,upload.dedup_scope='disabled',
    p_now+make_interval(secs=>p_retention_seconds::double precision),p_now,p_now)
   ON CONFLICT(upload_id) DO NOTHING;
  IF upload.dedup_scope='disabled' THEN
   UPDATE folioharbor.blob_locations SET state='quarantined',updated_at=p_now WHERE storage_key=upload.storage_key;
  END IF;
 END IF;
 UPDATE folioharbor.upload_sessions SET state=p_to,error_code=p_error,updated_at=p_now WHERE upload_id=p_upload;
 RETURN true;
END $$;
ALTER FUNCTION folioharbor.import_record_failure_worker(uuid,uuid,text,text,timestamptz,bigint)
 OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.import_record_failure_worker(uuid,uuid,text,text,timestamptz,bigint)
 FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.import_record_failure_worker(uuid,uuid,text,text,timestamptz,bigint)
 TO folioharbor_worker;
REVOKE EXECUTE ON FUNCTION folioharbor.import_record_failure_worker(uuid,uuid,text,text,timestamptz)
 FROM folioharbor_worker;
DROP FUNCTION folioharbor.import_record_failure_worker(uuid,uuid,text,text,timestamptz);

CREATE FUNCTION folioharbor.import_reconcile_worker(
 p_upload uuid,p_library uuid,p_blob_candidate uuid,p_request text,p_now timestamptz,p_retention_seconds bigint
) RETURNS TABLE(outcome text,actor_id uuid,blob_id uuid,logical_bytes bigint,
 storage_key text,upload_state text,error_code text)
LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE upload folioharbor.upload_sessions%ROWTYPE;
DECLARE reservation folioharbor.quota_reservations%ROWTYPE;
DECLARE resolved_blob uuid;
DECLARE namespace text;
BEGIN
 IF session_user<>'folioharbor_worker' OR NOT folioharbor.is_worker()
    OR p_library IS DISTINCT FROM folioharbor.current_library_id()
    OR p_request IS DISTINCT FROM folioharbor.current_request_id()
    OR p_retention_seconds<1 THEN RETURN; END IF;
 PERFORM 1 FROM folioharbor.libraries WHERE library_id=p_library FOR UPDATE;
 SELECT * INTO upload FROM folioharbor.upload_sessions
  WHERE upload_id=p_upload AND library_id=p_library FOR UPDATE;
 IF upload.upload_id IS NULL THEN RETURN; END IF;
 IF upload.state IN('ready','duplicate') THEN
  RETURN QUERY SELECT 'complete'::text,upload.created_by,NULL::uuid,upload.received_bytes,
   upload.storage_key,upload.state,NULL::text; RETURN;
 END IF;
 IF upload.state='operator_required' THEN
  RETURN QUERY SELECT 'operator_required'::text,upload.created_by,NULL::uuid,upload.received_bytes,
   upload.storage_key,upload.state,upload.error_code; RETURN;
 END IF;
 IF upload.state='failed' AND upload.storage_key IS NOT NULL THEN
  SELECT * INTO reservation FROM folioharbor.quota_reservations WHERE upload_id=p_upload FOR UPDATE;
  IF reservation.state='active' THEN
   UPDATE folioharbor.libraries SET quota_reserved_bytes=quota_reserved_bytes-reservation.reserved_bytes
    WHERE library_id=p_library;
   UPDATE folioharbor.quota_reservations SET state='released',updated_at=p_now WHERE upload_id=p_upload;
  END IF;
  INSERT INTO folioharbor.failed_upload_purges(upload_id,storage_key,delete_file,eligible_at,created_at,updated_at)
   VALUES(p_upload,upload.storage_key,upload.dedup_scope='disabled',
    p_now+make_interval(secs=>p_retention_seconds::double precision),p_now,p_now)
   ON CONFLICT(upload_id) DO NOTHING;
  IF upload.dedup_scope='disabled' THEN
   UPDATE folioharbor.blob_locations location SET state='quarantined',updated_at=p_now,
    purge_pending_at=NULL,purge_after=NULL,purged_at=NULL,purge_lease_owner=NULL,
    purge_lease_token=NULL,purge_lease_expires_at=NULL
    WHERE location.storage_key=upload.storage_key;
  END IF;
  RETURN QUERY SELECT 'failed'::text,upload.created_by,NULL::uuid,upload.received_bytes,
   upload.storage_key,upload.state,upload.error_code; RETURN;
 END IF;
 IF upload.state='retry_wait' THEN
  UPDATE folioharbor.upload_sessions SET state='queued',error_code=NULL,updated_at=p_now
   WHERE upload_id=p_upload;
  upload.state:='queued';
 END IF;
 IF upload.state='queued' THEN
  UPDATE folioharbor.upload_sessions SET state='validating',updated_at=p_now WHERE upload_id=p_upload;
  upload.state:='validating';
 END IF;
 IF upload.state NOT IN('validating','importing') OR upload.sha256 IS NULL
    OR upload.storage_key IS NULL OR upload.received_bytes<1 THEN RETURN; END IF;
 namespace:=CASE upload.dedup_scope
  WHEN 'instance' THEN 'instance-v1'
  WHEN 'library' THEN 'library-'||replace(p_library::text,'-','')
  WHEN 'disabled' THEN 'upload-'||replace(p_upload::text,'-','') END;
 INSERT INTO folioharbor.blobs(blob_id,storage_namespace,sha256,byte_size,created_at)
  VALUES(p_blob_candidate,namespace,upload.sha256,upload.received_bytes,p_now)
  ON CONFLICT(storage_namespace,sha256,byte_size) DO NOTHING;
 SELECT candidate.blob_id INTO resolved_blob FROM folioharbor.blobs candidate
  WHERE candidate.storage_namespace=namespace AND candidate.sha256=upload.sha256
   AND candidate.byte_size=upload.received_bytes FOR KEY SHARE;
 IF resolved_blob IS NULL THEN RETURN; END IF;
 INSERT INTO folioharbor.blob_locations(blob_id,storage_key,state,created_at,updated_at)
  VALUES(resolved_blob,upload.storage_key,'ready',p_now,p_now)
  ON CONFLICT ON CONSTRAINT blob_locations_storage_key_key DO UPDATE SET state='ready',
   updated_at=p_now,purge_pending_at=NULL,purge_after=NULL,purged_at=NULL,
   purge_lease_owner=NULL,purge_lease_token=NULL,purge_lease_expires_at=NULL
  WHERE folioharbor.blob_locations.blob_id=EXCLUDED.blob_id;
 IF NOT FOUND THEN RETURN; END IF;
 RETURN QUERY SELECT 'work'::text,upload.created_by,resolved_blob,upload.received_bytes,
  upload.storage_key,upload.state,NULL::text;
END $$;
ALTER FUNCTION folioharbor.import_reconcile_worker(uuid,uuid,uuid,text,timestamptz,bigint)
 OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.import_reconcile_worker(uuid,uuid,uuid,text,timestamptz,bigint)
 FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.import_reconcile_worker(uuid,uuid,uuid,text,timestamptz,bigint)
 TO folioharbor_worker;
REVOKE EXECUTE ON FUNCTION folioharbor.import_reconcile_worker(uuid,uuid,uuid,text,timestamptz)
 FROM folioharbor_worker;
DROP FUNCTION folioharbor.import_reconcile_worker(uuid,uuid,uuid,text,timestamptz);

REVOKE EXECUTE ON FUNCTION folioharbor.import_schedule_failed_purge_worker(uuid,uuid,timestamptz)
 FROM folioharbor_worker;
DROP FUNCTION folioharbor.import_schedule_failed_purge_worker(uuid,uuid,timestamptz);

UPDATE folioharbor.schema_metadata SET schema_version=29,applied_at=clock_timestamp()
WHERE singleton;
