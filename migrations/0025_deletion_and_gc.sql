ALTER TABLE folioharbor.items
    DROP CONSTRAINT items_state_check,
    DROP CONSTRAINT items_check,
    DROP CONSTRAINT items_package_id_manifestation_id_fkey,
    ALTER COLUMN package_id DROP NOT NULL,
    ADD COLUMN purge_eligible_at timestamptz,
    ADD COLUMN purged_at timestamptz;

UPDATE folioharbor.items
SET purge_eligible_at=deleted_at+interval '7 days'
WHERE state='deleted';

ALTER TABLE folioharbor.items
    ADD CONSTRAINT items_state_check CHECK (
        state IN ('active','deleted','purge_eligible','purged')
    ),
    ADD CONSTRAINT items_lifecycle_timestamps CHECK (
      (state='active' AND deleted_at IS NULL AND purge_eligible_at IS NULL AND purged_at IS NULL)
      OR
      (state IN ('deleted','purge_eligible') AND deleted_at IS NOT NULL
        AND purge_eligible_at=deleted_at+interval '7 days' AND purged_at IS NULL)
      OR
      (state='purged' AND deleted_at IS NOT NULL
        AND purge_eligible_at=deleted_at+interval '7 days' AND purged_at>=purge_eligible_at)
    ),
    ADD CONSTRAINT items_package_id_manifestation_id_fkey
      FOREIGN KEY(package_id,manifestation_id)
      REFERENCES folioharbor.publication_packages(package_id,manifestation_id)
      ON DELETE SET NULL (package_id);

CREATE INDEX items_deleted_purge_batch_idx
    ON folioharbor.items(purge_eligible_at,item_id)
    WHERE state='deleted';

ALTER TABLE folioharbor.content_units
    DROP CONSTRAINT content_units_package_id_fkey,
    ALTER COLUMN package_id DROP NOT NULL,
    ADD CONSTRAINT content_units_package_id_fkey
      FOREIGN KEY(package_id) REFERENCES folioharbor.publication_packages(package_id)
      ON DELETE SET NULL;

ALTER TABLE folioharbor.reading_states
    DROP CONSTRAINT reading_states_package_id_manifestation_id_fkey,
    DROP CONSTRAINT reading_states_package_id_content_unit_id_fkey,
    DROP CONSTRAINT reading_states_check,
    ADD CONSTRAINT reading_states_package_id_fkey
      FOREIGN KEY(package_id) REFERENCES folioharbor.publication_packages(package_id)
      ON DELETE SET NULL,
    ADD CONSTRAINT reading_states_content_unit_id_fkey
      FOREIGN KEY(content_unit_id) REFERENCES folioharbor.content_units(content_unit_id)
      ON DELETE RESTRICT;

ALTER TABLE folioharbor.reading_mutations
    DROP CONSTRAINT reading_mutations_global_package_id_manifestation_id_fkey,
    DROP CONSTRAINT reading_mutations_global_package_id_global_content_unit_id_fkey,
    DROP CONSTRAINT reading_mutations_check,
    ADD CONSTRAINT reading_mutations_global_package_id_fkey
      FOREIGN KEY(global_package_id) REFERENCES folioharbor.publication_packages(package_id)
      ON DELETE SET NULL,
    ADD CONSTRAINT reading_mutations_global_content_unit_id_fkey
      FOREIGN KEY(global_content_unit_id) REFERENCES folioharbor.content_units(content_unit_id)
      ON DELETE RESTRICT;

ALTER TABLE folioharbor.blob_locations
    DROP CONSTRAINT blob_locations_state_check,
    ADD COLUMN purge_pending_at timestamptz,
    ADD COLUMN purge_after timestamptz,
    ADD COLUMN purged_at timestamptz,
    ADD COLUMN purge_lease_owner text CHECK (
      purge_lease_owner IS NULL OR length(purge_lease_owner) BETWEEN 1 AND 128
    ),
    ADD COLUMN purge_lease_token uuid,
    ADD COLUMN purge_lease_expires_at timestamptz,
    ADD CONSTRAINT blob_locations_state_check CHECK (
      state IN ('staging','ready','quarantined','purge_pending','deleting','purged')
    );

