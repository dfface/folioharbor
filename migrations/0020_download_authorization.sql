ALTER TABLE folioharbor.libraries
    ADD COLUMN reader_download_enabled boolean NOT NULL DEFAULT false;

ALTER TABLE folioharbor.audit_events
    ADD COLUMN metadata jsonb NOT NULL DEFAULT '{}'::jsonb,
    ADD CONSTRAINT audit_events_metadata_object CHECK (jsonb_typeof(metadata) = 'object'),
    ADD CONSTRAINT audit_events_download_metadata CHECK (
        action_code <> 'item.download'
        OR decision = 'denied'
        OR (metadata ? 'range_start' AND metadata ? 'range_end'
            AND metadata - 'range_start' - 'range_end' = '{}'::jsonb)
    );
ALTER TABLE folioharbor.audit_events DROP CONSTRAINT audit_events_action_code_check;
ALTER TABLE folioharbor.audit_events ADD CONSTRAINT audit_events_action_code_check
    CHECK(action_code IN('library.view','library.manage','member.invite','member.role.change',
        'member.remove','publication.import','item.download'));
ALTER TABLE folioharbor.audit_events DROP CONSTRAINT audit_events_resource_type_check;
ALTER TABLE folioharbor.audit_events ADD CONSTRAINT audit_events_resource_type_check
    CHECK(resource_type IN('library','membership','invitation','upload','item'));
ALTER TABLE folioharbor.audit_events DROP CONSTRAINT audit_events_check;
ALTER TABLE folioharbor.audit_events ADD CONSTRAINT audit_events_check CHECK(
    (decision='allowed' AND reason_code IS NULL) OR
    (decision='denied' AND reason_code IN(
        'library_action_forbidden','library_not_found','item_download_forbidden')));

CREATE FUNCTION folioharbor.download_item_authorize(p_actor uuid,p_item uuid,p_request text)
RETURNS TABLE(
    outcome text,library_id uuid,item_id uuid,blob_id uuid,storage_key text,
    byte_size bigint,file_name text
) LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
#variable_conflict use_column
DECLARE candidate record;
DECLARE permitted boolean;
BEGIN
 IF session_user<>'folioharbor_api' OR p_actor IS DISTINCT FROM folioharbor.current_user_id()
    OR p_request IS DISTINCT FROM folioharbor.current_request_id() THEN
   RETURN QUERY SELECT 'not_found'::text,NULL::uuid,NULL::uuid,NULL::uuid,NULL::text,NULL::bigint,NULL::text;
   RETURN;
 END IF;
 SELECT holding.library_id,item.item_id,asset.blob_id,location.storage_key,blob.byte_size,
        upload.file_name,membership.role_code,library.reader_download_enabled
 INTO candidate
 FROM folioharbor.items item
 JOIN folioharbor.holdings holding ON holding.holding_id=item.holding_id
 JOIN folioharbor.libraries library ON library.library_id=holding.library_id
 JOIN folioharbor.library_memberships membership ON membership.library_id=holding.library_id
      AND membership.user_id=p_actor AND membership.status='active'
 JOIN folioharbor.item_assets asset ON asset.item_id=item.item_id AND asset.asset_kind='original'
 JOIN folioharbor.blobs blob ON blob.blob_id=asset.blob_id
 JOIN LATERAL (SELECT location.storage_key FROM folioharbor.blob_locations location
      WHERE location.blob_id=asset.blob_id AND location.state='ready'
      ORDER BY location.storage_key LIMIT 1) location ON true
 JOIN folioharbor.upload_sessions upload ON upload.upload_id=item.source_upload_id
 WHERE item.item_id=p_item AND item.state='active' AND holding.state='active';
 IF candidate.item_id IS NULL THEN
   RETURN QUERY SELECT 'not_found'::text,NULL::uuid,NULL::uuid,NULL::uuid,NULL::text,NULL::bigint,NULL::text;
   RETURN;
 END IF;
 permitted := candidate.role_code IN ('owner','editor') OR
     (candidate.role_code='reader' AND candidate.reader_download_enabled);
 IF NOT permitted THEN
   PERFORM pg_advisory_xact_lock(hashtextextended(p_actor::text||p_item::text,0));
   IF NOT EXISTS(SELECT 1 FROM folioharbor.audit_events event
       WHERE event.actor_id=p_actor AND event.resource_id=p_item
       AND event.action_code='item.download' AND event.decision='denied'
       AND event.occurred_at>clock_timestamp()-interval '1 minute') THEN
     INSERT INTO folioharbor.audit_events(
       audit_event_id,actor_id,effective_actor_id,library_id,action_code,resource_type,
       resource_id,decision,reason_code,request_id,source,occurred_at,network_hmac,metadata)
     VALUES(gen_random_uuid(),p_actor,p_actor,candidate.library_id,'item.download','item',p_item,
       'denied','item_download_forbidden',p_request,'api',clock_timestamp(),NULL,'{}');
   END IF;
   RETURN QUERY SELECT 'forbidden'::text,NULL::uuid,NULL::uuid,NULL::uuid,NULL::text,NULL::bigint,NULL::text;
   RETURN;
 END IF;
 RETURN QUERY SELECT 'granted'::text,candidate.library_id,candidate.item_id,candidate.blob_id,
      candidate.storage_key,candidate.byte_size,candidate.file_name;
