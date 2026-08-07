CREATE OR REPLACE FUNCTION folioharbor.catalog_finish_import(
    p_library uuid, p_upload uuid, p_actor uuid, p_blob uuid, p_logical bigint,
    p_profile text, p_item uuid,
    p_audit uuid, p_request text, p_now timestamptz
) RETURNS text LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE quota_outcome text;
DECLARE completion_outcome text;
BEGIN
    IF NOT folioharbor.catalog_validate_import(
      p_library,p_upload,p_actor,p_blob,p_logical,p_request
    ) THEN RETURN 'not_active'; END IF;
    SELECT CASE WHEN item.source_upload_id=p_upload THEN 'created' ELSE 'duplicate' END
      INTO completion_outcome
      FROM folioharbor.items item
      JOIN folioharbor.holdings holding
        ON holding.holding_id=item.holding_id
       AND holding.manifestation_id=item.manifestation_id
      JOIN folioharbor.publication_packages package
        ON package.package_id=item.package_id
       AND package.manifestation_id=item.manifestation_id
      JOIN folioharbor.item_assets asset
        ON asset.item_id=item.item_id AND asset.asset_kind='original'
      JOIN folioharbor.upload_sessions provenance
        ON provenance.upload_id=item.source_upload_id
      JOIN folioharbor.blobs blob ON blob.blob_id=p_blob
      WHERE item.item_id=p_item AND item.state='active' AND holding.state='active'
        AND holding.library_id=p_library AND package.blob_id=p_blob
        AND package.parser_profile_version=p_profile AND asset.blob_id=p_blob
        AND provenance.library_id=p_library
        AND provenance.sha256 IS NOT DISTINCT FROM blob.sha256
        AND provenance.received_bytes=blob.byte_size
        AND ((item.source_upload_id=p_upload AND provenance.state='importing')
          OR (item.source_upload_id<>p_upload AND provenance.state='ready'))
      FOR UPDATE OF item;
    IF completion_outcome IS NULL THEN RETURN 'not_active'; END IF;
    quota_outcome := CASE WHEN completion_outcome='duplicate'
      THEN folioharbor.quota_release(p_library,p_upload)
      ELSE folioharbor.quota_consume(p_library,p_upload) END;
    IF quota_outcome <> 'applied' THEN RETURN 'not_active'; END IF;
    UPDATE folioharbor.upload_sessions SET
      state=CASE WHEN completion_outcome='duplicate' THEN 'duplicate' ELSE 'ready' END,
      result_item_id=p_item,
      updated_at=p_now WHERE upload_id=p_upload;
    DELETE FROM folioharbor.blob_reachability_candidates
      WHERE source_upload_id=p_upload;
    INSERT INTO folioharbor.audit_events(
      audit_event_id,actor_id,effective_actor_id,library_id,action_code,resource_type,
      resource_id,decision,reason_code,request_id,source,occurred_at,network_hmac
    ) VALUES (p_audit,p_actor,p_actor,p_library,'publication.import','upload',p_upload,
      'allowed',NULL,p_request,'worker',p_now,NULL);
    RETURN completion_outcome;
END $$;

ALTER FUNCTION folioharbor.catalog_finish_import(
    uuid,uuid,uuid,uuid,bigint,text,uuid,uuid,text,timestamptz
) OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.catalog_finish_import(
    uuid,uuid,uuid,uuid,bigint,text,uuid,uuid,text,timestamptz
) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.catalog_finish_import(
    uuid,uuid,uuid,uuid,bigint,text,uuid,uuid,text,timestamptz
) TO folioharbor_worker;

UPDATE folioharbor.schema_metadata
   SET schema_version = 28, applied_at = clock_timestamp()
 WHERE singleton;