UPDATE folioharbor.blob_locations
SET purge_pending_at=updated_at-interval '24 hours',purge_after=updated_at,purged_at=updated_at
WHERE state='purged';

ALTER TABLE folioharbor.blob_locations
    ADD CONSTRAINT blob_locations_lifecycle_timestamps CHECK (
      (state IN ('staging','ready','quarantined') AND purge_pending_at IS NULL
        AND purge_after IS NULL AND purged_at IS NULL
        AND purge_lease_owner IS NULL AND purge_lease_token IS NULL
        AND purge_lease_expires_at IS NULL)
      OR
      (state='purge_pending' AND purge_pending_at IS NOT NULL
        AND purge_after=purge_pending_at+interval '24 hours' AND purged_at IS NULL
        AND purge_lease_owner IS NULL AND purge_lease_token IS NULL
        AND purge_lease_expires_at IS NULL)
      OR
      (state='deleting' AND purge_pending_at IS NOT NULL
        AND purge_after=purge_pending_at+interval '24 hours' AND purged_at IS NULL
        AND purge_lease_owner IS NOT NULL AND purge_lease_token IS NOT NULL
        AND purge_lease_expires_at IS NOT NULL)
      OR
      (state='purged' AND purge_pending_at IS NOT NULL
        AND purge_after=purge_pending_at+interval '24 hours' AND purged_at>=purge_after
        AND purge_lease_owner IS NULL AND purge_lease_token IS NULL
        AND purge_lease_expires_at IS NULL)
    );

CREATE INDEX blob_locations_purge_batch_idx
    ON folioharbor.blob_locations(purge_after,blob_id,storage_key)
    WHERE state IN ('purge_pending','deleting');

CREATE FUNCTION folioharbor.blob_location_fill_legacy_purge_timestamps()
RETURNS trigger LANGUAGE plpgsql SET search_path TO '' AS $$
BEGIN
  IF NEW.state='purged' AND NEW.purged_at IS NULL THEN
    NEW.purge_after:=COALESCE(NEW.purge_after,NEW.updated_at);
    NEW.purge_pending_at:=COALESCE(
      NEW.purge_pending_at,NEW.purge_after-interval '24 hours'
    );
    NEW.purged_at:=NEW.updated_at;
    NEW.purge_lease_owner:=NULL;
    NEW.purge_lease_token:=NULL;
    NEW.purge_lease_expires_at:=NULL;
  END IF;
  RETURN NEW;
END $$;
REVOKE ALL ON FUNCTION folioharbor.blob_location_fill_legacy_purge_timestamps() FROM PUBLIC;
CREATE TRIGGER blob_locations_fill_legacy_purge_timestamps
    BEFORE UPDATE OF state ON folioharbor.blob_locations
    FOR EACH ROW EXECUTE FUNCTION folioharbor.blob_location_fill_legacy_purge_timestamps();

ALTER TABLE folioharbor.audit_events DROP CONSTRAINT audit_events_action_code_check;
ALTER TABLE folioharbor.audit_events ADD CONSTRAINT audit_events_action_code_check
    CHECK(action_code IN('library.view','library.manage','member.invite','member.role.change',
        'member.remove','publication.import','item.download','item.delete','item.restore'));