END $$;

CREATE FUNCTION folioharbor.download_record_start(
 p_actor uuid,p_item uuid,p_request text,p_start bigint,p_end bigint,p_at timestamptz
) RETURNS boolean LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE candidate record;
BEGIN
 IF session_user<>'folioharbor_api' OR p_actor IS DISTINCT FROM folioharbor.current_user_id()
    OR p_request IS DISTINCT FROM folioharbor.current_request_id()
    OR p_start<0 OR p_end<p_start THEN RETURN false; END IF;
 SELECT holding.library_id,membership.role_code,library.reader_download_enabled,blob.byte_size
 INTO candidate FROM folioharbor.items item
 JOIN folioharbor.holdings holding ON holding.holding_id=item.holding_id
 JOIN folioharbor.libraries library ON library.library_id=holding.library_id
 JOIN folioharbor.library_memberships membership ON membership.library_id=holding.library_id
      AND membership.user_id=p_actor AND membership.status='active'
 JOIN folioharbor.item_assets asset ON asset.item_id=item.item_id AND asset.asset_kind='original'
 JOIN folioharbor.blobs blob ON blob.blob_id=asset.blob_id
 WHERE item.item_id=p_item AND item.state='active' AND holding.state='active';
 IF candidate.library_id IS NULL OR p_end>=candidate.byte_size OR NOT(
      candidate.role_code IN('owner','editor') OR
      (candidate.role_code='reader' AND candidate.reader_download_enabled)) THEN RETURN false; END IF;
 INSERT INTO folioharbor.audit_events(
   audit_event_id,actor_id,effective_actor_id,library_id,action_code,resource_type,resource_id,
   decision,reason_code,request_id,source,occurred_at,network_hmac,metadata)
 VALUES(gen_random_uuid(),p_actor,p_actor,candidate.library_id,'item.download','item',p_item,
   'allowed',NULL,p_request,'api',p_at,NULL,
   jsonb_build_object('range_start',p_start,'range_end',p_end));
 RETURN true;
END $$;

ALTER FUNCTION folioharbor.download_item_authorize(uuid,uuid,text) OWNER TO folioharbor_owner;
ALTER FUNCTION folioharbor.download_record_start(uuid,uuid,text,bigint,bigint,timestamptz) OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.download_item_authorize(uuid,uuid,text) FROM PUBLIC,folioharbor_worker;
REVOKE ALL ON FUNCTION folioharbor.download_record_start(uuid,uuid,text,bigint,bigint,timestamptz) FROM PUBLIC,folioharbor_worker;
GRANT EXECUTE ON FUNCTION folioharbor.download_item_authorize(uuid,uuid,text) TO folioharbor_api;
GRANT EXECUTE ON FUNCTION folioharbor.download_record_start(uuid,uuid,text,bigint,bigint,timestamptz) TO folioharbor_api;

CREATE FUNCTION folioharbor.library_update_reader_download_authorized(
 p_actor uuid,p_library uuid,p_enabled boolean,p_membership_version bigint,p_request text
) RETURNS boolean LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
BEGIN
 IF session_user<>'folioharbor_api' OR p_actor IS DISTINCT FROM folioharbor.current_user_id()
    OR p_library IS DISTINCT FROM folioharbor.current_library_id()
    OR p_request IS DISTINCT FROM folioharbor.current_request_id()
    OR NOT folioharbor.library_revalidate_grant(
      p_actor,p_library,'library.manage',p_membership_version) THEN RETURN false; END IF;
 UPDATE folioharbor.libraries SET reader_download_enabled=p_enabled,
   updated_at=clock_timestamp(),version=version+1 WHERE library_id=p_library;
 RETURN FOUND;
END $$;
ALTER FUNCTION folioharbor.library_update_reader_download_authorized(uuid,uuid,boolean,bigint,text) OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.library_update_reader_download_authorized(uuid,uuid,boolean,bigint,text) FROM PUBLIC,folioharbor_worker;
GRANT EXECUTE ON FUNCTION folioharbor.library_update_reader_download_authorized(uuid,uuid,boolean,bigint,text) TO folioharbor_api;

UPDATE folioharbor.schema_metadata SET schema_version=20,applied_at=clock_timestamp() WHERE singleton;
