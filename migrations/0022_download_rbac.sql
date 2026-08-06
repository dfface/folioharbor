CREATE FUNCTION folioharbor.catalog_item_projection_download_rbac_visible(
    p_actor uuid, p_library uuid, p_item uuid, p_membership_version bigint
) RETURNS TABLE(
    holding_id uuid,
    item_id uuid,
    package_id uuid,
    manifestation_id uuid,
    primary_title text,
    authors text[],
    languages text[],
    identifiers text[],
    media_type text,
    can_download boolean
)
LANGUAGE sql SECURITY DEFINER SET search_path TO '' AS $$
    SELECT visible.holding_id,
           visible.item_id,
           visible.package_id,
           visible.manifestation_id,
           visible.primary_title,
           visible.authors,
           visible.languages,
           visible.identifiers,
           visible.media_type,
           EXISTS (
               SELECT 1
               FROM folioharbor.library_memberships membership
               JOIN folioharbor.role_permissions permission USING(role_code)
               WHERE membership.library_id=p_library
                 AND membership.user_id=p_actor
                 AND membership.status='active'
                 AND membership.version=p_membership_version
                 AND permission.permission_code='item.download'
                 AND (membership.role_code<>'reader' OR visible.reader_download_enabled)
           )
    FROM folioharbor.catalog_item_projection_download_visible(
        p_actor, p_library, p_item, p_membership_version
    ) visible
$$;

ALTER FUNCTION folioharbor.catalog_item_projection_download_rbac_visible(uuid,uuid,uuid,bigint)
    OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.catalog_item_projection_download_rbac_visible(uuid,uuid,uuid,bigint)
    FROM PUBLIC,folioharbor_worker;
GRANT EXECUTE ON FUNCTION folioharbor.catalog_item_projection_download_rbac_visible(uuid,uuid,uuid,bigint)
    TO folioharbor_api;

CREATE OR REPLACE FUNCTION folioharbor.download_item_authorize(
    p_actor uuid,p_item uuid,p_request text
) RETURNS TABLE(
    outcome text,library_id uuid,item_id uuid,blob_id uuid,storage_key text,
    byte_size bigint,file_name text
) LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
#variable_conflict use_column
DECLARE candidate record;
DECLARE source record;
DECLARE membership_role text;
DECLARE has_download_permission boolean;
DECLARE permitted boolean;
BEGIN
 IF session_user<>'folioharbor_api' OR p_actor IS DISTINCT FROM folioharbor.current_user_id()
    OR p_request IS DISTINCT FROM folioharbor.current_request_id() THEN
   RETURN QUERY SELECT 'not_found'::text,NULL::uuid,NULL::uuid,NULL::uuid,NULL::text,NULL::bigint,NULL::text;
   RETURN;
 END IF;
 SELECT holding.library_id,item.item_id,library.reader_download_enabled
 INTO candidate
 FROM folioharbor.items item
 JOIN folioharbor.holdings holding ON holding.holding_id=item.holding_id
 JOIN folioharbor.libraries library ON library.library_id=holding.library_id
 WHERE item.item_id=p_item AND item.state='active' AND holding.state='active';
 IF candidate.item_id IS NULL THEN
   RETURN QUERY SELECT 'not_found'::text,NULL::uuid,NULL::uuid,NULL::uuid,NULL::text,NULL::bigint,NULL::text;
   RETURN;
 END IF;
 SELECT membership.role_code,
        EXISTS(SELECT 1 FROM folioharbor.role_permissions permission
          WHERE permission.role_code=membership.role_code
            AND permission.permission_code='item.download')
 INTO membership_role,has_download_permission
 FROM folioharbor.library_memberships membership
 WHERE membership.library_id=candidate.library_id AND membership.user_id=p_actor
   AND membership.status='active';
 permitted := COALESCE(has_download_permission,false) AND
     (membership_role<>'reader' OR candidate.reader_download_enabled);
 IF membership_role IS NULL OR NOT permitted THEN
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
   IF membership_role IS NULL THEN
     RETURN QUERY SELECT 'not_found'::text,NULL::uuid,NULL::uuid,NULL::uuid,NULL::text,NULL::bigint,NULL::text;
   ELSE
     RETURN QUERY SELECT 'forbidden'::text,NULL::uuid,NULL::uuid,NULL::uuid,NULL::text,NULL::bigint,NULL::text;
   END IF;
   RETURN;
 END IF;
 SELECT asset.blob_id,location.storage_key,blob.byte_size,upload.file_name
 INTO source
 FROM folioharbor.items item
 JOIN folioharbor.item_assets asset ON asset.item_id=item.item_id AND asset.asset_kind='original'
 JOIN folioharbor.blobs blob ON blob.blob_id=asset.blob_id
 JOIN LATERAL (SELECT location.storage_key FROM folioharbor.blob_locations location
      WHERE location.blob_id=asset.blob_id AND location.state='ready'
      ORDER BY location.storage_key LIMIT 1) location ON true
 JOIN folioharbor.upload_sessions upload ON upload.upload_id=item.source_upload_id
 WHERE item.item_id=candidate.item_id;
 IF source.blob_id IS NULL THEN
   RETURN QUERY SELECT 'not_found'::text,NULL::uuid,NULL::uuid,NULL::uuid,NULL::text,NULL::bigint,NULL::text;
   RETURN;
 END IF;
 RETURN QUERY SELECT 'granted'::text,candidate.library_id,candidate.item_id,source.blob_id,
      source.storage_key,source.byte_size,source.file_name;
END $$;

CREATE OR REPLACE FUNCTION folioharbor.download_record_start(
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
 JOIN folioharbor.role_permissions permission ON permission.role_code=membership.role_code
      AND permission.permission_code='item.download'
 JOIN folioharbor.item_assets asset ON asset.item_id=item.item_id AND asset.asset_kind='original'
 JOIN folioharbor.blobs blob ON blob.blob_id=asset.blob_id
 WHERE item.item_id=p_item AND item.state='active' AND holding.state='active';
 IF candidate.library_id IS NULL OR p_end>=candidate.byte_size OR
    (candidate.role_code='reader' AND NOT candidate.reader_download_enabled) THEN RETURN false; END IF;
 INSERT INTO folioharbor.audit_events(
   audit_event_id,actor_id,effective_actor_id,library_id,action_code,resource_type,resource_id,
   decision,reason_code,request_id,source,occurred_at,network_hmac,metadata)
 VALUES(gen_random_uuid(),p_actor,p_actor,candidate.library_id,'item.download','item',p_item,
   'allowed',NULL,p_request,'api',p_at,NULL,
   jsonb_build_object('range_start',p_start,'range_end',p_end));
 RETURN true;
END $$;

ALTER FUNCTION folioharbor.download_item_authorize(uuid,uuid,text) OWNER TO folioharbor_owner;
ALTER FUNCTION folioharbor.download_record_start(uuid,uuid,text,bigint,bigint,timestamptz)
    OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.download_item_authorize(uuid,uuid,text)
    FROM PUBLIC,folioharbor_worker;
REVOKE ALL ON FUNCTION folioharbor.download_record_start(uuid,uuid,text,bigint,bigint,timestamptz)
    FROM PUBLIC,folioharbor_worker;
GRANT EXECUTE ON FUNCTION folioharbor.download_item_authorize(uuid,uuid,text)
    TO folioharbor_api;
GRANT EXECUTE ON FUNCTION folioharbor.download_record_start(uuid,uuid,text,bigint,bigint,timestamptz)
    TO folioharbor_api;

UPDATE folioharbor.schema_metadata
SET schema_version=22,applied_at=clock_timestamp()
WHERE singleton;