CREATE FUNCTION folioharbor.item_lifecycle_mutate_authorized(
    p_actor uuid,p_library uuid,p_item uuid,p_operation text,p_now timestamptz,
    p_membership_version bigint,p_request text
) RETURNS TABLE(
    outcome text,item_state text,item_deleted_at timestamptz,
    item_purge_eligible_at timestamptz,item_purged_at timestamptz
) LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE current_item folioharbor.items%ROWTYPE;
DECLARE expected_action text;
BEGIN
  IF session_user<>'folioharbor_api' OR p_actor IS DISTINCT FROM folioharbor.current_user_id()
     OR p_library IS DISTINCT FROM folioharbor.current_library_id()
     OR p_request IS DISTINCT FROM folioharbor.current_request_id()
     OR p_operation NOT IN ('delete','restore') THEN
    RETURN QUERY SELECT 'not_found'::text,NULL::text,NULL::timestamptz,NULL::timestamptz,NULL::timestamptz;
    RETURN;
  END IF;
  SELECT item.* INTO current_item
  FROM folioharbor.items item
  JOIN folioharbor.holdings holding USING(holding_id)
  WHERE item.item_id=p_item AND holding.library_id=p_library
  FOR UPDATE OF item;
  IF current_item.item_id IS NULL THEN
    RETURN QUERY SELECT 'not_found'::text,NULL::text,NULL::timestamptz,NULL::timestamptz,NULL::timestamptz;
    RETURN;
  END IF;
  IF NOT folioharbor.library_revalidate_grant(
      p_actor,p_library,'holding.edit',p_membership_version
  ) THEN
    RETURN QUERY SELECT 'forbidden'::text,NULL::text,NULL::timestamptz,NULL::timestamptz,NULL::timestamptz;
    RETURN;
  END IF;
  IF p_operation='delete' THEN
    IF current_item.state='active' THEN
      UPDATE folioharbor.items SET state='deleted',deleted_at=p_now,
        purge_eligible_at=p_now+interval '7 days',purged_at=NULL
      WHERE item_id=p_item RETURNING * INTO current_item;
    END IF;
    expected_action:='item.delete';
  ELSE
    IF current_item.state='deleted' AND p_now<current_item.purge_eligible_at THEN
      UPDATE folioharbor.items SET state='active',deleted_at=NULL,
        purge_eligible_at=NULL,purged_at=NULL
      WHERE item_id=p_item RETURNING * INTO current_item;
    ELSIF current_item.state<>'active' THEN
      RETURN QUERY SELECT 'window_elapsed'::text,current_item.state,current_item.deleted_at,
        current_item.purge_eligible_at,current_item.purged_at;
      RETURN;
    END IF;
    expected_action:='item.restore';
  END IF;
  PERFORM folioharbor.audit_record_allowed(
    gen_random_uuid(),p_actor,p_actor,p_library,expected_action,'item',p_item,'allowed',NULL,
    p_request,'api',p_now,NULL,expected_action,'item',p_item
  );
  RETURN QUERY SELECT 'applied'::text,current_item.state,current_item.deleted_at,
    current_item.purge_eligible_at,current_item.purged_at;
END $$;
ALTER FUNCTION folioharbor.item_lifecycle_mutate_authorized(uuid,uuid,uuid,text,timestamptz,bigint,text)
    OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.item_lifecycle_mutate_authorized(uuid,uuid,uuid,text,timestamptz,bigint,text)
    FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.item_lifecycle_mutate_authorized(uuid,uuid,uuid,text,timestamptz,bigint,text)
    TO folioharbor_api;

CREATE FUNCTION folioharbor.blob_reference_guard()
RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE referenced_blob uuid;
BEGIN
  referenced_blob:=(to_jsonb(NEW)->>TG_ARGV[0])::uuid;
  PERFORM 1 FROM folioharbor.blobs WHERE blob_id=referenced_blob FOR KEY SHARE;
  IF NOT FOUND OR EXISTS(
      SELECT 1 FROM folioharbor.blob_locations
      WHERE blob_id=referenced_blob AND state IN ('deleting','purged')
  ) THEN
    RAISE EXCEPTION 'Blob is not referenceable' USING ERRCODE='55000';
  END IF;
  RETURN NEW;
END $$;
ALTER FUNCTION folioharbor.blob_reference_guard() OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.blob_reference_guard() FROM PUBLIC;
CREATE TRIGGER item_assets_blob_reference_guard
    BEFORE INSERT OR UPDATE OF blob_id ON folioharbor.item_assets
    FOR EACH ROW EXECUTE FUNCTION folioharbor.blob_reference_guard('blob_id');
CREATE TRIGGER manifestation_assets_blob_reference_guard
    BEFORE INSERT OR UPDATE OF blob_id ON folioharbor.manifestation_assets
    FOR EACH ROW EXECUTE FUNCTION folioharbor.blob_reference_guard('blob_id');
CREATE TRIGGER publication_packages_blob_reference_guard
    BEFORE INSERT OR UPDATE OF blob_id ON folioharbor.publication_packages
    FOR EACH ROW EXECUTE FUNCTION folioharbor.blob_reference_guard('blob_id');
CREATE TRIGGER publication_resources_blob_reference_guard
    BEFORE INSERT OR UPDATE OF source_blob_id ON folioharbor.publication_resources
    FOR EACH ROW EXECUTE FUNCTION folioharbor.blob_reference_guard('source_blob_id');

CREATE FUNCTION folioharbor.blob_candidate_reference_guard()
RETURNS trigger LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE referenced_blob uuid;
BEGIN
  SELECT blob_id INTO referenced_blob FROM folioharbor.blob_locations
    WHERE storage_key=NEW.storage_key;
  IF referenced_blob IS NULL THEN RETURN NEW; END IF;
  PERFORM 1 FROM folioharbor.blobs WHERE blob_id=referenced_blob FOR KEY SHARE;
  IF EXISTS(
      SELECT 1 FROM folioharbor.blob_locations
      WHERE blob_id=referenced_blob AND storage_key=NEW.storage_key
        AND state IN ('deleting','purged')
  ) THEN
    RAISE EXCEPTION 'Blob candidate is not referenceable' USING ERRCODE='55000';
  END IF;
  RETURN NEW;
END $$;
ALTER FUNCTION folioharbor.blob_candidate_reference_guard() OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.blob_candidate_reference_guard() FROM PUBLIC;
CREATE TRIGGER blob_candidates_reference_guard
    BEFORE INSERT OR UPDATE OF storage_key ON folioharbor.blob_reachability_candidates
    FOR EACH ROW EXECUTE FUNCTION folioharbor.blob_candidate_reference_guard();

CREATE FUNCTION folioharbor.blob_has_authoritative_reference(p_blob uuid)
RETURNS boolean LANGUAGE sql STABLE SECURITY DEFINER SET search_path TO '' AS $$
  SELECT EXISTS(SELECT 1 FROM folioharbor.item_assets WHERE blob_id=p_blob)
      OR EXISTS(SELECT 1 FROM folioharbor.manifestation_assets WHERE blob_id=p_blob)
      OR EXISTS(SELECT 1 FROM folioharbor.publication_packages WHERE blob_id=p_blob)
      OR EXISTS(SELECT 1 FROM folioharbor.publication_resources WHERE source_blob_id=p_blob)
      OR EXISTS(
        SELECT 1 FROM folioharbor.blob_reachability_candidates candidate
        JOIN folioharbor.blob_locations location USING(storage_key)
        WHERE location.blob_id=p_blob
      )
$$;
ALTER FUNCTION folioharbor.blob_has_authoritative_reference(uuid) OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.blob_has_authoritative_reference(uuid) FROM PUBLIC;

CREATE FUNCTION folioharbor.gc_prepare_items_worker(p_now timestamptz,p_limit bigint)
RETURNS bigint LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE candidate record;
DECLARE candidate_blob uuid;
DECLARE candidate_blobs uuid[];
DECLARE logical_bytes bigint;
DECLARE processed bigint:=0;
BEGIN
  IF session_user<>'folioharbor_worker' OR NOT folioharbor.is_worker()
     OR p_limit<1 OR p_limit>1000 THEN RETURN 0; END IF;
  FOR candidate IN
    SELECT item.item_id,item.package_id,item.manifestation_id,holding.library_id
    FROM folioharbor.items item JOIN folioharbor.holdings holding USING(holding_id)
    WHERE item.state='deleted' AND item.purge_eligible_at<=p_now
    ORDER BY item.purge_eligible_at,item.item_id
    LIMIT p_limit FOR UPDATE OF item SKIP LOCKED
  LOOP
    SELECT COALESCE(array_agg(DISTINCT reference.blob_id),ARRAY[]::uuid[])
    INTO candidate_blobs FROM (
      SELECT asset.blob_id FROM folioharbor.item_assets asset WHERE asset.item_id=candidate.item_id
      UNION SELECT package.blob_id FROM folioharbor.publication_packages package WHERE package.package_id=candidate.package_id
      UNION SELECT resource.source_blob_id FROM folioharbor.publication_resources resource WHERE resource.package_id=candidate.package_id
      UNION SELECT asset.blob_id FROM folioharbor.manifestation_assets asset WHERE asset.manifestation_id=candidate.manifestation_id
    ) reference;
    PERFORM 1 FROM folioharbor.libraries WHERE library_id=candidate.library_id FOR UPDATE;
    PERFORM blob.blob_id FROM folioharbor.blobs blob
      WHERE blob.blob_id=ANY(candidate_blobs) ORDER BY blob.blob_id FOR UPDATE;
    SELECT COALESCE(sum(blob.byte_size),0) INTO logical_bytes
      FROM folioharbor.item_assets asset JOIN folioharbor.blobs blob USING(blob_id)
      WHERE asset.item_id=candidate.item_id;
    IF (SELECT quota_used_bytes FROM folioharbor.libraries WHERE library_id=candidate.library_id)
       < logical_bytes THEN
      RAISE EXCEPTION 'logical quota underflow' USING ERRCODE='23514';
    END IF;
    UPDATE folioharbor.items SET state='purge_eligible' WHERE item_id=candidate.item_id;
    UPDATE folioharbor.libraries SET quota_used_bytes=quota_used_bytes-logical_bytes,
      updated_at=p_now WHERE library_id=candidate.library_id;
    DELETE FROM folioharbor.item_assets WHERE item_id=candidate.item_id;
    UPDATE folioharbor.items SET package_id=NULL WHERE item_id=candidate.item_id;
    IF candidate.package_id IS NOT NULL AND NOT EXISTS(
      SELECT 1 FROM folioharbor.items WHERE package_id=candidate.package_id
    ) THEN
      DELETE FROM folioharbor.manifestation_assets
        WHERE manifestation_id=candidate.manifestation_id;
      DELETE FROM folioharbor.publication_packages WHERE package_id=candidate.package_id;
    END IF;
    UPDATE folioharbor.items SET state='purged',purged_at=p_now WHERE item_id=candidate.item_id;
    FOREACH candidate_blob IN ARRAY candidate_blobs LOOP
      IF NOT folioharbor.blob_has_authoritative_reference(candidate_blob) THEN
        UPDATE folioharbor.blob_locations SET state='purge_pending',
          purge_pending_at=p_now,purge_after=p_now+interval '24 hours',purged_at=NULL,
          purge_lease_owner=NULL,purge_lease_token=NULL,purge_lease_expires_at=NULL,updated_at=p_now
        WHERE blob_id=candidate_blob AND state='ready';
      END IF;
    END LOOP;
    processed:=processed+1;
  END LOOP;
  RETURN processed;
END $$;
ALTER FUNCTION folioharbor.gc_prepare_items_worker(timestamptz,bigint) OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.gc_prepare_items_worker(timestamptz,bigint) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.gc_prepare_items_worker(timestamptz,bigint) TO folioharbor_worker;

CREATE FUNCTION folioharbor.gc_claim_blobs_worker(
  p_worker text,p_now timestamptz,p_limit bigint
) RETURNS TABLE(blob_id uuid,storage_key text,lease_token uuid) LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
DECLARE candidate record;
BEGIN
  IF session_user<>'folioharbor_worker' OR NOT folioharbor.is_worker()
     OR length(p_worker) NOT BETWEEN 1 AND 128 OR p_limit<1 OR p_limit>1000 THEN RETURN; END IF;
  FOR candidate IN
    SELECT location.blob_id,location.storage_key
    FROM folioharbor.blob_locations location
    WHERE (location.state='purge_pending' AND location.purge_after<=p_now)
       OR (location.state='deleting' AND location.purge_lease_expires_at<=p_now)
    ORDER BY location.purge_after,location.blob_id,location.storage_key
    LIMIT p_limit FOR UPDATE SKIP LOCKED
  LOOP
    PERFORM 1 FROM folioharbor.blobs blob WHERE blob.blob_id=candidate.blob_id FOR UPDATE;
    IF folioharbor.blob_has_authoritative_reference(candidate.blob_id) THEN
      UPDATE folioharbor.blob_locations location SET state='ready',purge_pending_at=NULL,
        purge_after=NULL,purged_at=NULL,purge_lease_owner=NULL,purge_lease_token=NULL,
        purge_lease_expires_at=NULL,
        updated_at=p_now WHERE location.blob_id=candidate.blob_id;
    ELSE
      UPDATE folioharbor.blob_locations location SET state='deleting',purge_lease_owner=p_worker,
        purge_lease_token=gen_random_uuid(),purge_lease_expires_at=p_now+interval '5 minutes',
        updated_at=p_now
      WHERE location.blob_id=candidate.blob_id AND location.storage_key=candidate.storage_key
      RETURNING location.purge_lease_token INTO lease_token;
      blob_id:=candidate.blob_id; storage_key:=candidate.storage_key; RETURN NEXT;
    END IF;
  END LOOP;
END $$;
ALTER FUNCTION folioharbor.gc_claim_blobs_worker(text,timestamptz,bigint) OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.gc_claim_blobs_worker(text,timestamptz,bigint) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.gc_claim_blobs_worker(text,timestamptz,bigint) TO folioharbor_worker;

CREATE FUNCTION folioharbor.gc_complete_blob_worker(
  p_blob uuid,p_storage text,p_worker text,p_token uuid,p_now timestamptz
) RETURNS boolean LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
BEGIN
  IF session_user<>'folioharbor_worker' OR NOT folioharbor.is_worker() THEN RETURN false; END IF;
  UPDATE folioharbor.blob_locations SET state='purged',purged_at=p_now,
    purge_lease_owner=NULL,purge_lease_token=NULL,purge_lease_expires_at=NULL,updated_at=p_now
  WHERE blob_id=p_blob AND storage_key=p_storage AND state='deleting'
    AND purge_lease_owner=p_worker AND purge_lease_token=p_token;
  RETURN FOUND;
END $$;
ALTER FUNCTION folioharbor.gc_complete_blob_worker(uuid,text,text,uuid,timestamptz) OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.gc_complete_blob_worker(uuid,text,text,uuid,timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.gc_complete_blob_worker(uuid,text,text,uuid,timestamptz) TO folioharbor_worker;

CREATE FUNCTION folioharbor.gc_release_blob_worker(
  p_blob uuid,p_storage text,p_worker text,p_token uuid,p_now timestamptz
) RETURNS boolean LANGUAGE plpgsql SECURITY DEFINER SET search_path TO '' AS $$
BEGIN
  IF session_user<>'folioharbor_worker' OR NOT folioharbor.is_worker() THEN RETURN false; END IF;
  UPDATE folioharbor.blob_locations SET state='purge_pending',purge_lease_owner=NULL,
    purge_lease_token=NULL,purge_lease_expires_at=NULL,updated_at=p_now
  WHERE blob_id=p_blob AND storage_key=p_storage AND state='deleting'
    AND purge_lease_owner=p_worker AND purge_lease_token=p_token;
  RETURN FOUND;
END $$;
ALTER FUNCTION folioharbor.gc_release_blob_worker(uuid,text,text,uuid,timestamptz) OWNER TO folioharbor_owner;
REVOKE ALL ON FUNCTION folioharbor.gc_release_blob_worker(uuid,text,text,uuid,timestamptz) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION folioharbor.gc_release_blob_worker(uuid,text,text,uuid,timestamptz) TO folioharbor_worker;

UPDATE folioharbor.schema_metadata SET schema_version=25,applied_at=clock_timestamp()
WHERE singleton;
